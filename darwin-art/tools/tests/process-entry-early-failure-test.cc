#include "darwin_art/darwin_art.h"
#include "darwin_art_bionic_process_state.h"

#include <dlfcn.h>

#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>

namespace {

using InstallSnapshot = int (*)(const DarwinArtProcessSnapshotConfig*);
using RunProcess = int32_t (*)(const darwin_art_process_config_t*,
                               darwin_art_process_result_t*);

struct LifecycleState {
  int begin_count = 0;
  int finish_count = 0;
  int finish_runtime_created = -1;
  int shutdown_count = 0;
  int failed_count = 0;
};

int BeginRun(void* context) {
  ++static_cast<LifecycleState*>(context)->begin_count;
  return 0;
}

int FinishRun(void* context, int32_t runtime_created) {
  auto* state = static_cast<LifecycleState*>(context);
  ++state->finish_count;
  state->finish_runtime_created = runtime_created;
  return 0;
}

int BeginShutdown(void* context) {
  ++static_cast<LifecycleState*>(context)->shutdown_count;
  return 0;
}

void MarkFailed(void* context, int32_t) {
  ++static_cast<LifecycleState*>(context)->failed_count;
}

void* Resolve(void* library, const char* name) {
  dlerror();
  void* symbol = dlsym(library, name);
  const char* error = dlerror();
  if (error != nullptr || symbol == nullptr) {
    std::fprintf(stderr, "missing product symbol %s: %s\n", name,
                 error == nullptr ? "null" : error);
    std::abort();
  }
  return symbol;
}

void InstallTrustedSnapshot(InstallSnapshot install_snapshot) {
  DarwinArtProcessSnapshotConfig snapshot{};
  snapshot.abi_version = 1;
  snapshot.struct_size = sizeof(snapshot);
  snapshot.page_size = 16384;
  snapshot.hwcap = UINT64_C(3);
  snapshot.hwcap2 = UINT64_C(0);
  snapshot.secure = 0;
  for (uint8_t& value : snapshot.random) value = 0x5a;
  assert(install_snapshot(&snapshot) == 0);
}

darwin_art_process_config_t ProcessConfig(
    LifecycleState* lifecycle, const darwin_art_host_services_t* host_services,
    void* graphics_session_context) {
  static const char kCoreOjJar[] = "dummy-core-oj.jar";
  static const char kCoreLibartJar[] = "dummy-core-libart.jar";
  static const char kFrameworkJar[] = "dummy-framework.jar";
  static const char kCoreIcu4jJar[] = "dummy-core-icu4j.jar";
  static const char kAppDex[] = "dummy-app.dex";
  static const darwin_art_lifecycle_hooks_t kLifecycleHooks{
      sizeof(kLifecycleHooks),
      DARWIN_ART_ABI_VERSION,
      lifecycle,
      &BeginRun,
      &FinishRun,
      &BeginShutdown,
      &MarkFailed,
  };
  return darwin_art_process_config_t{
      sizeof(darwin_art_process_config_t),
      DARWIN_ART_ABI_VERSION,
      kCoreOjJar,
      kCoreLibartJar,
      kFrameworkJar,
      kCoreIcu4jJar,
      kAppDex,
      0,
      0,
      nullptr,
      nullptr,
      nullptr,
      nullptr,
      nullptr,
      graphics_session_context,
      &kLifecycleHooks,
      host_services,
      nullptr,
      nullptr,
      nullptr,
  };
}

void RunCase(const char* library_path, const char* test_case) {
  void* library = dlopen(library_path, RTLD_NOW | RTLD_GLOBAL);
  if (library == nullptr) {
    std::fprintf(stderr, "dlopen(%s): %s\n", library_path, dlerror());
    std::abort();
  }
  auto install_snapshot = reinterpret_cast<InstallSnapshot>(
      Resolve(library, "darwin_art_bionic_process_state_install_configured"));
  auto run_process =
      reinterpret_cast<RunProcess>(Resolve(library, "darwin_art_run_process"));
  InstallTrustedSnapshot(install_snapshot);

  LifecycleState lifecycle;
  darwin_art_host_services_t invalid_services{
      sizeof(darwin_art_host_services_t), DARWIN_ART_ABI_VERSION, nullptr,
      nullptr, nullptr};
  constexpr uintptr_t kInvalidGraphicsContext = static_cast<uintptr_t>(0x1234u);
  const bool invalid_host_services =
      std::strcmp(test_case, "invalid-host-services") == 0;
  const bool invalid_graphics_context =
      std::strcmp(test_case, "invalid-graphics-context") == 0;
  assert(invalid_host_services != invalid_graphics_context);
  auto config = ProcessConfig(
      &lifecycle, invalid_host_services ? &invalid_services : nullptr,
      invalid_graphics_context
          ? reinterpret_cast<void*>(kInvalidGraphicsContext)
          : nullptr);
  darwin_art_process_result_t result{
      sizeof(darwin_art_process_result_t), DARWIN_ART_ABI_VERSION,
      INT32_C(-101), INT32_C(-102), INT32_C(-103), INT32_C(-104),
      INT32_C(-105), UINT32_C(0), UINT32_C(0)};
  const int32_t status = run_process(&config, &result);
  const int32_t expected = invalid_host_services
                               ? INT32_C(64)
                               : DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  assert(status == expected);
  assert(lifecycle.begin_count == 1);
  assert(lifecycle.finish_count == 1);
  assert(lifecycle.finish_runtime_created == 0);
  assert(lifecycle.shutdown_count == 0);
  assert(lifecycle.failed_count == 0);
  dlclose(library);
  std::printf("process-entry early failure: %s status=%d begin=1 finish=1 "
              "runtime_created=0 shutdown=0 PASS\n",
              test_case, status);
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 3 || (std::strcmp(argv[2], "invalid-host-services") != 0 &&
                    std::strcmp(argv[2], "invalid-graphics-context") != 0)) {
    std::fprintf(stderr,
                 "usage: %s PRODUCT_DYLIB invalid-host-services|"
                 "invalid-graphics-context\n",
                 argv[0]);
    return 2;
  }
  RunCase(argv[1], argv[2]);
  return 0;
}
