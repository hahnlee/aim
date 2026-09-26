#include "../../../runtime/framework/system/process_entry.h"
#include "../../../runtime/framework/am/application_binding.h"
#include "../../../runtime/framework/system/system_context.h"
#include "../../../runtime/framework/compat/policy_binding.h"
#include "../../../runtime/framework/pm/installd/installd_jni.h"
#include "../../../runtime/framework/system/host_command_jni.h"
#include <cstdlib>
#include <cstring>
#include <string>

namespace {
int mode;
int served;
bool policy_ready;
bool kernel_ready;
}
extern "C" int darwin_art_install_context_loader(JNIEnv*, jobject loader) {
  return loader == nullptr ? 4 : 0;
}
extern "C" bool darwin_art_surfaceflinger_service_start() { return mode != 3; }
namespace darwin_art::framework::app {
int FinishFrameworkRegistration(JNIEnv*, bool) { return 0; }
}
namespace darwin_art::framework::connectivity {
bool RegisterNetworkPathProvider(JNIEnv*, jclass owner) { return owner != nullptr; }
}
namespace darwin_art::framework::power {
bool RegisterPowerStateProvider(JNIEnv*, jclass owner) { return owner != nullptr; }
bool RegisterBatteryStateProvider(JNIEnv*, jclass owner) { return owner != nullptr; }
}
namespace darwin_art::framework::display {
bool RegisterHostDisplayFacts(JNIEnv*, jclass owner) { return owner != nullptr; }
}
namespace darwin_art::framework::time {
bool RegisterHostTimeZoneProvider(JNIEnv*, jclass owner) { return owner != nullptr; }
}
namespace darwin_art::framework::wm {
bool RegisterActivityLaunchScheduler(JNIEnv*, jclass owner) { return owner != nullptr; }
}
namespace darwin_art::framework::system {
bool StartKernelBinderService(JNIEnv*, jobject service_directory) {
  kernel_ready = service_directory != nullptr && mode != 4;
  return kernel_ready;
}
jobject CreateSystemContext(JNIEnv* env) {
  if (mode == 5 || !kernel_ready) return nullptr;
  jclass type = env->FindClass("java/lang/Object");
  jmethodID ctor = env->GetMethodID(type, "<init>", "()V");
  jobject context = env->NewObject(type, ctor);
  env->DeleteLocalRef(type);
  return context;
}
bool InitializeApplicationSharedMemory(JNIEnv* env) {
  if (mode != 2) return true;
  jclass error = env->FindClass("java/lang/IllegalStateException");
  env->ThrowNew(error, "initialization failed");
  env->DeleteLocalRef(error);
  return false;
}
}
namespace darwin_art::framework::compat {
bool InitializeSystemPolicy(JNIEnv* env, jobject context) {
  if (mode == 6) {
    jclass error = env->FindClass("java/lang/IllegalStateException");
    env->ThrowNew(error, "compat catalog failed");
    env->DeleteLocalRef(error);
    return false;
  }
  policy_ready = context != nullptr;
  return policy_ready;
}
}
namespace darwin_art::framework::am {
bool RegisterActivityManager(JNIEnv*, jclass endpoint) { return endpoint != nullptr; }
}
namespace darwin_art::framework::pm {
bool RegisterDarwinInstalld(JNIEnv*, jclass installd) { return installd != nullptr; }
}
namespace darwin_art::framework::system {
bool RegisterHostCommandService(JNIEnv*, jclass service) { return service != nullptr; }
}
namespace darwin_art {
int ServeBinderServiceEndpoint(JNIEnv*, jobject binder, const char* socket) {
  if (!policy_ready) return 72;
  ++served;
  return binder != nullptr && std::strcmp(socket, "/test/explicit-socket") == 0 ? 23 : 71;
}
}
extern "C" JNIEXPORT jint JNICALL Java_SystemProcessEntryTest_run(
    JNIEnv* env, jclass, jint requested) {
  mode = requested;
  // SYSTEMSERVERCLASSPATH as derive_classpath exported it in the image
  // (the harness writes system/etc/classpath under DARWIN_ART_TEST_IMAGE).
  const char* image = std::getenv("DARWIN_ART_TEST_IMAGE");
  std::string classpath;
  if (!darwin_art::framework::system::BuildSystemClassPath(
          image, "/support/code.dex", &classpath) ||
      classpath != std::string(image) + "/system/framework/services.jar:" + image +
                       "/apex/com.android.art/javalib/service-art.jar:/support/code.dex")
    return 74;
  const std::string previous = classpath;
  const char* invalid_paths[] = {nullptr, "relative", "/", "/image:extra", "/image/.."};
  for (const char* invalid : invalid_paths) {
    if (darwin_art::framework::system::BuildSystemClassPath(
            invalid, "/support/code.dex", &classpath) || classpath != previous) return 75;
  }
  if (darwin_art::framework::system::BuildSystemClassPath(
          image, "/support:/extra", &classpath) || classpath != previous) return 76;
  served = 0;
  policy_ready = false;
  kernel_ready = false;
  int result = darwin_art::framework::system::RunSystemProcess(
      env, mode == 1 ? nullptr : "/test/explicit-socket");
  if ((mode == 0 && served != 1) || (mode != 0 && served != 0)) return 72;
  return result;
}
