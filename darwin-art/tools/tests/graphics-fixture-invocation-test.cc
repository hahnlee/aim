#include "probes/graphics_fixture_state.h"

#include <cassert>

using namespace darwin_art_graphics_fixture;
namespace {
int deleted_globals = 0;
int endpoint_cleanup = 0;
darwin_art_graphics::GraphicsState* cleanup_state = nullptr;
void DeleteGlobal(JNIEnv*, jobject) { ++deleted_globals; }
}

extern "C" DarwinArtSurfaceResult darwin_art_surface_set_input_sink(
    DarwinArtSurface*, const DarwinArtSurfaceInputSink*) {
  return DARWIN_ART_SURFACE_OK;
}
extern "C" DarwinArtSurface* darwin_art_surface_active_gpu() { return nullptr; }
extern "C" DarwinArtSurfaceResult darwin_art_surface_set_owner_wake(
    DarwinArtSurface*, DarwinArtSurfaceOwnerWakeCallback, void*) {
  return DARWIN_ART_SURFACE_OK;
}
namespace darwin_art_graphics_fixture {
// Endpoint resource disposal is a separate tested component. This test runs
// actual sidecar registry/cleanup and checks its reentrant ownership boundary.
void DisposeFixtureInputEndpoints(GraphicsFixtureState* fixture, JNIEnv* env) {
  ++endpoint_cleanup;
  assert(fixture->retiring.load() && fixture->active_invocations == 0);
  if (cleanup_state != nullptr) {
    assert(GetGraphicsFixtureState(cleanup_state) == fixture);
    assert(EnsureGraphicsFixtureState(cleanup_state) == fixture);
    GraphicsFixtureInvocation rejected(cleanup_state, env);
    assert(rejected.get() == nullptr);
    ClearProbeCanvasState(cleanup_state, env); // No recursive cleanup/deadlock.
  }
}
}
int main() {
  JNINativeInterface table{};
  table.DeleteGlobalRef = DeleteGlobal;
  JNIEnv env{&table};
  darwin_art_graphics::GraphicsState state;
  cleanup_state = &state;
  {
    GraphicsFixtureInvocation outer(&state, &env);
    auto* fixture = outer.get();
    assert(fixture != nullptr && fixture->active_invocations == 1);
    fixture->interactive_root = reinterpret_cast<jobject>(0x1234);
    {
      GraphicsFixtureInvocation nested(&state, &env);
      assert(nested.get() == fixture && fixture->active_invocations == 2);
      ClearProbeCanvasState(&state, &env);
      assert(fixture->retiring.load());
      assert(GetGraphicsFixtureState(&state) == fixture);
      assert(deleted_globals == 0 && endpoint_cleanup == 0);
      GraphicsFixtureInvocation rejected(&state, &env);
      assert(rejected.get() == nullptr);
    }
    assert(fixture->active_invocations == 1 && deleted_globals == 0);
    ClearAllProbeCanvasState(&env);
    assert(GetGraphicsFixtureState(&state) == fixture);
    assert(fixture->interactive_root != nullptr && endpoint_cleanup == 0);
  }
  assert(GetGraphicsFixtureState(&state) == nullptr);
  assert(deleted_globals == 1 && endpoint_cleanup == 1);
  // A later invocation can establish a new sidecar only after the old one
  // finished cleanup; it could not do so inside disposal/reentrant progress.
  {
    GraphicsFixtureInvocation next(&state, &env);
    assert(next.get() != nullptr && !next.get()->retiring.load());
  }
  ClearAllProbeCanvasState(&env);
  assert(endpoint_cleanup == 2 && deleted_globals == 1);
}
