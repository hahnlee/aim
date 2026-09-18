#include "process_entry.h"

#include "../../art/system_class_loader.h"
#include "class_path_contract.h"

#include "application_shared_memory.h"
#include "system_context.h"
#include "../compat/policy_binding.h"
#include "kernel_binder_service.h"
#include "../connectivity/network_provider_jni.h"
#include "../power/power_state_jni.h"
#include "../app/process_registration.h"
#include "../app/java_exception_report.h"
#include "../am/application_binding.h"
#include "../wm/activity_launch_transaction.h"
#include "binder/service_endpoint.h"
#include "surfaceflinger/service_darwin.h"

namespace darwin_art::framework::system {
bool BuildSystemClassPath(const char* image_root, const char* support_dex,
                          std::string* result) {
  if (result == nullptr || !IsValidSystemPath(image_root) ||
      !IsValidSystemPath(support_dex))
    return false;
  *result = std::string(image_root) + "/system/framework/services.jar:" + support_dex;
  return true;
}

namespace {

jclass LoadClass(JNIEnv* env, jobject loader, jmethodID load, const char* name) {
  jstring text = env->NewStringUTF(name);
  if (text == nullptr) return nullptr;
  jobject result = env->CallObjectMethod(loader, load, text);
  env->DeleteLocalRef(text);
  return static_cast<jclass>(result);
}

int Run(JNIEnv* env, const char* socket_path, InstalledRecordResolver resolver) {
  // RuntimeArgumentMap::ClassPath owns this canonical process loader. Do not
  // construct another loader or discover ProbeActivity/ProbeView to obtain it.
  jclass loaders = env->FindClass("java/lang/ClassLoader");
  if (loaders == nullptr) return 70;
  jmethodID system = env->GetStaticMethodID(
      loaders, "getSystemClassLoader", "()Ljava/lang/ClassLoader;");
  if (system == nullptr) return 70;
  jobject loader = env->CallStaticObjectMethod(loaders, system);
  if (loader == nullptr || env->ExceptionCheck()) return 70;
  jmethodID load = env->GetMethodID(
      loaders, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;");
  if (load == nullptr) return 70;
  if (darwin_art_install_context_loader(env, loader) != 0) return 70;
  // Prepare the service endpoints' main Looper before creating the genuine
  // system ActivityThread/Context later in the bootstrap sequence.
  const int registration = app::FinishFrameworkRegistration(env, true);
  if (registration != 0) return registration;
  if (!InitializeApplicationSharedMemory(env)) return 70;

  jclass network_path = LoadClass(
      env, loader, load, "dev.darwinart.runtime.connectivity.NetworkPathProvider");
  if (network_path == nullptr || env->ExceptionCheck() ||
      !darwin_art::framework::connectivity::RegisterNetworkPathProvider(env, network_path)) {
    return 70;
  }

  jclass power_state = LoadClass(
      env, loader, load, "dev.darwinart.runtime.power.DarwinPowerStateProvider");
  if (power_state == nullptr || env->ExceptionCheck() ||
      !darwin_art::framework::power::RegisterPowerStateProvider(env,
                                                                power_state)) {
    return 70;
  }

  jclass activity = LoadClass(
      env, loader, load, "dev.darwinart.runtime.am.ActivityManagerEndpoint");
  if (activity == nullptr || env->ExceptionCheck()) return 70;
  if (!am::RegisterActivityManager(env, activity, resolver)) return 70;
  jclass activity_task = LoadClass(
      env, loader, load, "dev.darwinart.runtime.wm.ActivityTaskManagerEndpoint");
  if (activity_task == nullptr || env->ExceptionCheck() ||
      !wm::RegisterActivityLaunchScheduler(env, activity_task)) {
    return 70;
  }
  // Existing service directory remains migration debt; this entry does not
  // claim to be AOSP SystemServer or to initialize original SystemConfig yet.
  jclass server = LoadClass(env, loader, load, "dev.darwinart.system.DarwinSystemServer");
  if (server == nullptr || env->ExceptionCheck()) return 70;
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeResolvePackage"),
       const_cast<char*>("(Ljava/lang/String;)Ljava/lang/String;"),
       reinterpret_cast<void*>(resolver)},
  };
  if (env->RegisterNatives(server, methods, 1) != JNI_OK) return 70;
  jmethodID create = env->GetStaticMethodID(
      server, "createServiceDirectory", "()Landroid/os/Binder;");
  if (create == nullptr) return 70;
  jobject directory = env->CallStaticObjectMethod(server, create);
  if (directory == nullptr || env->ExceptionCheck()) return 70;
  // AOSP system Context resolves DisplayManager while constructing its real
  // Resources. Internal handle-zero lookup must exist before systemMain;
  // ServiceDirectory admits only the exact system PID during this phase.
  if (!StartKernelBinderService(env, directory)) return 70;
  jobject system_context = CreateSystemContext(env);
  if (system_context == nullptr || env->ExceptionCheck() ||
      !compat::InitializeSystemPolicy(env, system_context)) return 70;
  jclass directory_type = env->GetObjectClass(directory);
  jmethodID publish = directory_type == nullptr ? nullptr : env->GetMethodID(
      directory_type, "publishApplicationLookups", "()V");
  if (publish == nullptr || env->ExceptionCheck()) return 70;
  env->CallVoidMethod(directory, publish);
  if (env->ExceptionCheck()) return 70;
  if (!darwin_art_surfaceflinger_service_start()) return 70;
  return darwin_art::ServeBinderServiceEndpoint(env, directory, socket_path);
}

}  // namespace

int RunSystemProcess(JNIEnv* env, const char* socket_path,
                     InstalledRecordResolver resolver) {
  if (env == nullptr || socket_path == nullptr || *socket_path == '\0' ||
      resolver == nullptr || env->ExceptionCheck()) return 70;
  if (env->PushLocalFrame(16) != JNI_OK) return 70;
  const int status = Run(env, socket_path, resolver);
  if (status != 0 && env->ExceptionCheck()) app::ReportPendingJavaException(env);
  // Preserve failures for the process boundary; never clear a Java exception
  // and continue with a partially initialized directory.
  env->PopLocalFrame(nullptr);
  return status;
}

}  // namespace darwin_art::framework::system
