#include "context_manager.h"
#include "../darwin_binder_wire.h"
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
#include "android_util_Binder.h"
#include <binder/ProcessState.h>
#endif

#include <cstdlib>
#include <mutex>

namespace darwin_art {
namespace {
std::mutex context_mutex;
jobject context = nullptr;

void ThrowConnectionFailure(JNIEnv* env) {
  if (env->ExceptionCheck()) return;
  jclass exception = env->FindClass("android/os/RemoteException");
  if (exception == nullptr) return;
  env->ThrowNew(exception, "Cannot connect to the system Binder service manager");
  env->DeleteLocalRef(exception);
}
}  // namespace

jobject GetSystemContextObject(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return nullptr;
  // Connection constructs only the remote endpoint and starts its dispatcher;
  // it must not query services or call application code while holding this lock.
  std::lock_guard<std::mutex> lock(context_mutex);
  if (context != nullptr) return env->NewLocalRef(context);
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
  android::sp<android::IBinder> native_root =
      android::ProcessState::initWithDriver("/dev/binder")->getContextObject(nullptr);
  jobject root = native_root == nullptr
                     ? nullptr
                     : android::javaObjectForIBinder(env, native_root);
#else
  jobject root =
      ConnectSystemBinder(env, std::getenv("DARWIN_ART_SYSTEM_SERVER_SOCKET"));
#endif
  if (root == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(root);
    ThrowConnectionFailure(env);
    return nullptr;
  }
  context = env->NewGlobalRef(root);
  if (context == nullptr) {
    env->DeleteLocalRef(root);
    ThrowConnectionFailure(env);
    return nullptr;
  }
  return root;
}
}  // namespace darwin_art
