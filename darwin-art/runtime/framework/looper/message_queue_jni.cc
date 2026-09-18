#include "message_queue_jni.h"
#include "darwin_android_platform.h"

#include <atomic>
#include <limits>
#include <memory>
#include <mutex>
#include <new>
#include <unordered_map>

namespace darwin_art::framework_system {
namespace {
// The Java mPtr is an opaque generation, never a reclaimable C++ address.
// Acquire pins a queue across native operations; retire prevents new admission.
// JNI and provider calls run outside the registry lock.
struct NativeMessageQueue {
  explicit NativeMessageQueue(void* owner) : looper(owner) {}
  void* const looper;
  std::atomic<bool> polling{false};
};
std::mutex registry_mutex;
std::unordered_map<jlong, std::shared_ptr<NativeMessageQueue>> queues;
jlong next_id = 1;

std::shared_ptr<NativeMessageQueue> Acquire(jlong id) {
  std::lock_guard lock(registry_mutex);
  auto it = queues.find(id);
  return it == queues.end() ? nullptr : it->second;
}
void ThrowOutOfMemory(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return;
  jclass klass = env->FindClass("java/lang/OutOfMemoryError");
  if (klass != nullptr) {
    env->ThrowNew(klass, "Native MessageQueue allocation failed");
    env->DeleteLocalRef(klass);
  }
}
}

jlong message_queue_native_init(JNIEnv* env, jclass) {
  if (env == nullptr || env->ExceptionCheck()) return 0;
  try {
    void* looper = darwin_art_android_platform_prepare_current_looper();
    if (looper == nullptr) return 0;
    auto queue = std::make_shared<NativeMessageQueue>(looper);
    std::lock_guard lock(registry_mutex);
    if (next_id == std::numeric_limits<jlong>::max()) throw std::bad_alloc();
    const jlong id = next_id++;
    queues.emplace(id, std::move(queue));
    return id;
  } catch (const std::bad_alloc&) {
    ThrowOutOfMemory(env);
    return 0;
  }
}

void message_queue_native_destroy(JNIEnv*, jclass, jlong id) {
  std::shared_ptr<NativeMessageQueue> retired;
  {
    std::lock_guard lock(registry_mutex);
    auto it = queues.find(id);
    if (it == queues.end()) return;
    retired = std::move(it->second);
    queues.erase(it);
  }
  // Retire outside the lock; in-flight operations retain their own lease.
}

void message_queue_native_poll_once(JNIEnv*, jobject, jlong id, jint timeout) {
  auto queue = Acquire(id);
  if (!queue) return;
  queue->polling.store(true, std::memory_order_release);
  (void)darwin_art_android_platform_poll_current_looper_timeout(timeout);
  queue->polling.store(false, std::memory_order_release);
}
void message_queue_native_wake(JNIEnv*, jclass, jlong id) {
  auto queue = Acquire(id);
  if (queue) darwin_art_android_platform_wake_looper(queue->looper);
}
jboolean message_queue_native_is_polling(JNIEnv*, jclass, jlong id) {
  auto queue = Acquire(id);
  return queue && queue->polling.load(std::memory_order_acquire) ? JNI_TRUE : JNI_FALSE;
}
void message_queue_native_set_file_descriptor_events(JNIEnv*, jclass, jlong, jint, jint) {
  // Existing framework FD-listener contract remains unsupported; this lifetime
  // extraction does not claim to implement Java descriptor dispatch.
}
void* message_queue_looper(JNIEnv* env, jobject java_queue) {
  if (env == nullptr || java_queue == nullptr || env->ExceptionCheck()) return nullptr;
  jclass klass = env->GetObjectClass(java_queue);
  if (klass == nullptr) return nullptr;
  jfieldID field = env->GetFieldID(klass, "mPtr", "J");
  env->DeleteLocalRef(klass);
  if (field == nullptr || env->ExceptionCheck()) return nullptr;
  const jlong id = env->GetLongField(java_queue, field);
  if (env->ExceptionCheck()) return nullptr;
  auto queue = Acquire(id);
  // The provider's process-lifetime thread association retains the ALooper.
  return queue ? queue->looper : nullptr;
}
} // namespace darwin_art::framework_system
