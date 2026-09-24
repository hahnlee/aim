#include "desktop_root_geometry_jni.h"

#include "../../../compat/window/root_geometry.h"

#include <dispatch/dispatch.h>
#include <pthread.h>

#include <atomic>
#include <mutex>

namespace darwin_art::framework::wm {
namespace {

std::mutex g_client_lock;
jclass g_client_class = nullptr;
jmethodID g_register_method = nullptr;
std::atomic<bool> g_registered{false};

jclass LoadClientClass(JNIEnv* env) {
  jclass loader_class = env->FindClass("java/lang/ClassLoader");
  jmethodID get_system_loader = loader_class == nullptr
      ? nullptr
      : env->GetStaticMethodID(loader_class, "getSystemClassLoader",
                               "()Ljava/lang/ClassLoader;");
  jobject loader = get_system_loader == nullptr
      ? nullptr
      : env->CallStaticObjectMethod(loader_class, get_system_loader);
  jmethodID load_class = loader_class == nullptr
      ? nullptr
      : env->GetMethodID(loader_class, "loadClass",
                         "(Ljava/lang/String;)Ljava/lang/Class;");
  jstring name = env->NewStringUTF("dev.darwinart.runtime.wm.DesktopRootGeometryClient");
  jclass client = loader == nullptr || load_class == nullptr || name == nullptr
      ? nullptr
      : static_cast<jclass>(env->CallObjectMethod(loader, load_class, name));
  if (name != nullptr) env->DeleteLocalRef(name);
  if (loader != nullptr) env->DeleteLocalRef(loader);
  if (loader_class != nullptr) env->DeleteLocalRef(loader_class);
  return client;
}

jboolean HasRoot(JNIEnv*, jclass) {
  return window::ProcessHasVisibleRoot() ? JNI_TRUE : JNI_FALSE;
}

// Runs on the dedicated Java reporter thread; blocks outside every Java lock.
jlongArray AwaitReport(JNIEnv* env, jclass, jlong after_serial) {
  window::RootGeometryReport report;
  if (!window::RootGeometryReports::Process().Await(
          static_cast<uint64_t>(after_serial), &report)) {
    return nullptr;
  }
  jlongArray result = env->NewLongArray(3);
  if (result == nullptr) return nullptr;
  const jlong values[3] = {static_cast<jlong>(report.serial),
                           static_cast<jlong>(report.points_width),
                           static_cast<jlong>(report.points_height)};
  env->SetLongArrayRegion(result, 0, 3, values);
  return result;
}

struct ApplyRequest {
  window::RootGeometryPublication publication;
  window::RootGeometryStatus status = window::RootGeometryStatus::kInvalid;
};

void ApplyOnMain(void* context) {
  auto* request = static_cast<ApplyRequest*>(context);
  request->status = window::ApplyProcessRootGeometry(request->publication);
}

// Binder thread: no Java monitor is held; the system side sent this one-way,
// so waiting for the AppKit main thread cannot block the geometry owner.
jint Apply(JNIEnv*, jclass, jlong revision, jlong host_serial, jint android_width,
           jint android_height, jint points_width, jint points_height) {
  if (revision < 0 || host_serial < 0 || android_width <= 0 || android_height <= 0 ||
      points_width <= 0 || points_height <= 0 || pthread_main_np() != 0) {
    return static_cast<jint>(window::RootGeometryStatus::kInvalid);
  }
  ApplyRequest request;
  request.publication.revision = static_cast<uint64_t>(revision);
  request.publication.host_serial = static_cast<uint64_t>(host_serial);
  request.publication.android_width = static_cast<uint32_t>(android_width);
  request.publication.android_height = static_cast<uint32_t>(android_height);
  request.publication.points_width = static_cast<uint32_t>(points_width);
  request.publication.points_height = static_cast<uint32_t>(points_height);
  dispatch_sync_f(dispatch_get_main_queue(), &request, &ApplyOnMain);
  return static_cast<jint>(request.status);
}

}  // namespace

bool RegisterDesktopRootGeometryClient(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  std::lock_guard<std::mutex> guard(g_client_lock);
  if (g_client_class != nullptr) return true;
  jclass client = LoadClientClass(env);
  if (client == nullptr) return false;
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeHasRoot"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(&HasRoot)},
      {const_cast<char*>("nativeAwaitReport"), const_cast<char*>("(J)[J"),
       reinterpret_cast<void*>(&AwaitReport)},
      {const_cast<char*>("nativeApply"), const_cast<char*>("(JJIIII)I"),
       reinterpret_cast<void*>(&Apply)},
  };
  const bool registered = env->RegisterNatives(client, methods, 3) == JNI_OK &&
      !env->ExceptionCheck();
  jmethodID registration = registered
      ? env->GetStaticMethodID(client, "register", "()V") : nullptr;
  jclass global = registration == nullptr || env->ExceptionCheck()
      ? nullptr : static_cast<jclass>(env->NewGlobalRef(client));
  env->DeleteLocalRef(client);
  if (global == nullptr || env->ExceptionCheck()) return false;
  g_client_class = global;
  g_register_method = registration;
  return true;
}

bool EnsureDesktopRootGeometryClient(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  if (g_registered.load(std::memory_order_acquire)) return true;
  jclass activity_thread = env->FindClass("android/app/ActivityThread");
  jmethodID current = activity_thread == nullptr
      ? nullptr
      : env->GetStaticMethodID(activity_thread, "currentActivityThread",
                               "()Landroid/app/ActivityThread;");
  jobject thread = current == nullptr
      ? nullptr : env->CallStaticObjectMethod(activity_thread, current);
  env->DeleteLocalRef(activity_thread);
  if (env->ExceptionCheck()) return false;
  if (thread == nullptr) return true;  // Not yet an attached application.
  env->DeleteLocalRef(thread);
  jclass client = nullptr;
  jmethodID registration = nullptr;
  {
    std::lock_guard<std::mutex> guard(g_client_lock);
    if (g_client_class != nullptr) {
      client = static_cast<jclass>(env->NewLocalRef(g_client_class));
      registration = g_register_method;
    }
  }
  if (client == nullptr || registration == nullptr) return false;
  env->CallStaticVoidMethod(client, registration);
  env->DeleteLocalRef(client);
  if (env->ExceptionCheck()) return false;
  g_registered.store(true, std::memory_order_release);
  return true;
}

}  // namespace darwin_art::framework::wm
