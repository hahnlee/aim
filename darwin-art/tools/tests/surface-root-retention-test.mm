#include "compat/darwin_surface_internal.h"
#include "compat/window/desktop_root_surface.h"
#include "runtime/framework/input/surface_input_context.h"

#include <cassert>
#include <cstdio>
#include <thread>

using namespace darwin_art::window;
using darwin_art::input::SurfaceInputContext;

namespace {
NSWindow* Window() {
  NSWindow* window = [[NSWindow alloc]
      initWithContentRect:NSMakeRect(0, 0, 100, 100)
      styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
  window.releasedWhenClosed = NO;
  return window;
}
DarwinArtSurfaceInputResult Pointer(void*, const DarwinArtPointerEventV2*) {
  return DARWIN_ART_SURFACE_INPUT_NO_TARGET;
}
DarwinArtSurfaceInputResult Key(void*, const DarwinArtKeyEventV1*) {
  return DARWIN_ART_SURFACE_INPUT_NO_TARGET;
}
}

int main() {
  @autoreleasepool {
    [NSApplication sharedApplication];
    auto surface = std::make_unique<DarwinArtSurface>();
    surface->window = Window();
    assert(InitializeDesktopRoot(surface.get()) == DARWIN_ART_SURFACE_OK);
    auto root = surface->desktop_root_events;
    std::weak_ptr<DesktopRootEvents> weak_root = root;
    const auto initial = root->Snapshot();
    std::shared_ptr<DesktopRootEvents> retained;
    std::thread owner([&] {
      assert(RetainSurfaceDesktopRoot(surface.get(), &retained) ==
             DARWIN_ART_SURFACE_OK);
    });
    owner.join();
    assert(retained == root);
    auto* context = SurfaceInputContext::Create(retained);
    assert(context != nullptr);
    DarwinArtSurfaceInputSink sink{
        1, sizeof(DarwinArtSurfaceInputSink), context,
        SurfaceInputContext::Retain, SurfaceInputContext::Release, Pointer, Key};
    assert(darwin_art_surface_set_input_sink(surface.get(), &sink) ==
           DARWIN_ART_SURFACE_OK);
    auto binding = surface->input_sink;
    SurfaceInputContext::Release(context);
    auto callback_snapshot = binding;

    // A second published root makes the process catalog ambiguous. Exact
    // surface acquisition and existing callback identity must not change.
    DarwinArtSurface successor;
    successor.window = Window();
    assert(InitializeDesktopRoot(&successor) == DARWIN_ART_SURFACE_OK);
    assert(AcquireProcessDesktopRootTarget() == nullptr);
    std::shared_ptr<DesktopRootEvents> again;
    assert(RetainSurfaceDesktopRoot(surface.get(), &again) == DARWIN_ART_SURFACE_OK);
    assert(again == root && context->root() == root);
    const auto after = root->Snapshot();
    assert(after.state_revision == initial.state_revision);
    assert(after.latest_emitted_serial == initial.latest_emitted_serial);
    assert(after.key == initial.key && after.closed == initial.closed);

    auto* replacement = SurfaceInputContext::Create(successor.desktop_root_events);
    assert(replacement != nullptr);
    sink.context = replacement;
    assert(darwin_art_surface_set_input_sink(surface.get(), &sink) ==
           DARWIN_ART_SURFACE_OK);
    SurfaceInputContext::Release(replacement);
    assert(context->root() == root);
    assert(darwin_art_surface_set_input_sink(surface.get(), nullptr) ==
           DARWIN_ART_SURFACE_OK);
    assert(context->root() == root && !root->Snapshot().closed);

    assert(RetainSurfaceDesktopRoot(nullptr, &again) == DARWIN_ART_SURFACE_INVALID_ARGUMENT);
    DarwinArtSurface missing;
    assert(RetainSurfaceDesktopRoot(&missing, &again) == DARWIN_ART_SURFACE_INVALID_ARGUMENT);
    assert(again == root);
    assert(surface->desktop_root_target->Close());
    assert(RetainSurfaceDesktopRoot(surface.get(), &again) == DARWIN_ART_SURFACE_WINDOW_CLOSED);
    [surface->window close];
    surface.reset();
    binding.reset();
    assert(context->root() == root && context->root()->Snapshot().closed);
    root.reset();
    retained.reset();
    again.reset();
    assert(!weak_root.expired());
    callback_snapshot.reset();
    assert(weak_root.expired());
    assert(successor.desktop_root_target->Close());
    [successor.window close];
  }
  std::puts("surface exact-root retention: PASS");
}
