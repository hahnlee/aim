#include "runtime_shutdown_probe.h"

#include <iostream>

#include "darwin_art/darwin_art.h"
#include "../runtime/art/process_state.h"
#include "../runtime/embedding/process_shutdown.h"
#include "../runtime/embedding/graphics_state.h"
#include "graphics_fixture_state.h"
#include "runtime_acceptance_state.h"
#include "runtime_frame_probe.h"
#include "elf_fixture_journal.h"
#include "runtime.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"


namespace darwin_art_process {
namespace {

void ClearFixtureGraphicsStateBeforeVmTeardown() {
  if (art::Runtime* runtime = art::Runtime::Current();
      runtime == nullptr || runtime->IsShuttingDownUnsafe()) return;
  auto* state = darwin_art_process::graphics_state_for_callback();
  auto* thread = art::Thread::Current();
  if (state == nullptr || thread == nullptr) return;
  art::ScopedObjectAccess soa(thread);
  // The callback lookup validates the ART owner thread while the process is
  // still running. Clear the whole fixture map here: a failed fixture can
  // have created a second sidecar before returning to the host.
  darwin_art_graphics_fixture::ClearAllProbeCanvasState(
      thread->GetJniEnv());
}

int32_t RunFixtureShutdownChecks(const ShutdownState& state) {
  // Internal reference-count/DNS retirement assertions belong to standalone
  // production-TU component tests. This separate client observes actual VM
  // shutdown status and its external guest-DSO lifecycle journal, not private
  // product singleton counters.
  if (state.apk_elf_loaded) {
    std::cout << "ART Android APK ELF: apk-sha256=" << state.apk_sha256
              << " root-sha256=" << state.apk_root_sha256
              << " graph=root+child+grandchild load=JavaVMExt+NativeBridge "
                 "unload=shutdown-completed\n"
              << std::flush;
  }
  if (state.direct_apk_loaded) {
    std::cout << "ART Android direct APK ELF: source=readonly-fd-slices "
                 "copy=0 extract=0 alignment=16384 graph=root+child+grandchild "
                 "load=JavaVMExt+NativeBridge JNI_OnLoad=0x00010006 "
                 "unload=shutdown-completed authority=isolated-process\n"
              << std::flush;
  }
  if (std::getenv("DARWIN_ART_ANDROID_ELF_JNI_FIXTURE") != nullptr &&
      darwin_art_elf_probe::ReadJournal() != "1234567") {
    std::cerr << "ART Darwin shutdown: ELF JNI graph finalizer order failed, status="
              << darwin_art_elf_probe::ReadJournal() << "\n";
    return DARWIN_ART_STATUS_SHUTDOWN_FAILED;
  }
  return 0;
}

}  // namespace

int32_t run_shutdown(const ShutdownState& state) {
  bool requires_probe_checks = false;
  // The production shutdown owner intentionally knows nothing about fixture
  // JNI references, RenderNodes, or surfaces. Release those objects while
  // ART/JNI are still valid, before the product VM teardown begins.
  ClearFixtureGraphicsStateBeforeVmTeardown();
  const int32_t production_status =
      darwin_art::embedding::RunProcessShutdown(&requires_probe_checks);
  if (production_status != 0 || !requires_probe_checks) {
    return production_status;
  }

  // Frame callback state is probe-owned and is intentionally reset only after
  // VM destruction, immediately before fixture-only observations.
  darwin_art_frame_probe::reset();
  const int32_t fixture_status = RunFixtureShutdownChecks(state);
  const int32_t completion_status = darwin_art::embedding::CompleteProcessShutdown();
  if (fixture_status != 0) {
    return fixture_status;
  }
  return completion_status;
}

}  // namespace darwin_art_process

extern "C" DARWIN_ART_EXPORT int32_t darwin_art_shutdown_fixture() {
  const auto acceptance = darwin_art_acceptance::snapshot();
  darwin_art_process::ShutdownState state;
  state.network_elf_loaded = acceptance.network_elf_loaded;
  state.apk_elf_loaded = acceptance.apk_elf_loaded;
  state.direct_apk_loaded = acceptance.direct_apk_loaded;
  state.apk_sha256 = acceptance.apk_sha256;
  state.apk_root_sha256 = acceptance.apk_root_sha256;
  return darwin_art_process::run_shutdown(state);
}
