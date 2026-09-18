#include "process_entry.h"

#include <cstddef>
#include <cstdlib>
#include <iostream>
#include <string>

#include "darwin_art/darwin_art.h"
#include "../../compat/darwin_provider_owners.h"
#include "../../compat/jni/scoped_local_frame.h"
#include "../../compat/process/host_services.h"
#include "../../compat/binder/native_endpoint_lifetime.h"
#include "../art/native_registration.h"
#include "../art/process_state.h"
#include "../art/vm_bootstrap.h"
#include "../framework/app/process_entry.h"
#include "../framework/input/event_ingress.h"
#include "../framework/pm/installed_record_source.h"
#include "../framework/system/process_entry.h"
#include "graphics_session.h"
#include "process_config.h"

#include "interpreter/unstarted_runtime.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"
#include "well_known_classes.h"

namespace darwin_art::embedding {
namespace {

jstring ResolveInstalledPackage(JNIEnv* env, jclass, jstring package_name) {
  return darwin_art::framework::pm::QueryInstalledRecord(
      env, std::getenv("DARWIN_ART_PROFILE_SOCKET"), package_name);
}

bool HasTail(const darwin_art_process_config_t* config, size_t offset,
             size_t size) {
  return config != nullptr && config->struct_size >= offset + size;
}

}  // namespace

int32_t RunProcess(const darwin_art_process_config_t* config,
                   darwin_art_process_result_t* result) {
  ProcessConfigBounds bounds;
  std::string error;
  const int config_status =
      ValidateProcessConfig(config, result, &bounds, &error);
  if (config_status != 0) {
    std::cerr << "darwin_art_run_process: " << error << "\n";
    return config_status;
  }

  const auto* lifecycle_hooks =
      HasTail(config, offsetof(darwin_art_process_config_t, lifecycle_hooks),
              sizeof(config->lifecycle_hooks))
          ? config->lifecycle_hooks
          : nullptr;
  if (!darwin_art_process::begin_run(lifecycle_hooks))
    return DARWIN_ART_STATUS_PROCESS_ALREADY_STARTED;
  // Every fallible acquisition after lifecycle admission must complete that
  // admission, including failures before an ART thread or session is bound.
  darwin_art_process::ScopedRunBoundary process_boundary;

  const auto* host_services =
      HasTail(config, offsetof(darwin_art_process_config_t, host_services),
              sizeof(config->host_services))
          ? config->host_services
          : nullptr;
  if (!darwin_art::process::InstallHostServices(host_services)) return 64;

  const auto* binder_authority =
      HasTail(config, offsetof(darwin_art_process_config_t, binder_authority_hooks),
              sizeof(config->binder_authority_hooks))
          ? config->binder_authority_hooks
          : nullptr;
  if (!darwin_art::binder::InstallNativeBinderAuthority(binder_authority)) return 64;
  struct PreRuntimeBinderOwner final {
    bool runtime_created = false;
    ~PreRuntimeBinderOwner() {
      if (!runtime_created) {
        // No Binder nodes can be admitted before VM/native registration. A
        // failed synchronous request must release its retained host capability.
        darwin_art::binder::CloseNativeBinderEndpointAdmission();
        darwin_art::binder::PollNativeBinderEndpointsQuiesced();
      }
    }
  } pre_runtime_binder_owner;

  void* graphics_context =
      HasTail(config,
              offsetof(darwin_art_process_config_t, graphics_session_context),
              sizeof(config->graphics_session_context))
          ? config->graphics_session_context
          : nullptr;
  if (graphics_context != nullptr &&
      darwin_art_graphics::bind_session_for_process(graphics_context) != 0)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  darwin_art_process::record_graphics_state(
      darwin_art_graphics::state_for_context(graphics_context));

  ProcessConfigOptions process_config;
  if (LoadProcessConfig(&process_config, &error) != 0) {
    std::cerr << "darwin_art_run_process: " << error << "\n";
    return 48;
  }
  if (!process_config.run_apk_app && !process_config.system_server_mode) {
    std::cerr << "darwin_art_run_process: installed APK identity is required\n";
    return 48;
  }

  const bool system_server = process_config.system_server_mode;
  void* desktop_surface_context =
      HasTail(config, offsetof(darwin_art_process_config_t, desktop_surface_context),
              sizeof(config->desktop_surface_context))
          ? config->desktop_surface_context
          : nullptr;
  if (system_server && desktop_surface_context != nullptr) {
    std::cerr << "darwin_art_run_process: system service cannot own an app display\n";
    return 64;
  }
  std::string class_path = process_config.apk_app_support_dex;
  if (system_server &&
      !darwin_art::framework::system::BuildSystemClassPath(
          process_config.android_filesystem_root.c_str(),
          process_config.apk_app_support_dex.c_str(), &class_path)) {
    std::cerr << "darwin_art_run_process: invalid service classpath inputs\n";
    return 70;
  }
  runtime_art::VmBootstrapResult vm;
  const int vm_status = runtime_art::CreateVm(config, bounds, class_path, &vm);
  if (vm_status != 0) return vm_status;
  pre_runtime_binder_owner.runtime_created = true;
  darwin_art_process::record_created_runtime(vm.self);
  process_boundary.set_art_thread(vm.self);
  if (graphics_context != nullptr &&
      darwin_art_graphics::bind_session_art_thread(vm.self) != 0)
    return 33;
  if (HasTail(config, offsetof(darwin_art_process_config_t, provider_acquire),
              sizeof(config->provider_acquire)) &&
      config->provider_acquire != nullptr) {
    darwin_art::providers::darwin_art_provider_install_hooks(
        config->provider_context, config->provider_acquire,
        config->provider_release);
  }

  JNIEnv* env = vm.env;
  art::Thread* self = vm.self;
  art::interpreter::UnstartedRuntime::Initialize();
  art::ScopedObjectAccess soa(self);
  darwin_art_jni_scope::ScopedLocalFrame frame(env);
  if (!frame.valid()) return 34;
  art::WellKnownClasses::Init(env);
  const int registration_status = runtime_art::StartNativeRegistration(env, self);
  if (registration_status != 0) return registration_status;

  if (desktop_surface_context != nullptr) {
    // Runtime::Start (within native registration) restores the real owner
    // to kNative. Runtime::Create alone leaves it runnable. Activate the
    // exact borrowed display only after that upstream transition, before
    // ActivityThread prepares its Java Looper and enters application code.
    const auto input_status =
        darwin_art_graphics::input::darwin_art_android_input_sink_install(
            static_cast<DarwinArtSurface*>(desktop_surface_context));
    if (input_status != DARWIN_ART_SURFACE_OK) {
      std::cerr << "darwin_art_run_process: desktop input activation failed status="
                << input_status << "\n";
      return 33;
    }
  }

  if (system_server) {
    const int status = darwin_art::framework::system::RunSystemProcess(
        env, std::getenv("DARWIN_ART_SYSTEM_SERVER_SOCKET"),
        &ResolveInstalledPackage);
    result->hello_answer = 0;
    result->native_round_trip = 0;
    result->arraycopy_result = 0;
    result->activity_probe_result = 0;
    result->lifecycle_result = 0;
    result->frame_width = 0;
    result->frame_height = 0;
    return status;
  }

  // ActivityThread.main() owns Looper preparation, Activity/LoadedApk
  // creation, Binder callbacks and the application UI thread.  No Probe
  // class or direct View construction is reachable from this entry.
  return darwin_art::framework::app::RunApplicationProcess(env, self);
}

}  // namespace darwin_art::embedding

extern "C" DARWIN_ART_EXPORT int32_t darwin_art_run_process(
    const darwin_art_process_config_t* config,
    darwin_art_process_result_t* result) {
  return darwin_art::embedding::RunProcess(config, result);
}
