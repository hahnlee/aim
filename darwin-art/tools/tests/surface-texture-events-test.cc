#include <android/hardware_buffer.h>

#include <algorithm>
#include <cassert>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <map>
#include <condition_variable>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

// Include the production translation unit so these tests exercise its actual
// queue, callback, JNI lifetime, and fence paths rather than a mirror.
#include "../../compat/darwin_android_surface_texture.cc"

struct AHardwareBuffer {
  int references = 1;
  uint32_t width = 8;
  uint32_t height = 4;
  int32_t format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM;
};

namespace {
struct Producer {
  int references = 1;
  DarwinArtAndroidNativeWindowQueueCallback callback = nullptr;
  void* context = nullptr;
  void (*release_context)(void*) = nullptr;
  struct Retired {
    DarwinArtAndroidNativeWindowQueueCallback callback;
    void* context;
    void (*release_context)(void*);
  };
  std::vector<Retired> retired;
  struct ReturnedFrame {
    int32_t slot;
    uint64_t generation;
    uint64_t frame;
    int fence;
  };
  std::vector<ReturnedFrame> returned_frames;
  std::vector<int> returned_fences;
  std::vector<int> waited_fences;
  uint64_t next_frame = 1;
  int present_mode = DARWIN_ART_ANDROID_PRESENT_MODE_MAILBOX;
} *g_producer = nullptr;

AHardwareBuffer g_buffer;
int g_closed_fences = 0;
int g_global_refs = 0;
int g_global_deletes = 0;
int g_events = 0;
bool g_java_exception = false;
bool g_reenter_timestamp = false;
bool g_fail_wait = true;
bool g_block_wait = false;
bool g_wait_entered = false;
bool g_release_wait = false;
std::mutex g_wait_mutex;
std::condition_variable g_wait_cv;
ASurfaceTexture* g_reenter_texture = nullptr;
std::map<std::string, void*> g_methods;

struct Object {
  jlong surface_texture = 0;
  jlong producer = 0;
};
Object g_surface;

void RetireCallbacks() {
  assert(g_producer != nullptr);
  auto retired = std::move(g_producer->retired);
  for (const auto& callback : retired)
    if (callback.release_context != nullptr)
      callback.release_context(callback.context);
}

void InvokeFrame(int fence, int slot = 3, uint64_t generation = 1,
                 uint64_t frame = 0) {
  assert(g_producer != nullptr && g_producer->callback != nullptr);
  if (frame == 0) frame = g_producer->next_frame++;
  g_producer->callback(g_producer->context, &g_buffer, slot, generation, frame,
                       fence, 77);
}

jfieldID Field(JNIEnv*, jclass, const char* name, const char*) {
  if (std::strcmp(name, "mSurfaceTexture") == 0)
    return reinterpret_cast<jfieldID>(1);
  if (std::strcmp(name, "mProducer") == 0)
    return reinterpret_cast<jfieldID>(2);
  return nullptr;
}
jlong GetLong(JNIEnv*, jobject object, jfieldID field) {
  auto* value = reinterpret_cast<Object*>(object);
  return reinterpret_cast<uintptr_t>(field) == 1 ? value->surface_texture
                                                : value->producer;
}
void SetLong(JNIEnv*, jobject object, jfieldID field, jlong value) {
  auto* target = reinterpret_cast<Object*>(object);
  if (reinterpret_cast<uintptr_t>(field) == 1)
    target->surface_texture = value;
  else
    target->producer = value;
}

JNIEnv g_env{};
JavaVM g_vm{};

jint VmGetEnv(JavaVM*, void** output, jint) {
  *output = &g_env;
  return JNI_OK;
}
jint VmAttach(JavaVM*, JNIEnv** output, void*) {
  *output = &g_env;
  return JNI_OK;
}
jint VmDetach(JavaVM*) { return JNI_OK; }

void CallStatic(JNIEnv*, jclass, jmethodID, va_list arguments) {
  jobject weak = va_arg(arguments, jobject);
  assert(weak != nullptr);
  ++g_events;
  if (g_reenter_timestamp && g_reenter_texture != nullptr)
    (void)ASurfaceTexture_getTimestamp(g_reenter_texture);
}

void InstallJni() {
  static JNINativeInterface functions{};
  functions.FindClass = [](JNIEnv*, const char*) {
    return reinterpret_cast<jclass>(0x11);
  };
  functions.GetFieldID = Field;
  functions.GetStaticMethodID = [](JNIEnv*, jclass, const char* name,
                                   const char* signature) {
    assert(std::strcmp(name, "postEventFromNative") == 0);
    assert(std::strcmp(signature, "(Ljava/lang/ref/WeakReference;)V") == 0);
    return reinterpret_cast<jmethodID>(0x12);
  };
  functions.RegisterNatives = [](JNIEnv*, jclass, const JNINativeMethod* methods,
                                 jint count) {
    for (jint i = 0; i != count; ++i) g_methods[methods[i].name] = methods[i].fnPtr;
    return JNI_OK;
  };
  functions.GetObjectClass = [](JNIEnv*, jobject) {
    return reinterpret_cast<jclass>(0x11);
  };
  functions.DeleteLocalRef = [](JNIEnv*, jobject) {};
  functions.NewGlobalRef = [](JNIEnv*, jobject object) {
    ++g_global_refs;
    return object;
  };
  functions.DeleteGlobalRef = [](JNIEnv*, jobject) { ++g_global_deletes; };
  functions.GetLongField = GetLong;
  functions.SetLongField = SetLong;
  functions.GetJavaVM = [](JNIEnv*, JavaVM** output) {
    *output = &g_vm;
    return JNI_OK;
  };
  functions.ExceptionCheck = [](JNIEnv*) -> jboolean {
    return g_java_exception ? JNI_TRUE : JNI_FALSE;
  };
  functions.CallStaticVoidMethodV = CallStatic;
  g_env.functions = &functions;
  static JNIInvokeInterface vm_functions{};
  vm_functions.GetEnv = VmGetEnv;
  vm_functions.AttachCurrentThread = VmAttach;
  vm_functions.DetachCurrentThread = VmDetach;
  g_vm.functions = &vm_functions;
  assert(darwin_art::RegisterDarwinSurfaceTextureNatives(&g_env));
}
}  // namespace

extern "C" void* darwin_art_android_ANativeWindow_create(int32_t, int32_t,
                                                           int32_t) {
  assert(g_producer == nullptr);
  g_producer = new Producer;
  return g_producer;
}
extern "C" void darwin_art_android_ANativeWindow_acquire(void* window) {
  assert(window == g_producer);
  ++g_producer->references;
}
extern "C" void darwin_art_android_ANativeWindow_release(void* window) {
  assert(window == g_producer);
  assert(g_producer->references > 0);
  --g_producer->references;
}
extern "C" bool darwin_art_android_ANativeWindow_set_owned_queue_callback(
    void* window, DarwinArtAndroidNativeWindowQueueCallback callback,
    void* context, void (*release_context)(void*)) {
  assert(window == g_producer);
  if (g_producer->callback != nullptr)
    g_producer->retired.push_back(
        {g_producer->callback, g_producer->context, g_producer->release_context});
  g_producer->callback = callback;
  g_producer->context = context;
  g_producer->release_context = release_context;
  return true;
}
extern "C" void darwin_art_android_ANativeWindow_set_consumer_bound(void*, bool) {}
extern "C" bool darwin_art_android_ANativeWindow_supports_mailbox(void*) {
  return true;
}
extern "C" int32_t darwin_art_android_ANativeWindow_set_present_mode(
    void* window, int32_t mode) {
  assert(window == g_producer);
  assert(mode == DARWIN_ART_ANDROID_PRESENT_MODE_MAILBOX || mode ==
         DARWIN_ART_ANDROID_PRESENT_MODE_FIFO);
  g_producer->present_mode = mode;
  return 0;
}
extern "C" int32_t darwin_art_android_ANativeWindow_get_present_mode(
    void* window) {
  assert(window == g_producer);
  return g_producer->present_mode;
}
extern "C" bool darwin_art_android_ANativeWindow_has_queue_callback(void* window) {
  return window == g_producer && g_producer->callback != nullptr;
}
extern "C" bool darwin_art_android_ANativeWindow_is_consumer_bound(void*) { return true; }
extern "C" void darwin_art_android_ANativeWindow_release_consumer_slot(
    void* window, int32_t, int fence) {
  assert(window == g_producer);
  g_producer->returned_fences.push_back(fence);
}
extern "C" void darwin_art_android_ANativeWindow_release_consumer_frame(
    void* window, int32_t slot, uint64_t generation, uint64_t frame,
    int fence) {
  assert(window == g_producer);
  g_producer->returned_frames.push_back({slot, generation, frame, fence});
  g_producer->returned_fences.push_back(fence);
}
extern "C" int32_t darwin_art_android_ANativeWindow_setBuffersGeometry(void*, int32_t,
                                                                          int32_t, int32_t) {
  return 0;
}
extern "C" int darwin_art_bionic_socket_broker_close(int fence) {
  assert(fence >= 0);
  ++g_closed_fences;
  return 0;
}
extern "C" int sync_wait(int fence, int) {
  assert(fence >= 0);
  g_producer->waited_fences.push_back(fence);
  if (fence == 70 && g_block_wait) {
    std::unique_lock<std::mutex> lock(g_wait_mutex);
    g_wait_entered = true;
    g_wait_cv.notify_all();
    g_wait_cv.wait(lock, [] { return g_release_wait; });
  }
  return (fence == 60 || fence == 61) && g_fail_wait ? -1 : 0;
}
extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* buffer) {
  assert(buffer == &g_buffer);
  ++buffer->references;
}
extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer) {
  assert(buffer == &g_buffer && buffer->references > 0);
  --buffer->references;
}
extern "C" void AHardwareBuffer_describe(const AHardwareBuffer* buffer,
                                          AHardwareBuffer_Desc* output) {
  output->width = buffer->width;
  output->height = buffer->height;
  output->format = buffer->format;
}

int main() {
  InstallJni();

  // Native-only creation installs a real producer callback without any Java
  // listener and still publishes a frame to the consumer queue.
  ASurfaceTexture* native_only =
      darwin_art_android_surface_texture_create(8, 4, g_buffer.format, 0);
  assert(native_only != nullptr);
  android::ASurfaceTexture_takeConsumerOwnership(native_only);
  InvokeFrame(41);
  bool new_content = false;
  uint32_t transform = 0;
  AHardwareBuffer* output = android::ASurfaceTexture_dequeueBuffer(
      native_only, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
      &transform, &new_content, nullptr, nullptr, nullptr, nullptr);
  assert(output == &g_buffer && new_content);
  AHardwareBuffer_release(output);
  InvokeFrame(51, 5);
  InvokeFrame(52, 6);
  // MAILBOX releases the displaced pending frame at publication time, so a
  // three-slot producer does not wait for the consumer callback to catch up.
  assert(std::find(g_producer->returned_fences.begin(),
                   g_producer->returned_fences.end(), 51) !=
         g_producer->returned_fences.end());
  assert(std::find_if(g_producer->returned_frames.begin(),
                      g_producer->returned_frames.end(),
                      [](const Producer::ReturnedFrame& frame) {
                        return frame.slot == 5 && frame.generation == 1 &&
                               frame.frame == 2 && frame.fence == 51;
                      }) != g_producer->returned_frames.end());
  new_content = false;
  output = android::ASurfaceTexture_dequeueBuffer(
      native_only, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
      &transform, &new_content, nullptr, nullptr, nullptr, nullptr);
  assert(output == &g_buffer && new_content);
  assert(std::find(g_producer->returned_fences.begin(),
                   g_producer->returned_fences.end(), 51) !=
         g_producer->returned_fences.end());
  assert(g_producer->waited_fences.back() == 52);
  AHardwareBuffer_release(output);
  // Switching the same live consumer to FIFO preserves publication order;
  // each acquire fence is waited by its corresponding dequeue.
  assert(darwin_art_android_ANativeWindow_set_present_mode(
             g_producer, DARWIN_ART_ANDROID_PRESENT_MODE_FIFO) == 0);
  InvokeFrame(53, 7);
  InvokeFrame(54, 8);
  bool fifo_new_content = false;
  output = android::ASurfaceTexture_dequeueBuffer(
      native_only, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
      &transform, &fifo_new_content, nullptr, nullptr, nullptr, nullptr);
  assert(output == &g_buffer && fifo_new_content);
  assert(g_producer->waited_fences.back() == 53);
  AHardwareBuffer_release(output);
  output = android::ASurfaceTexture_dequeueBuffer(
      native_only, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
      &transform, &fifo_new_content, nullptr, nullptr, nullptr, nullptr);
  assert(output == &g_buffer && fifo_new_content);
  assert(g_producer->waited_fences.back() == 54);
  AHardwareBuffer_release(output);
  ASurfaceTexture_release(native_only);
  RetireCallbacks();
  g_closed_fences = 0;

  // Old-generation held frames can coexist with a new three-slot pool after
  // resize. FIFO may therefore hold more than three pending records before a
  // MAILBOX transition; every displaced token and fence must be returned.
  g_producer = nullptr;
  ASurfaceTexture* resized =
      darwin_art_android_surface_texture_create(8, 4, g_buffer.format, 0);
  assert(resized != nullptr);
  assert(darwin_art_android_ANativeWindow_set_present_mode(
             g_producer, DARWIN_ART_ANDROID_PRESENT_MODE_FIFO) == 0);
  for (int slot = 0; slot < 5; ++slot)
    InvokeFrame(100 + slot, slot, slot < 3 ? 1 : 2);
  assert(darwin_art_android_ANativeWindow_set_present_mode(
             g_producer, DARWIN_ART_ANDROID_PRESENT_MODE_MAILBOX) == 0);
  InvokeFrame(105, 5, 2);
  for (int slot = 0; slot < 5; ++slot) {
    assert(std::find_if(g_producer->returned_frames.begin(),
                        g_producer->returned_frames.end(),
                        [slot](const Producer::ReturnedFrame& frame) {
                          return frame.slot == slot && frame.fence == 100 + slot;
                        }) != g_producer->returned_frames.end());
  }
  ASurfaceTexture_release(resized);
  RetireCallbacks();

  // JNI initialization publishes before posting, and the Java callback may
  // reenter the timestamp reader without taking the publication mutex.
  g_producer = nullptr;
  Object weak_object;
  SurfaceTextureNativeInit(&g_env, reinterpret_cast<jobject>(&g_surface), JNI_FALSE,
                           0, JNI_FALSE, reinterpret_cast<jobject>(&weak_object));
  auto* texture = reinterpret_cast<ASurfaceTexture*>(
      static_cast<uintptr_t>(g_surface.surface_texture));
  assert(texture != nullptr && g_producer->callback != nullptr);
  g_reenter_texture = texture;
  g_reenter_timestamp = true;
  InvokeFrame(42);
  assert(g_global_refs == 3);  // class registration + weak target + callback class
  assert(g_events == 1);
  assert(g_producer->returned_fences.empty());

  // An abandon transfers an unconsumed acquire fence to the producer slot;
  // it must not close or reuse that fence early.
  darwin_art_android_surface_texture_abandon(texture);
  assert(g_producer->returned_fences.back() == 42);
  assert(g_closed_fences == 0);
  Producer* retired_producer = g_producer;
  ASurfaceTexture_release(texture);
  assert(g_global_deletes == 0);
  assert(!retired_producer->retired.empty());
  retired_producer->retired.front().callback(
      retired_producer->retired.front().context, &g_buffer, 4, 1, 2, 43, 77);
  assert(g_events == 1);
  RetireCallbacks();
  assert(g_global_deletes == 2);

  // A failed consumer wait leaves the frame pending. Abandon then returns the
  // same fence unchanged, proving the failure path did not discard it.
  g_surface.surface_texture = 0;
  g_producer = nullptr;
  ASurfaceTexture* waited =
      darwin_art_android_surface_texture_create(8, 4, g_buffer.format, 0);
  android::ASurfaceTexture_takeConsumerOwnership(waited);
  InvokeFrame(60);
  assert(android::ASurfaceTexture_dequeueBuffer(
             waited, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
             nullptr, nullptr, nullptr, nullptr, nullptr, nullptr) == nullptr);
  assert(!g_producer->waited_fences.empty() && g_producer->waited_fences.back() == 60);
  darwin_art_android_surface_texture_abandon(waited);
  assert(g_producer->returned_fences.back() == 60);
  ASurfaceTexture_release(waited);
  RetireCallbacks();

  // A successful retry of a retained current buffer is new content too.
  g_producer = nullptr;
  g_fail_wait = true;
  ASurfaceTexture* retry_texture =
      darwin_art_android_surface_texture_create(8, 4, g_buffer.format, 0);
  android::ASurfaceTexture_takeConsumerOwnership(retry_texture);
  InvokeFrame(61);
  assert(android::ASurfaceTexture_dequeueBuffer(
             retry_texture, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
             nullptr, nullptr, nullptr, nullptr, nullptr, nullptr) == nullptr);
  g_fail_wait = false;
  bool retry_new_content = false;
  AHardwareBuffer* retry = android::ASurfaceTexture_dequeueBuffer(
      retry_texture, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
      nullptr, &retry_new_content, nullptr, nullptr, nullptr, nullptr);
  assert(retry == &g_buffer);
  assert(retry_new_content);
  AHardwareBuffer_release(retry);
  darwin_art_android_surface_texture_abandon(retry_texture);
  assert(g_producer->returned_fences.back() == -1);
  ASurfaceTexture_release(retry_texture);
  RetireCallbacks();

  // Abandon serializes behind an in-flight fence wait. The returned buffer
  // remains a valid, metadata-described snapshot until the dequeue thread
  // releases its AHardwareBuffer reference.
  g_producer = nullptr;
  ASurfaceTexture* raced =
      darwin_art_android_surface_texture_create(8, 4, g_buffer.format, 0);
  android::ASurfaceTexture_takeConsumerOwnership(raced);
  InvokeFrame(70);
  g_block_wait = true;
  g_wait_entered = false;
  g_release_wait = false;
  AHardwareBuffer* raced_output = nullptr;
  android_dataspace raced_dataspace = HAL_DATASPACE_UNKNOWN;
  ARect raced_crop{};
  bool raced_new_content = false;
  std::thread dequeue_thread([&] {
    raced_output = android::ASurfaceTexture_dequeueBuffer(
        raced, nullptr, &raced_dataspace, nullptr, nullptr, nullptr, nullptr,
        nullptr, &raced_new_content, nullptr, nullptr, nullptr, &raced_crop);
  });
  {
    std::unique_lock<std::mutex> lock(g_wait_mutex);
    g_wait_cv.wait(lock, [] { return g_wait_entered; });
  }
  std::thread abandon_thread([&] { darwin_art_android_surface_texture_abandon(raced); });
  std::this_thread::yield();
  assert(g_producer->returned_fences.empty());
  {
    std::lock_guard<std::mutex> lock(g_wait_mutex);
    g_release_wait = true;
  }
  g_wait_cv.notify_all();
  dequeue_thread.join();
  abandon_thread.join();
  g_block_wait = false;
  assert(raced_output == &g_buffer && raced_new_content);
  assert(raced_dataspace == 77 && raced_crop.right == 8 && raced_crop.bottom == 4);
  AHardwareBuffer_release(raced_output);
  ASurfaceTexture_release(raced);
  RetireCallbacks();

  std::puts("surface-texture-events: PASS actual queue publication/reentry/abandon/fence/native-only");
}
