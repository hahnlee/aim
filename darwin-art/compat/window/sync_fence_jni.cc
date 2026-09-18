#include "sync_fence_jni.h"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <iterator>
#include <limits>
#include <new>

extern "C" int darwin_art_bionic_socket_broker_close(int);
extern "C" int sync_wait(int, int);

namespace {

struct DarwinSyncFence {
  std::atomic<uint32_t> references{1};
  int fd = -1;
};

void SyncFenceFinalizer(void* opaque) {
  auto* fence = static_cast<DarwinSyncFence*>(opaque);
  if (fence == nullptr ||
      fence->references.fetch_sub(1, std::memory_order_acq_rel) != 1) {
    return;
  }
  if (fence->fd >= 0) (void)darwin_art_bionic_socket_broker_close(fence->fd);
  delete fence;
}

jlong SyncFenceNativeGetDestructor(JNIEnv*, jclass) {
  return reinterpret_cast<jlong>(&SyncFenceFinalizer);
}

jlong SyncFenceNativeCreate(JNIEnv*, jclass, jint fd) {
  if (fd < 0) return 0;
  auto* fence = new (std::nothrow) DarwinSyncFence();
  if (fence == nullptr) {
    (void)darwin_art_bionic_socket_broker_close(fd);
    return 0;
  }
  fence->fd = fd;
  return reinterpret_cast<jlong>(fence);
}

jboolean SyncFenceNativeIsValid(JNIEnv*, jclass, jlong handle) {
  const auto* fence = reinterpret_cast<const DarwinSyncFence*>(
      static_cast<uintptr_t>(handle));
  return fence != nullptr && fence->fd >= 0 ? JNI_TRUE : JNI_FALSE;
}

jint SyncFenceNativeGetFd(JNIEnv*, jclass, jlong handle) {
  const auto* fence = reinterpret_cast<const DarwinSyncFence*>(
      static_cast<uintptr_t>(handle));
  return fence == nullptr ? -1 : fence->fd;
}

jboolean SyncFenceNativeWait(JNIEnv*, jclass, jlong handle,
                             jlong timeout_nanos) {
  const auto* fence = reinterpret_cast<const DarwinSyncFence*>(
      static_cast<uintptr_t>(handle));
  if (fence == nullptr || fence->fd < 0) return JNI_TRUE;
  const int timeout_millis =
      timeout_nanos < 0
          ? -1
          : static_cast<int>(std::min<jlong>(
                std::numeric_limits<int>::max(),
                timeout_nanos / 1000000 +
                    (timeout_nanos % 1000000 != 0 ? 1 : 0)));
  return sync_wait(fence->fd, timeout_millis) == 0 ? JNI_TRUE : JNI_FALSE;
}

jlong SyncFenceNativeGetSignalTime(JNIEnv*, jclass, jlong handle) {
  const auto* fence = reinterpret_cast<const DarwinSyncFence*>(
      static_cast<uintptr_t>(handle));
  if (fence == nullptr || fence->fd < 0) return -1;
  if (sync_wait(fence->fd, 0) != 0) return std::numeric_limits<jlong>::max();
  return std::chrono::duration_cast<std::chrono::nanoseconds>(
             std::chrono::steady_clock::now().time_since_epoch())
      .count();
}

void SyncFenceNativeIncRef(JNIEnv*, jclass, jlong handle) {
  auto* fence = reinterpret_cast<DarwinSyncFence*>(
      static_cast<uintptr_t>(handle));
  if (fence != nullptr)
    fence->references.fetch_add(1, std::memory_order_relaxed);
}

bool Register(JNIEnv* env, const char* class_name, JNINativeMethod* methods,
              jint method_count) {
  jclass klass = env->FindClass(class_name);
  if (klass == nullptr) return false;
  const bool registered =
      env->RegisterNatives(klass, methods, method_count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return registered;
}

}  // namespace

namespace darwin_art::window {

bool RegisterSyncFenceNatives(JNIEnv* env) {
  JNINativeMethod methods[] = {
      {const_cast<char*>("nGetDestructor"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&SyncFenceNativeGetDestructor)},
      {const_cast<char*>("nCreate"), const_cast<char*>("(I)J"),
       reinterpret_cast<void*>(&SyncFenceNativeCreate)},
      {const_cast<char*>("nIsValid"), const_cast<char*>("(J)Z"),
       reinterpret_cast<void*>(&SyncFenceNativeIsValid)},
      {const_cast<char*>("nGetFd"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&SyncFenceNativeGetFd)},
      {const_cast<char*>("nWait"), const_cast<char*>("(JJ)Z"),
       reinterpret_cast<void*>(&SyncFenceNativeWait)},
      {const_cast<char*>("nGetSignalTime"), const_cast<char*>("(J)J"),
       reinterpret_cast<void*>(&SyncFenceNativeGetSignalTime)},
      {const_cast<char*>("nIncRef"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&SyncFenceNativeIncRef)},
  };
  return Register(env, "android/hardware/SyncFence", methods,
                  static_cast<jint>(std::size(methods)));
}

}  // namespace darwin_art::window
