#include "process_shutdown.h"
#include "../framework/input/channel_identity_catalog.h"
#include "../framework/input/root_key_ingress.h"
#include "../framework/wm/desktop_root_client_jni.h"
#include "../framework/wm/root_key_decision_jni.h"
#include "aim/android_runtime_host.h"

#include <iostream>
#include <new>

#include "aim/aim.h"
#include "darwin_framework_natives.h"
#include "darwin_icu_natives.h"
#include "darwin_libcore_natives.h"
#include "../art/process_state.h"
#include "../art/vm_shutdown.h"
#include "../art/shutdown_readiness.h"
#include "../../compat/process/host_services.h"
#include "../../compat/binder/native_endpoint_lifetime.h"
#include "../../compat/window/blast_buffer_queue_jni.h"
#include "../../compat/window/surface_transaction_submission_lifecycle.h"
#include "graphics_session.h"
#include "runtime.h"
#include "jni/java_vm_ext.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"

namespace android {
bool ShutdownElfLibraries();
}

namespace aim::embedding {

int32_t RunProcessShutdown(bool* needs_completion) {
  if (needs_completion != nullptr) *needs_completion = false;
  aim_process::ShutdownSnapshot shutdown{};
  switch (aim_process::inspect_shutdown(&shutdown)) {
    case aim_process::ShutdownBeginResult::kAlreadyComplete:
      return AIM_STATUS_SHUTDOWN_ALREADY_COMPLETED;
    case aim_process::ShutdownBeginResult::kFailed:
      return AIM_STATUS_SHUTDOWN_FAILED;
    case aim_process::ShutdownBeginResult::kNotReady:
      return AIM_STATUS_SHUTDOWN_NOT_READY;
    case aim_process::ShutdownBeginResult::kWrongThread:
      return AIM_STATUS_SHUTDOWN_WRONG_THREAD;
    case aim_process::ShutdownBeginResult::kReady:
      break;
  }

  // Inspect terminal bookkeeping first: missing Runtime after completed
  // teardown must not turn a completed/failed result into perpetual pending.
  if (art::Runtime* runtime = art::Runtime::Current();
      runtime == nullptr || runtime->IsShuttingDownUnsafe())
    return AIM_STATUS_SHUTDOWN_NOT_READY;
  if (shutdown.art_thread == nullptr)
    return AIM_STATUS_SHUTDOWN_NOT_READY;
  const bool input_closed = aim::input::CloseRootKeyIngressAdmission();
  {
    art::ScopedObjectAccess soa(shutdown.art_thread);
    JNIEnv* env = shutdown.art_thread->GetJniEnv();
    // Cut off both independent admissions before any retryable early return.
    // Root Java cleanup may leave an exception pending; native key authority
    // still must revoke before waiting for either callback owner to quiesce.
    const bool root_closed =
        aim::framework::wm::CloseDesktopRootClientAdmission(env);
    const bool decisions_closed =
        aim::framework::wm::CloseRootKeyDecisionAdmission(env);
    // Retire native resources only after the root owners released their locks.
    // unlinkToDeath is not a callback barrier; the provider also counts obituary
    // pins, capture factories and the host metadata release tail.
    aim::binder::CloseNativeBinderEndpointAdmission();
    if (!input_closed || !root_closed || !decisions_closed ||
        !aim::framework::wm::PollDesktopRootClientQuiesced(env) ||
        !aim::framework::wm::PollRootKeyDecisionQuiesced(env))
      return AIM_STATUS_SHUTDOWN_NOT_READY;
  }
  if (!aim::input::PollRootKeyIngressQuiesced())
    return AIM_STATUS_SHUTDOWN_NOT_READY;
  if (!aim::binder::PollNativeBinderEndpointsQuiesced())
    return AIM_STATUS_SHUTDOWN_NOT_READY;
  CloseSurfaceTransactionSubmissionAdmission();
  bool blast_quiesced = false;
  {
    art::ScopedObjectAccess soa(shutdown.art_thread);
    blast_quiesced = aim::window::QuiesceBlastBufferQueues(
        shutdown.art_thread->GetJniEnv());
  }
  const bool submission_quiesced = PollSurfaceTransactionSubmissionQuiesced();
  if (!blast_quiesced || !submission_quiesced)
    return AIM_STATUS_SHUTDOWN_NOT_READY;
  if (!aim::runtime_art::IsVmReadyForShutdown(shutdown.art_thread))
    return AIM_STATUS_SHUTDOWN_NOT_READY;
  if (!aim::binder::NativeBinderWorkersQuiesced()) {
    static bool reported = false;  // shutdown is restricted to the ART owner
    if (!reported) {
      reported = true;
      std::cerr << "ART Darwin shutdown pending: Binder pool lacks joined-worker proof\n";
    }
    return AIM_STATUS_SHUTDOWN_NOT_READY;
  }
  // The session close/quiescence gate is part of the canonical shutdown
  // transaction. Do this before begin_shutdown commits process state, so an
  // admitted callback leaves the caller with a retryable result rather than a
  // half-transitioned ART teardown.
  if (shutdown.graphics_state != nullptr &&
      !aim_graphics::bound_session_quiescent(
          shutdown.graphics_state))
    return AIM_STATUS_SHUTDOWN_NOT_READY;
  aim::input::InputChannelIdentityCatalog* channel_catalog = nullptr;
  try {
    channel_catalog = &aim::input::GetChannelIdentityCatalog();
  } catch (const std::bad_alloc&) {
    // A channel-free process may first initialize this owner during teardown.
    // Never let allocation failure escape the exported C shutdown boundary;
    // process state is still uncommitted and the caller may retry.
    return AIM_STATUS_SHUTDOWN_NOT_READY;
  }
  if (!channel_catalog->CloseAdmission()) return AIM_STATUS_SHUTDOWN_NOT_READY;
  switch (aim_process::begin_shutdown(&shutdown)) {
    case aim_process::ShutdownBeginResult::kReady: break;
    case aim_process::ShutdownBeginResult::kAlreadyComplete:
      return AIM_STATUS_SHUTDOWN_ALREADY_COMPLETED;
    case aim_process::ShutdownBeginResult::kFailed:
      return AIM_STATUS_SHUTDOWN_FAILED;
    case aim_process::ShutdownBeginResult::kWrongThread:
      return AIM_STATUS_SHUTDOWN_WRONG_THREAD;
    case aim_process::ShutdownBeginResult::kNotReady:
      return AIM_STATUS_SHUTDOWN_NOT_READY;
  }
  std::cerr << "ART Darwin shutdown stage=run enter\n";

  JavaVM* java_vm = shutdown.java_vm;
  art::Thread* art_thread = shutdown.art_thread;
  const bool framework_vm_bound = shutdown.framework_vm_bound;
  CHECK(java_vm != nullptr);
  if (art_thread != nullptr) {
    CHECK_EQ(art_thread->GetState(), art::ThreadState::kNative);
    {
      art::ScopedObjectAccess soa(art_thread);
      if (art_thread->IsExceptionPending()) {
        std::cerr << "ART Darwin shutdown: clearing pending exception: "
                  << art_thread->GetException()->Dump() << "\n";
        art_thread->ClearException();
      }
      JNIEnv* env = art_thread->GetJniEnv();
      if (!aim::framework::wm::ClearRootKeyDecisionReferences(env)) {
        aim_process::mark_shutdown_failed();
        return AIM_STATUS_SHUTDOWN_FAILED;
      }
      if (!aim::framework::wm::ClearDesktopRootClientReferences(env)) {
        aim_process::mark_shutdown_failed();
        return AIM_STATUS_SHUTDOWN_FAILED;
      }
      aim_graphics::shutdown(shutdown.graphics_state, env);
      std::cerr << "ART Darwin shutdown stage=graphics complete\n";
      if (art_thread->IsExceptionPending()) {
        std::cerr << "ART Darwin shutdown: global reference cleanup threw: "
                  << art_thread->GetException()->Dump() << "\n";
        art_thread->ClearException();
      }
#if defined(AIM_REAL_GRAPHICS)
      {
        art::ScopedThreadSuspension suspended(art_thread, art::ThreadState::kNative);
        aim::ShutdownFrameworkAsyncWorkers();
      }
#endif
      std::cerr << "ART Darwin shutdown stage=async-workers-joined\n";
      if (!channel_catalog->Clear(env)) {
        aim_process::mark_shutdown_failed();
        return AIM_STATUS_SHUTDOWN_FAILED;
      }
      std::cerr << "ART Darwin shutdown stage=libcore-unload enter\n";
      if (!aim::ShutdownLibcoreNatives()) {
        std::cerr << "ART Darwin shutdown: libcore host state restore failed\n";
        aim_process::mark_shutdown_failed();
        return AIM_STATUS_SHUTDOWN_FAILED;
      }
      std::cerr << "ART Darwin shutdown stage=libcore-unload exit\n";
      std::cerr << "ART Darwin shutdown stage=elf-unload enter\n";
      if (!android::ShutdownElfLibraries()) {
        std::cerr << "ART Darwin shutdown: NativeLoader DSO unload failed\n";
        aim_process::mark_shutdown_failed();
        return AIM_STATUS_SHUTDOWN_FAILED;
      }
      std::cerr << "ART Darwin shutdown stage=elf-unload complete\n";
      if (framework_vm_bound &&
          aim_android_runtime_uninstall(env) != AIM_ANDROID_RUNTIME_OK) {
        std::cerr << "ART Darwin shutdown: AndroidRuntime ownership uninstall failed\n";
        aim_process::mark_shutdown_failed();
        return AIM_STATUS_SHUTDOWN_FAILED;
      }
    }
    CHECK_EQ(art_thread->GetState(), art::ThreadState::kNative);
    // No graphics-state use remains beyond this point. Finalization permits
    // post-ART handle destruction, so publish it only after the last use.
    if (shutdown.graphics_state != nullptr) {
      const int32_t graphics_finalize_status =
          aim_graphics::finalize_bound_session(shutdown.graphics_state);
      if (graphics_finalize_status != 0) {
        std::cerr << "ART Darwin shutdown: graphics session finalization failed status="
                  << graphics_finalize_status << "\n";
        aim_process::mark_shutdown_failed();
        return AIM_STATUS_SHUTDOWN_FAILED;
      }
    }
  }

  if (!aim::runtime_art::DetachAndDestroyVm(java_vm)) {
    aim_process::mark_shutdown_failed();
    return AIM_STATUS_SHUTDOWN_FAILED;
  }
  if (needs_completion != nullptr) *needs_completion = true;
  return 0;
}

int32_t CompleteProcessShutdown() {
  aim::ShutdownIcuCharsetNatives();
  aim::ShutdownFrameworkGraphicsRuntime();
  aim::process::ClearHostServices();
  aim_process::clear_app_dex_files();
  aim_process::mark_shutdown_complete();
  return 0;
}

int32_t PrepareProcessExit() {
  // Android's process teardown asks JavaVMExt to unload every library tracked
  // by System.load before the zygote child exits. This is distinct from the
  // Darwin ELF graph registry, which only owns converted guest images.
  if (art::Runtime* runtime = art::Runtime::Current(); runtime != nullptr) {
    if (art::JavaVMExt* vm = runtime->GetJavaVM(); vm != nullptr) {
      vm->UnloadNativeLibraries();
    }
  }
  return android::ShutdownElfLibraries()
             ? 0
             : AIM_STATUS_SHUTDOWN_FAILED;
}

}  // namespace aim::embedding

extern "C" AIM_EXPORT int32_t aim_shutdown_process() {
  bool needs_completion = false;
  const int32_t status =
      aim::embedding::RunProcessShutdown(&needs_completion);
  return status == 0 && needs_completion
             ? aim::embedding::CompleteProcessShutdown()
             : status;
}

extern "C" AIM_EXPORT int32_t aim_prepare_process_exit() {
  return aim::embedding::PrepareProcessExit();
}
