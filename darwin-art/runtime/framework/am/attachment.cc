#include "application_binding.h"
#include "process_launch.h"
#include "connection_death_jni.h"
#include "../../../compat/binder/process_registry.h"
#include "../wm/activity_launch_transaction.h"
#include <cstdlib>
#include <iostream>
#include <sys/types.h>

// This runtime unit links the original libbinder object code but intentionally
// does not inherit its private build include tree. Keep the boundary limited to
// the three exported IPCThreadState methods used by AOSP Binder identity.
namespace android {
class IPCThreadState {
 public:
  static IPCThreadState* self();
  pid_t getCallingPid() const;
  uid_t getCallingUid() const;
};
}  // namespace android

namespace darwin_art::framework::am {
namespace {
PackageResolver resolve_package = nullptr;
void Error(JNIEnv* env, const char* message) {
  if (env->ExceptionCheck()) return;
  jclass type = env->FindClass("java/lang/IllegalStateException");
  if (type != nullptr) env->ThrowNew(type, message);
  env->DeleteLocalRef(type);
}
// Resolve the authenticated process and installed application independently
// of scheduling bindApplication. AMS publishes this identity before the app
// can execute Application.attachBaseContext and call back into system services.
bool ResolveApplication(JNIEnv* env, jint expected_uid, jstring* package,
                        jstring* record, jobject* info) {
  android::IPCThreadState* thread = android::IPCThreadState::self();
  const int32_t caller_pid = thread->getCallingPid();
  const int32_t caller_uid = thread->getCallingUid();
  DarwinArtRegisteredProcessIdentity registered{};
  if (caller_pid <= 0 || caller_uid < 0 || expected_uid < 0 ||
      caller_uid != expected_uid || resolve_package == nullptr ||
      !darwin_art_runtime_registered_process_identity(caller_pid, &registered) ||
      registered.uid != caller_uid) {
    Error(env, "Application attachment requires a registered Binder caller");
    return false;
  }
  *package = env->NewStringUTF(registered.package);
  *record = *package == nullptr ? nullptr : resolve_package(env, nullptr, *package);
  if (*record == nullptr || env->ExceptionCheck()) return false;
  // ActivityManagerService binds with the ApplicationInfo that
  // ActivityTaskSupervisor.resolveIntent generated under STOCK_PM_FLAGS
  // (GET_SHARED_LIBRARY_FILES), so metaData is absent; apps request it with
  // PackageManager.getApplicationInfo(..., GET_META_DATA).
  constexpr jlong kStockPmFlags = 0x400;
  jclass mapper = env->FindClass("dev/darwinart/runtime/pm/InstalledPackageInfos");
  jmethodID map = mapper == nullptr ? nullptr : env->GetStaticMethodID(mapper, "applicationInfo",
      "(Ljava/lang/String;Ljava/lang/String;J)Landroid/content/pm/ApplicationInfo;");
  *info = map == nullptr ? nullptr
                        : env->CallStaticObjectMethod(mapper, map, *package, *record,
                                                      kStockPmFlags);
  if (*info == nullptr || env->ExceptionCheck()) return false;
  jclass info_type = env->GetObjectClass(*info);
  jfieldID uid = env->GetFieldID(info_type, "uid", "I");
  const jint installed_uid = uid == nullptr ? -1 : env->GetIntField(*info, uid);
  const bool isolated_uid = registered.uid >= 99000 && registered.uid <= 99999;
  return uid != nullptr && !env->ExceptionCheck() && installed_uid >= 0
      && (isolated_uid || installed_uid == registered.uid);
}

jstring ResolveAttachedPackage(JNIEnv* env, jclass, jint expected_uid) {
  if (env->PushLocalFrame(32) < 0) return nullptr;
  jstring package = nullptr;
  jstring record = nullptr;
  jobject info = nullptr;
  if (!ResolveApplication(env, expected_uid, &package, &record, &info)) {
    Error(env, "Registered process has no matching installed application");
    package = nullptr;
  }
  return static_cast<jstring>(env->PopLocalFrame(package));
}

jstring Attach(JNIEnv* env, jclass, jobject application, jlong,
               jstring reserved_process_name, jint expected_uid) {
  const bool debug = std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr;
  if (debug) std::cerr << "ART AMS: attachApplication entered\n";
  if (env->PushLocalFrame(64) < 0) return nullptr;
  jstring attached_package = nullptr;
  auto bind = [&]() -> bool {
    jstring package = nullptr;
    jstring record = nullptr;
    jobject info = nullptr;
    if (!ResolveApplication(env, expected_uid, &package, &record, &info)) return false;
    attached_package = package;
    jclass stub = env->FindClass("android/app/IApplicationThread$Stub");
    jmethodID as_interface = stub == nullptr ? nullptr : env->GetStaticMethodID(stub,
        "asInterface", "(Landroid/os/IBinder;)Landroid/app/IApplicationThread;");
    jobject endpoint = as_interface == nullptr ? nullptr
        : env->CallStaticObjectMethod(stub, as_interface, application);
    jclass resource_type = env->FindClass("android/content/res/Resources");
    jmethodID get_system = resource_type == nullptr ? nullptr : env->GetStaticMethodID(
        resource_type, "getSystem", "()Landroid/content/res/Resources;");
    jobject resources = get_system == nullptr ? nullptr : env->CallStaticObjectMethod(resource_type, get_system);
    if (endpoint == nullptr || resources == nullptr || env->ExceptionCheck()) return false;
    jstring process_name = reserved_process_name == nullptr ? package
                                                            : reserved_process_name;
    // ContentProviderHelper.generateApplicationProvidersLocked: the
    // providers declared for this process, from the parsed package.
    jclass infos = env->FindClass("dev/darwinart/runtime/pm/InstalledPackageInfos");
    jmethodID process_providers = infos == nullptr ? nullptr : env->GetStaticMethodID(
        infos, "processProviders",
        "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/util/List;");
    jobject providers = process_providers == nullptr ? nullptr
        : env->CallStaticObjectMethod(infos, process_providers, package, record, process_name);
    if (providers == nullptr || env->ExceptionCheck()) return false;
    const bool dispatched = DispatchApplicationBinding(
        env, endpoint, info, resources, process_name, providers);
    if (debug) {
      std::cerr << "ART AMS: bindApplication dispatched=" << dispatched
                << " exception=" << env->ExceptionCheck() << "\n";
    }
    return dispatched;
  };
  if (!bind()) Error(env, "System application binding could not be dispatched");
  jthrowable failure = env->ExceptionOccurred();
  if (failure != nullptr) env->ExceptionClear();
  if (failure != nullptr) {
    if (debug) {
      env->Throw(failure);
      env->ExceptionDescribe();
      if (env->ExceptionCheck()) env->ExceptionClear();
    }
    env->Throw(failure);
  }
  if (debug) {
    std::cerr << "ART AMS: attachApplication returning exception="
              << env->ExceptionCheck() << "\n";
  }
  return static_cast<jstring>(env->PopLocalFrame(attached_package));
}
void Launch(JNIEnv* env, jclass, jobject application, jstring package,
            jstring record) {
  if (!darwin_art::framework::wm::ScheduleActivityLaunch(env, application,
                                                          package, record)) {
    Error(env, "System activity launch transaction could not be dispatched");
  }
}
}
bool RegisterActivityManager(JNIEnv* env, jclass endpoint, PackageResolver resolver) {
  if (endpoint == nullptr || resolver == nullptr || env->ExceptionCheck()) return false;
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeResolveAttachedPackage"),
       const_cast<char*>("(I)Ljava/lang/String;"),
       reinterpret_cast<void*>(&ResolveAttachedPackage)},
      {const_cast<char*>("nativeAttach"),
       const_cast<char*>("(Landroid/os/IBinder;JLjava/lang/String;I)Ljava/lang/String;"),
       reinterpret_cast<void*>(&Attach)},
      {const_cast<char*>("nativeLaunch"),
       const_cast<char*>("(Landroid/os/IBinder;Ljava/lang/String;Ljava/lang/String;)V"),
       reinterpret_cast<void*>(&Launch)}};
  if (env->RegisterNatives(endpoint, methods, 3) != JNI_OK) return false;
  if (!RegisterProcessLauncher(env)) return false;
  if (!RegisterConnectionDeathResources(env)) return false;
  resolve_package = resolver;
  return true;
}
}
