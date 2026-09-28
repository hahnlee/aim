/*
 * Darwin run-test agent entrypoint.
 *
 * AOSP's libtiagent common_load.cc dispatches dozens of test agents from one
 * DSO.  The compatibility corpus builds an isolated per-test DSO, so retain
 * the identical option grammar and dispatch the one implementation linked
 * into that DSO.  The JVMTI implementation and test callbacks remain the
 * pinned AOSP sources.
 */

#include <cstring>

#include "jni.h"
#include "jvmti_helper.h"
#if defined(AIM_TI_AGENT_901)
#include "901-hello-ti-agent/basics.h"
#endif
#if defined(AIM_TI_AGENT_993)
#include "993-breakpoints-non-debuggable/onload.h"
#endif
#if defined(AIM_TI_AGENT_936)
#include "936-search-onload/search_onload.h"
#endif
#if defined(AIM_TI_AGENT_1919)
#include "1919-vminit-thread-start-timing/vminit.h"
#endif
#include "test_env.h"

namespace art {

#if defined(AIM_TI_AGENT_COMMON_REDEFINE) || \
    defined(AIM_TI_AGENT_COMMON_RETRANSFORM) || \
    defined(AIM_TI_AGENT_COMMON_TRANSFORM) || \
    defined(AIM_TI_AGENT_1919)
namespace common_redefine {
jint OnLoad(JavaVM*, char*, void*);
}
namespace common_retransform {
jint OnLoad(JavaVM*, char*, void*);
}
namespace common_transform {
jint OnLoad(JavaVM*, char*, void*);
}

static char* AimAgentOptions(char* options) {
  if (options == nullptr) return nullptr;
  char* remaining_options = std::strchr(options, ',');
  if (remaining_options == nullptr) return nullptr;
  *remaining_options++ = '\0';
  SetJVM(std::strncmp(remaining_options, "jvm", 3) == 0);
  return remaining_options;
}
#endif

#if defined(AIM_TI_AGENT_936)
extern "C" JNIEXPORT jint JNICALL Agent_OnLoad(JavaVM* vm,
                                                 char* options,
                                                 void* reserved) {
  if (options == nullptr) return -1;
  char* remaining_options = std::strchr(options, ',');
  if (remaining_options == nullptr) return -1;
  *remaining_options++ = '\0';
  if (std::strcmp(options, "936-search-onload") != 0) return -2;
  SetJVM(std::strncmp(remaining_options, "jvm", 3) == 0);
  return Test936SearchOnload::OnLoad(vm, remaining_options, reserved);
}
#elif defined(AIM_TI_AGENT_MINIMAL)
static jint AimMinimalAgentStart(JavaVM* vm, char* options) {
  SetJVM(options != nullptr && std::strncmp(options, "jvm", 3) == 0);
  if (vm->GetEnv(reinterpret_cast<void**>(&jvmti_env), JVMTI_VERSION_1_0) !=
      JNI_OK) {
    return 1;
  }
  SetStandardCapabilities(jvmti_env);
  return JNI_OK;
}

extern "C" JNIEXPORT jint JNICALL Agent_OnLoad(JavaVM* vm,
                                                 char* options,
                                                 void*) {
  return AimMinimalAgentStart(vm, options);
}

// AOSP common_load.cc maps 2031's deferred Agent_OnAttach to MinimalOnLoad.
// Per-test Darwin agents do not carry common_load's global dispatch table, so
// consume the same "test-name,art|jvm" prefix locally before initializing the
// JVMTI environment.
extern "C" JNIEXPORT jint JNICALL Agent_OnAttach(JavaVM* vm,
                                                   char* options,
                                                   void*) {
  if (options == nullptr) return -1;
  char* remaining_options = std::strchr(options, ',');
  if (remaining_options == nullptr) return -1;
  return AimMinimalAgentStart(vm, ++remaining_options);
}
#elif defined(AIM_TI_AGENT_1919)
extern "C" JNIEXPORT jint JNICALL Agent_OnLoad(JavaVM* vm,
                                                 char* options,
                                                 void* reserved) {
  char* remaining_options = AimAgentOptions(options);
  return remaining_options == nullptr
             ? -1
             : Test1919VMInitThreadStart::OnLoad(
                   vm, remaining_options, reserved);
}
#endif

#if defined(AIM_TI_AGENT_COMMON_REDEFINE)
extern "C" JNIEXPORT jint JNICALL Agent_OnLoad(JavaVM* vm,
                                                 char* options,
                                                 void* reserved) {
  char* remaining_options = AimAgentOptions(options);
  return remaining_options == nullptr
             ? -1
             : common_redefine::OnLoad(vm, remaining_options, reserved);
}
#elif defined(AIM_TI_AGENT_COMMON_RETRANSFORM)
extern "C" JNIEXPORT jint JNICALL Agent_OnLoad(JavaVM* vm,
                                                 char* options,
                                                 void* reserved) {
  char* remaining_options = AimAgentOptions(options);
  return remaining_options == nullptr
             ? -1
             : common_retransform::OnLoad(vm, remaining_options, reserved);
}
#elif defined(AIM_TI_AGENT_COMMON_TRANSFORM)
extern "C" JNIEXPORT jint JNICALL Agent_OnLoad(JavaVM* vm,
                                                 char* options,
                                                 void* reserved) {
  char* remaining_options = AimAgentOptions(options);
  return remaining_options == nullptr
             ? -1
             : common_transform::OnLoad(vm, remaining_options, reserved);
}
#endif

#if defined(AIM_TI_AGENT_901)
extern "C" JNIEXPORT jint JNICALL Agent_OnLoad(JavaVM* vm,
                                                 char* options,
                                                 void* reserved) {
  if (options == nullptr) {
    return -1;
  }
  char* remaining_options = std::strchr(options, ',');
  if (remaining_options == nullptr) {
    return -1;
  }
  *remaining_options++ = '\0';
  if (std::strcmp(options, "901-hello-ti-agent") != 0) {
    return -2;
  }
  SetJVM(std::strncmp(remaining_options, "jvm", 3) == 0);
  return Test901HelloTi::OnLoad(vm, remaining_options, reserved);
}
#endif

#if defined(AIM_TI_AGENT_993)
extern "C" JNIEXPORT jint JNICALL Agent_OnAttach(JavaVM* vm,
                                                   char* options,
                                                   void* reserved) {
  if (options == nullptr) {
    return -1;
  }
  char* remaining_options = std::strchr(options, ',');
  if (remaining_options == nullptr) {
    return -1;
  }
  *remaining_options++ = '\0';
  if (std::strcmp(options, "993-non-debuggable") != 0) {
    return -2;
  }
  SetJVM(std::strncmp(remaining_options, "jvm", 3) == 0);
  return Test993BreakpointsNonDebuggable::OnLoad(vm,
                                                  remaining_options,
                                                  reserved);
}
#endif

}  // namespace art
