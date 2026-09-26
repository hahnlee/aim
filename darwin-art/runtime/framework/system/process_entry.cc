#include "process_entry.h"

#include <cstdlib>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string_view>
#include <vector>

#include "../../art/system_class_loader.h"
#include "class_path_contract.h"

#include "application_shared_memory.h"
#include "system_context.h"
#include "../compat/policy_binding.h"
#include "kernel_binder_service.h"
#include "../connectivity/network_provider_jni.h"
#include "../power/power_state_jni.h"
#include "../pm/installd/installd_jni.h"
#include "host_command_jni.h"
#include "../app/process_registration.h"
#include "../app/java_exception_report.h"
#include "../am/application_binding.h"
#include "../wm/activity_launch_transaction.h"
#include "binder/service_endpoint.h"
#include "surfaceflinger/service_darwin.h"

namespace darwin_art::framework::system {
namespace {
// One `export NAME a:b:c` line of derive_classpath's environment file for
// this image (system/etc/classpath), each entry a validated device path.
// Empty when the file does not export NAME; false when it is malformed.
bool ReadClasspathExport(const char* image_root, std::string_view name,
                         std::vector<std::string>* entries) {
  if (!IsValidSystemPath(image_root)) return false;
  std::ifstream environment(std::string(image_root) + "/system/etc/classpath");
  const std::string prefix = "export " + std::string(name) + " ";
  bool exported = false;
  for (std::string line; std::getline(environment, line);) {
    if (!line.starts_with(prefix)) continue;
    if (exported) return false;  // exported twice
    exported = true;
    std::stringstream values(line.substr(prefix.size()));
    for (std::string entry; std::getline(values, entry, ':');) {
      if (!IsValidSystemPath(entry.c_str()) || !entry.ends_with(".jar")) return false;
      entries->push_back(entry);
    }
  }
  return true;
}
}  // namespace

// SYSTEMSERVERCLASSPATH as derive_classpath exported it for this image,
// resolved under the image root, then the support DEX.
bool BuildSystemClassPath(const char* image_root, const char* support_dex,
                          std::string* result) {
  std::vector<std::string> entries;
  if (result == nullptr || !IsValidSystemPath(support_dex) ||
      !ReadClasspathExport(image_root, "SYSTEMSERVERCLASSPATH", &entries) || entries.empty())
    return false;
  std::string classpath;
  for (const std::string& entry : entries) classpath += std::string(image_root) + entry + ":";
  *result = classpath + support_dex;
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

// ZygoteInit.prefetchStandaloneSystemServerJars, which the zygote's native
// system-server specialization runs: a class loader per
// STANDALONE_SYSTEMSERVER_JARS entry (by device path), parented by the system
// server class loader. SystemServiceManager.startServiceFromJar only uses
// loaders created here. As in AOSP, a jar that fails to load is logged.
bool PrefetchStandaloneSystemServerJars(JNIEnv* env, jobject loader) {
  std::vector<std::string> jars;
  if (!ReadClasspathExport(std::getenv("DARWIN_ART_ANDROID_FILESYSTEM_ROOT"),
                           "STANDALONE_SYSTEMSERVER_JARS", &jars)) {
    return false;
  }
  jclass factory = env->FindClass("com/android/internal/os/SystemServerClassLoaderFactory");
  jmethodID create = factory == nullptr ? nullptr : env->GetStaticMethodID(
      factory, "createClassLoader",
      "(Ljava/lang/String;Ljava/lang/ClassLoader;)Ldalvik/system/PathClassLoader;");
  if (create == nullptr) return false;
  for (const std::string& jar : jars) {
    jstring path = env->NewStringUTF(jar.c_str());
    if (path == nullptr) return false;
    jobject created = env->CallStaticObjectMethod(factory, create, path, loader);
    env->DeleteLocalRef(path);
    if (env->ExceptionCheck()) {
      std::cerr << "Failed to prefetch standalone system server jar " << jar << "\n";
      env->ExceptionDescribe();
      env->ExceptionClear();
    }
    if (created != nullptr) env->DeleteLocalRef(created);
  }
  env->DeleteLocalRef(factory);
  return true;
}

int Run(JNIEnv* env, const char* socket_path) {
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
  // ZygoteInit.zygoteInit runs these before SystemServer.main: log streams,
  // the FATAL EXCEPTION IN SYSTEM PROCESS pre-handler and the kill-on-crash
  // default handler, so a crashed service thread takes the process down.
  jclass runtime_init = env->FindClass("com/android/internal/os/RuntimeInit");
  if (runtime_init == nullptr) return 70;
  for (const char* name : {"redirectLogStreams", "commonInit"}) {
    jmethodID method = env->GetStaticMethodID(runtime_init, name, "()V");
    if (method == nullptr) return 70;
    env->CallStaticVoidMethod(runtime_init, method);
    if (env->ExceptionCheck()) return 70;
  }
  env->DeleteLocalRef(runtime_init);

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

  jclass battery_state = LoadClass(
      env, loader, load, "dev.darwinart.runtime.power.DarwinBatteryStateProvider");
  if (battery_state == nullptr || env->ExceptionCheck() ||
      !darwin_art::framework::power::RegisterBatteryStateProvider(env,
                                                                  battery_state)) {
    return 70;
  }

  jclass installd = LoadClass(
      env, loader, load, "dev.darwinart.runtime.pm.installd.DarwinInstalld");
  if (installd == nullptr || env->ExceptionCheck() ||
      !darwin_art::framework::pm::RegisterDarwinInstalld(env, installd)) {
    return 70;
  }

  jclass host_commands = LoadClass(
      env, loader, load, "dev.darwinart.runtime.system.HostCommandService");
  if (host_commands == nullptr || env->ExceptionCheck() ||
      !RegisterHostCommandService(env, host_commands)) {
    return 70;
  }

  jclass activity = LoadClass(
      env, loader, load, "dev.darwinart.runtime.am.ActivityManagerEndpoint");
  if (activity == nullptr || env->ExceptionCheck()) return 70;
  if (!am::RegisterActivityManager(env, activity)) return 70;
  jclass activity_task = LoadClass(
      env, loader, load, "dev.darwinart.runtime.wm.ActivityTaskManagerEndpoint");
  if (activity_task == nullptr || env->ExceptionCheck() ||
      !wm::RegisterActivityLaunchScheduler(env, activity_task)) {
    return 70;
  }
  // The runtime's own service endpoints; the AOSP services started below
  // publish theirs into the same directory.
  jclass server = LoadClass(env, loader, load, "dev.darwinart.system.DarwinSystemServer");
  if (server == nullptr || env->ExceptionCheck()) return 70;
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
  // AOSP SystemServer's package stack: PackageManagerService and its owners
  // (ADR 0009). The profile daemon migrated the install ledger into PMS
  // Settings before this process started.
  if (!PrefetchStandaloneSystemServerJars(env, loader)) return 70;
  jclass bootstrap = LoadClass(
      env, loader, load, "dev.darwinart.runtime.system.SystemServerBootstrap");
  jmethodID start = bootstrap == nullptr ? nullptr : env->GetStaticMethodID(
      bootstrap, "startBootstrapServices", "(Landroid/content/Context;)V");
  if (start == nullptr || env->ExceptionCheck()) return 70;
  env->CallStaticVoidMethod(bootstrap, start, system_context);
  if (env->ExceptionCheck()) return 70;
  // adb's shell role for the host: `cmd` against the services above.
  jmethodID start_commands = env->GetStaticMethodID(
      host_commands, "start", "(Ldev/darwinart/runtime/system/ServiceDirectory;)V");
  if (start_commands == nullptr || env->ExceptionCheck()) return 70;
  env->CallStaticVoidMethod(host_commands, start_commands, directory);
  if (env->ExceptionCheck()) return 70;
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

int RunSystemProcess(JNIEnv* env, const char* socket_path) {
  if (env == nullptr || socket_path == nullptr || *socket_path == '\0' ||
      env->ExceptionCheck()) return 70;
  if (env->PushLocalFrame(16) != JNI_OK) return 70;
  const int status = Run(env, socket_path);
  if (status != 0 && env->ExceptionCheck()) app::ReportPendingJavaException(env);
  // Preserve failures for the process boundary; never clear a Java exception
  // and continue with a partially initialized directory.
  env->PopLocalFrame(nullptr);
  return status;
}

}  // namespace darwin_art::framework::system
