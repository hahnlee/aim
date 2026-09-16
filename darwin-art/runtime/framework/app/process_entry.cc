#include "process_entry.h"

#include "java_exception_report.h"
#include "kernel_binder_client.h"
#include "main_loop.h"
#include "process_registration.h"

#include <iostream>

namespace darwin_art::framework::app {
namespace {
const char* ExitName(ApplicationMainLoopExit exit) {
  switch (exit) {
    case ApplicationMainLoopExit::kInvalidEnvironment: return "invalid-environment";
    case ApplicationMainLoopExit::kLooperLookup: return "looper-lookup";
    case ApplicationMainLoopExit::kWrongLooper: return "wrong-looper";
    case ApplicationMainLoopExit::kActivityThreadLookup: return "activity-thread-lookup";
    case ApplicationMainLoopExit::kAlreadyAttached: return "already-attached";
    case ApplicationMainLoopExit::kActivityThreadConstruction:
      return "activity-thread-construction";
    case ApplicationMainLoopExit::kActivityManagerAttachment:
      return "activity-manager-attachment";
    case ApplicationMainLoopExit::kLooperDispatch: return "looper-dispatch";
  }
  return "unknown";
}
}

int RunApplicationProcess(JNIEnv* env, art::Thread* self) {
  if (env == nullptr || self == nullptr) return 4;
  // ActivityThread.main() owns Looper preparation and all framework process
  // initialization for an application process.
  const int registration_status = FinishFrameworkRegistration(env, false);
  if (registration_status != 0) return registration_status;
  if (!StartApplicationBinderPool()) return 28;
  const auto exit = RunPreparedApplicationMainLoop(env);
  std::cerr << "ART Android application main Looper failed at " << ExitName(exit) << ":\n";
  if (env->ExceptionCheck()) {
    ReportPendingJavaException(env);
  } else {
    std::cerr << "no pending Java exception\n";
  }
  // A normally running ActivityThread does not return. If its Looper exits,
  // main_loop has either preserved the Java exception or installed one.
  return 27;
}

}  // namespace darwin_art::framework::app
