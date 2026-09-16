#pragma once
#include <jni.h>

namespace darwin_art::framework::system {

// Called only by the system process bootstrap. AOSP owns the singleton and
// initializes its contents; native code neither fabricates fields nor copies
// the shared layout. A second initialization is an error, as it is on Android.
inline bool InitializeApplicationSharedMemory(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck() || env->PushLocalFrame(8) < 0) return false;
  jclass type = env->FindClass("com/android/internal/os/ApplicationSharedMemory");
  if (type == nullptr) { env->PopLocalFrame(nullptr); return false; }
  jmethodID create = env->GetStaticMethodID(type, "create",
      "()Lcom/android/internal/os/ApplicationSharedMemory;");
  jmethodID install = create == nullptr ? nullptr : env->GetStaticMethodID(type, "setInstance",
      "(Lcom/android/internal/os/ApplicationSharedMemory;)V");
  if (install == nullptr) { env->PopLocalFrame(nullptr); return false; }
  jobject region = env->CallStaticObjectMethod(type, create);
  if (region == nullptr || env->ExceptionCheck()) { env->PopLocalFrame(nullptr); return false; }
  env->CallStaticVoidMethod(type, install, region);
  bool success = !env->ExceptionCheck();
  env->PopLocalFrame(nullptr);
  return success;
}

// Used by the system's application binding endpoint. Java owns the returned
// FileDescriptor until it is transferred/closed by the caller. This cannot
// initialize a missing singleton, and must not be called in app bootstrap.
inline jobject DuplicateApplicationSharedMemoryReader(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck() || env->PushLocalFrame(8) < 0) return nullptr;
  jclass type = env->FindClass("com/android/internal/os/ApplicationSharedMemory");
  if (type == nullptr) return env->PopLocalFrame(nullptr);
  jmethodID current = env->GetStaticMethodID(type, "getInstance",
      "()Lcom/android/internal/os/ApplicationSharedMemory;");
  jmethodID reader = current == nullptr ? nullptr : env->GetMethodID(type,
      "getReadOnlyFileDescriptor", "()Ljava/io/FileDescriptor;");
  if (reader == nullptr) return env->PopLocalFrame(nullptr);
  jobject region = env->CallStaticObjectMethod(type, current);
  if (region == nullptr || env->ExceptionCheck()) return env->PopLocalFrame(nullptr);
  jobject fd = env->CallObjectMethod(region, reader);
  return env->PopLocalFrame(env->ExceptionCheck() ? nullptr : fd);
}

// Close only the sending duplicate after the one-way Binder transaction has
// consumed it. The system-owned mutable singleton remains open and mapped.
inline void CloseApplicationSharedMemoryReader(JNIEnv* env, jobject fd) {
  if (env == nullptr || fd == nullptr || env->ExceptionCheck()) return;
  jclass io = env->FindClass("libcore/io/IoUtils");
  jmethodID close = io == nullptr ? nullptr : env->GetStaticMethodID(
      io, "closeQuietly", "(Ljava/io/FileDescriptor;)V");
  if (close != nullptr) env->CallStaticVoidMethod(io, close, fd);
  if (io != nullptr) env->DeleteLocalRef(io);
}

}  // namespace darwin_art::framework::system
