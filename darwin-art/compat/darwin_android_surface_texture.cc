#include "darwin_angle_egl.h"
#include "darwin_android_surface_texture.h"
#include "darwin_art_bionic_socket_broker.h"

#include <android/hardware_buffer.h>
#include <android/native_window.h>
#include <android/surface_texture.h>
#include <surfacetexture/surface_texture_platform.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <deque>
#include <memory>
#include <mutex>
#include <new>
#include <utility>

extern "C" int sync_wait(int fd, int timeout_ms);

struct SurfaceTextureQueueState;
struct SurfaceTextureCallbackState;
void ReleaseSurfaceTextureCallbackState(void* context);

namespace {
constexpr unsigned int kGlTextureExternalOes = 0x8D65;

bool DebugSurfaceTexture() {
  return std::getenv("DARWIN_ART_DEBUG_SURFACE_TEXTURE") != nullptr;
}

struct QueuedBuffer {
  AHardwareBuffer* buffer = nullptr;
  int32_t slot = -1;
  uint64_t generation = 0;
  uint64_t frame = 0;
  int fence = -1;
  android_dataspace dataspace = HAL_DATASPACE_UNKNOWN;
  int64_t timestamp_ns = 0;
};

void ReleaseFence(int fence) {
  if (fence >= 0) (void)darwin_art_bionic_socket_broker_close(fence);
}

void ReleaseQueued(void* producer, QueuedBuffer* queued,
                   int release_fence = -1) {
  if (queued == nullptr || queued->buffer == nullptr) {
    ReleaseFence(release_fence);
    return;
  }
  if (release_fence < 0) {
    // An unconsumed frame returns its producer acquire fence with the slot.
    release_fence = std::exchange(queued->fence, -1);
  } else if (queued->fence >= 0) {
    (void)sync_wait(queued->fence, -1);
  }
  darwin_art_android_ANativeWindow_release_consumer_frame(
      producer, queued->slot, queued->generation, queued->frame,
      release_fence);
  ReleaseFence(queued->fence);
  AHardwareBuffer_release(queued->buffer);
  *queued = QueuedBuffer{};
}
}  // namespace

struct SurfaceTextureQueueState {
  // Acquisition/fence completion and consumer retirement must serialize;
  // producer publication uses only mutex and remains free to enqueue frames.
  std::mutex consumer_mutex;
  std::mutex mutex;
  void* producer = nullptr;  // Borrowed while the owning producer is retained.
  std::deque<QueuedBuffer> pending;
  QueuedBuffer current;
  bool consumer_owned = false;
  bool abandoned = false;
};

struct ASurfaceTexture {
  std::atomic<uint32_t> references{1};
  std::shared_ptr<SurfaceTextureQueueState> queue;
  void* producer = nullptr;
  unsigned int texture_target = kGlTextureExternalOes;
  uint32_t attached_texture = 0;
};

struct SurfaceTextureCallbackState {
  JavaVM* vm = nullptr;
  jobject weak_self = nullptr;
  jclass surface_texture_class = nullptr;
  jmethodID post_event = nullptr;
  std::shared_ptr<SurfaceTextureQueueState> queue;
};

void DeleteGlobalRefOnAttachedThread(JavaVM* vm, jobject object,
                                     jclass clazz) {
  if (vm == nullptr || (object == nullptr && clazz == nullptr)) return;
  JNIEnv* env = nullptr;
  bool attached = false;
  const jint status = vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6);
  if (status == JNI_EDETACHED) {
    attached = vm->AttachCurrentThread(&env, nullptr) == JNI_OK;
  } else if (status != JNI_OK) {
    env = nullptr;
  }
  if (env != nullptr) {
    if (object != nullptr) env->DeleteGlobalRef(object);
    if (clazz != nullptr) env->DeleteGlobalRef(clazz);
  }
  if (attached) vm->DetachCurrentThread();
}

void ReleaseSurfaceTextureCallbackState(void* opaque) {
  auto* state = static_cast<SurfaceTextureCallbackState*>(opaque);
  if (state == nullptr) return;
  DeleteGlobalRefOnAttachedThread(state->vm, state->weak_self,
                                  state->surface_texture_class);
  delete state;
}

void NotifySurfaceTextureFrame(SurfaceTextureCallbackState* state) {
  if (state == nullptr || state->vm == nullptr || state->weak_self == nullptr ||
      state->surface_texture_class == nullptr || state->post_event == nullptr) {
    if (DebugSurfaceTexture())
      std::fprintf(stderr, "ART SurfaceTexture: notify unavailable state=%p\n",
                   static_cast<void*>(state));
    return;
  }
  JNIEnv* env = nullptr;
  bool attached = false;
  const jint status =
      state->vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6);
  if (status == JNI_EDETACHED) {
    attached = state->vm->AttachCurrentThread(&env, nullptr) == JNI_OK;
  } else if (status != JNI_OK) {
    env = nullptr;
  }
  if (env != nullptr && !env->ExceptionCheck()) {
    if (DebugSurfaceTexture()) {
      jclass weak_class = env->GetObjectClass(state->weak_self);
      jmethodID get = weak_class == nullptr
                          ? nullptr
                          : env->GetMethodID(weak_class, "get", "()Ljava/lang/Object;");
      jobject texture = get == nullptr
                            ? nullptr
                            : env->CallObjectMethod(state->weak_self, get);
      jfieldID handler_field = texture == nullptr
                                   ? nullptr
                                   : env->GetFieldID(state->surface_texture_class,
                                                     "mOnFrameAvailableHandler",
                                                     "Landroid/os/Handler;");
      jobject handler = handler_field == nullptr
                            ? nullptr
                            : env->GetObjectField(texture, handler_field);
      jclass handler_class = handler == nullptr ? nullptr : env->GetObjectClass(handler);
      jmethodID has_messages = handler_class == nullptr
                                   ? nullptr
                                   : env->GetMethodID(handler_class, "hasMessages", "(I)Z");
      const int pending_message = has_messages == nullptr
                                      ? -1
                                      : env->CallBooleanMethod(handler, has_messages, 0);
      jfieldID listener_field = handler_class == nullptr
                                    ? nullptr
                                    : env->GetFieldID(
                                          handler_class, "val$listener",
                                          "Landroid/graphics/SurfaceTexture$OnFrameAvailableListener;");
      jobject listener = listener_field == nullptr
                             ? nullptr
                             : env->GetObjectField(handler, listener_field);
      jclass listener_class = listener == nullptr ? nullptr : env->GetObjectClass(listener);
      jclass class_class = listener_class == nullptr
                               ? nullptr
                               : env->FindClass("java/lang/Class");
      jmethodID get_name = class_class == nullptr
                               ? nullptr
                               : env->GetMethodID(class_class, "getName", "()Ljava/lang/String;");
      jstring name = get_name == nullptr
                         ? nullptr
                         : static_cast<jstring>(env->CallObjectMethod(listener_class, get_name));
      const char* name_text = name == nullptr ? nullptr : env->GetStringUTFChars(name, nullptr);
      std::fprintf(stderr,
                   "ART SurfaceTexture: listener texture=%p handler=%p pending=%d class=%s exception=%d\n",
                   static_cast<void*>(texture), static_cast<void*>(handler),
                   pending_message, name_text == nullptr ? "none" : name_text,
                   env->ExceptionCheck() ? 1 : 0);
      if (name_text != nullptr) env->ReleaseStringUTFChars(name, name_text);
      if (name != nullptr) env->DeleteLocalRef(name);
      if (class_class != nullptr) env->DeleteLocalRef(class_class);
      if (listener_class != nullptr) env->DeleteLocalRef(listener_class);
      if (listener != nullptr) env->DeleteLocalRef(listener);
      if (handler_class != nullptr) env->DeleteLocalRef(handler_class);
      if (handler != nullptr) env->DeleteLocalRef(handler);
      if (texture != nullptr) env->DeleteLocalRef(texture);
      if (weak_class != nullptr) env->DeleteLocalRef(weak_class);
    }
    if (env->ExceptionCheck()) {
      if (attached) state->vm->DetachCurrentThread();
      return;
    }
    env->CallStaticVoidMethod(state->surface_texture_class, state->post_event,
                              state->weak_self);
    if (DebugSurfaceTexture())
      std::fprintf(stderr, "ART SurfaceTexture: notify dispatched exception=%d\n",
                   env->ExceptionCheck() ? 1 : 0);
    // Keep a Java exception pending. The producer callback has no policy for
    // translating or clearing Java failures, and SurfaceTexture's handler owns
    // the event semantics.
  }
  else if (DebugSurfaceTexture())
    std::fprintf(stderr, "ART SurfaceTexture: notify skipped env=%p exception=%d\n",
                 static_cast<void*>(env), env != nullptr && env->ExceptionCheck());
  if (attached) state->vm->DetachCurrentThread();
}

namespace {
void QueueBuffer(void* context, AHardwareBuffer* buffer, int32_t slot,
                 uint64_t generation, uint64_t frame, int fence,
                 int32_t dataspace) {
  auto* state = static_cast<SurfaceTextureCallbackState*>(context);
  auto queue = state == nullptr ? nullptr : state->queue;
  if (queue == nullptr || buffer == nullptr) {
    ReleaseFence(fence);
    return;
  }
  AHardwareBuffer_acquire(buffer);
  QueuedBuffer incoming{
      .buffer = buffer,
      .slot = slot,
      .generation = generation,
      .frame = frame,
      .fence = fence,
      .dataspace = static_cast<android_dataspace>(dataspace),
      .timestamp_ns = std::chrono::duration_cast<std::chrono::nanoseconds>(
                          std::chrono::steady_clock::now().time_since_epoch())
                          .count(),
  };
  // A resize can retain old-generation frames while a new pool is active.
  // Swap the entire pending deque in O(1) so MAILBOX never assumes that only
  // three records can be outstanding across generations.
  std::deque<QueuedBuffer> replaced;
  bool published = false;
  {
    std::lock_guard<std::mutex> lock(queue->mutex);
    if (!queue->abandoned) {
      try {
        if (darwin_art_android_ANativeWindow_get_present_mode(
                queue->producer) == DARWIN_ART_ANDROID_PRESENT_MODE_MAILBOX &&
            !queue->pending.empty()) {
          replaced.swap(queue->pending);
          try {
            queue->pending.push_back(incoming);
          } catch (const std::bad_alloc&) {
            replaced.swap(queue->pending);
            throw;
          }
        } else {
          queue->pending.push_back(incoming);
        }
        published = true;
      } catch (const std::bad_alloc&) {
        // The producer callback is a C ABI boundary. Return the slot and
        // acquire fence below instead of unwinding into BufferQueue.
      }
    }
  }
  while (!replaced.empty()) {
    QueuedBuffer displaced = replaced.front();
    replaced.pop_front();
    ReleaseQueued(queue->producer, &displaced);
  }
  if (!published) {
    ReleaseQueued(queue->producer, &incoming);
    return;
  }
  if (DebugSurfaceTexture())
    std::fprintf(stderr,
                 "ART SurfaceTexture: queued slot=%d generation=%llu frame=%llu\n",
                 slot, static_cast<unsigned long long>(generation),
                 static_cast<unsigned long long>(frame));
  {
    std::lock_guard<std::mutex> lock(queue->mutex);
    if (queue->abandoned) return;
  }
  // SurfaceTexture.postEventFromNative is the Android owner of the handler
  // dispatch. Never call TextureView or invalidate a view from this callback.
  NotifySurfaceTextureFrame(state);
}

void Identity(float* matrix) {
  if (matrix == nullptr) return;
  std::fill(matrix, matrix + 16, 0.0f);
  matrix[0] = matrix[5] = matrix[10] = matrix[15] = 1.0f;
}
}  // namespace

extern "C" ASurfaceTexture* darwin_art_android_surface_texture_create(
    uint32_t width, uint32_t height, int32_t format, uint32_t texture) {
  auto* surface_texture = new (std::nothrow) ASurfaceTexture();
  if (surface_texture == nullptr) return nullptr;
  try {
    surface_texture->queue = std::make_shared<SurfaceTextureQueueState>();
  } catch (const std::bad_alloc&) {
    delete surface_texture;
    return nullptr;
  }
  surface_texture->attached_texture = texture;
  surface_texture->producer = darwin_art_android_ANativeWindow_create(
      static_cast<int32_t>(width), static_cast<int32_t>(height), format);
  if (surface_texture->producer == nullptr) {
    delete surface_texture;
    return nullptr;
  }
  surface_texture->queue->producer = surface_texture->producer;
  auto* callback = new (std::nothrow) SurfaceTextureCallbackState();
  if (callback == nullptr) {
    darwin_art_android_ANativeWindow_release(surface_texture->producer);
    delete surface_texture;
    return nullptr;
  }
  callback->queue = surface_texture->queue;
  if (!darwin_art_android_ANativeWindow_set_owned_queue_callback(
          surface_texture->producer, &QueueBuffer, callback,
          &ReleaseSurfaceTextureCallbackState)) {
    delete callback;
    darwin_art_android_ANativeWindow_release(surface_texture->producer);
    delete surface_texture;
    return nullptr;
  }
  darwin_art_android_ANativeWindow_set_consumer_bound(
      surface_texture->producer, true);
  // SurfaceTexture's native consumer uses acquireLatestBuffer semantics by
  // default. Install the corresponding queue policy before publication.
  (void)darwin_art_android_ANativeWindow_set_present_mode(
      surface_texture->producer, DARWIN_ART_ANDROID_PRESENT_MODE_MAILBOX);
  return surface_texture;
}

extern "C" void* darwin_art_android_surface_texture_producer(
    ASurfaceTexture* surface_texture) {
  return surface_texture == nullptr ? nullptr : surface_texture->producer;
}

extern "C" void darwin_art_android_surface_texture_set_default_size(
    ASurfaceTexture* surface_texture, uint32_t width, uint32_t height) {
  if (surface_texture == nullptr || width == 0 || height == 0) return;
  (void)darwin_art_android_ANativeWindow_setBuffersGeometry(
      surface_texture->producer, static_cast<int32_t>(width),
      static_cast<int32_t>(height), 0);
}

extern "C" void darwin_art_android_surface_texture_abandon(
    ASurfaceTexture* surface_texture) {
  if (surface_texture == nullptr) return;
  std::deque<QueuedBuffer> pending;
  QueuedBuffer current;
  void* producer = nullptr;
  auto queue = surface_texture->queue;
  if (queue == nullptr) return;
  std::lock_guard<std::mutex> operation(queue->consumer_mutex);
  // Close producer admission before marking the consumer abandoned. A queue
  // captured before this point can complete and will be drained below; a new
  // remote dequeue or queue must observe the missing callback and fail.
  (void)darwin_art_android_ANativeWindow_set_owned_queue_callback(
      surface_texture->producer, nullptr, nullptr, nullptr);
  {
    std::lock_guard<std::mutex> lock(queue->mutex);
    if (queue->abandoned) return;
    queue->abandoned = true;
    producer = surface_texture->producer;
    pending.swap(queue->pending);
    current = std::exchange(queue->current, QueuedBuffer{});
  }
  while (!pending.empty()) {
    QueuedBuffer queued = pending.front();
    pending.pop_front();
    ReleaseQueued(producer, &queued);
  }
  ReleaseQueued(producer, &current);
}

extern "C" ASurfaceTexture* ASurfaceTexture_fromSurfaceTexture(
    JNIEnv* env, jobject object) {
  if (env == nullptr || object == nullptr) return nullptr;
  jclass clazz = env->GetObjectClass(object);
  jfieldID field = clazz == nullptr
                       ? nullptr
                       : env->GetFieldID(clazz, "mSurfaceTexture", "J");
  auto* texture = field == nullptr
                      ? nullptr
                      : reinterpret_cast<ASurfaceTexture*>(
                            static_cast<uintptr_t>(env->GetLongField(object,
                                                                     field)));
  env->DeleteLocalRef(clazz);
  if (texture != nullptr)
    texture->references.fetch_add(1, std::memory_order_relaxed);
  return texture;
}

extern "C" ANativeWindow* ASurfaceTexture_acquireANativeWindow(
    ASurfaceTexture* st) {
  if (st == nullptr) return nullptr;
  darwin_art_android_ANativeWindow_acquire(st->producer);
  return static_cast<ANativeWindow*>(st->producer);
}

extern "C" int ASurfaceTexture_attachToGLContext(ASurfaceTexture* st,
                                                   uint32_t texture) {
  if (st == nullptr) return -1;
  if (st->queue == nullptr) return -1;
  std::lock_guard<std::mutex> lock(st->queue->mutex);
  st->attached_texture = texture;
  return 0;
}

extern "C" int ASurfaceTexture_detachFromGLContext(ASurfaceTexture* st) {
  if (st == nullptr) return -1;
  if (st->queue == nullptr) return -1;
  std::lock_guard<std::mutex> lock(st->queue->mutex);
  st->attached_texture = 0;
  return 0;
}

extern "C" int ASurfaceTexture_updateTexImage(ASurfaceTexture* st) {
  if (st == nullptr || st->queue == nullptr) return -1;
  std::lock_guard<std::mutex> lock(st->queue->mutex);
  if (DebugSurfaceTexture())
    std::fprintf(stderr,
                 "ART SurfaceTexture: updateTexImage texture=%p pending=%zu current=%p attached=%u\n",
                 static_cast<void*>(st), st->queue->pending.size(),
                 static_cast<void*>(st->queue->current.buffer),
                 st->attached_texture);
  return st->queue->abandoned ? -1 : 0;
}

extern "C" void ASurfaceTexture_getTransformMatrix(ASurfaceTexture*,
                                                      float matrix[16]) {
  Identity(matrix);
}

extern "C" int64_t ASurfaceTexture_getTimestamp(ASurfaceTexture* st) {
  if (st == nullptr) return 0;
  auto queue = st->queue;
  if (queue == nullptr) return 0;
  std::lock_guard<std::mutex> lock(queue->mutex);
  if (!queue->pending.empty()) return queue->pending.back().timestamp_ns;
  return queue->current.timestamp_ns;
}

namespace android {
ANativeWindow* ASurfaceTexture_routeAcquireANativeWindow(ASurfaceTexture* st) {
  return ASurfaceTexture_acquireANativeWindow(st);
}

int ASurfaceTexture_routeAttachToGLContext(ASurfaceTexture* st,
                                           uint32_t texture) {
  return ASurfaceTexture_attachToGLContext(st, texture);
}

int ASurfaceTexture_routeDetachFromGLContext(ASurfaceTexture* st) {
  return ASurfaceTexture_detachFromGLContext(st);
}

void ASurfaceTexture_routeRelease(ASurfaceTexture* st) {
  ASurfaceTexture_release(st);
}

int ASurfaceTexture_routeUpdateTexImage(ASurfaceTexture* st) {
  return ASurfaceTexture_updateTexImage(st);
}

void ASurfaceTexture_routeGetTransformMatrix(ASurfaceTexture* st,
                                             float matrix[16]) {
  ASurfaceTexture_getTransformMatrix(st, matrix);
}

int64_t ASurfaceTexture_routeGetTimestamp(ASurfaceTexture* st) {
  return ASurfaceTexture_getTimestamp(st);
}

ASurfaceTexture* ASurfaceTexture_routeFromSurfaceTexture(JNIEnv* env,
                                                         jobject object) {
  return ASurfaceTexture_fromSurfaceTexture(env, object);
}

unsigned int ASurfaceTexture_getCurrentTextureTarget(ASurfaceTexture* st) {
  return st == nullptr ? kGlTextureExternalOes : st->texture_target;
}

void ASurfaceTexture_takeConsumerOwnership(ASurfaceTexture* st) {
  if (st == nullptr) return;
  auto queue = st->queue;
  if (queue == nullptr) return;
  std::lock_guard<std::mutex> lock(queue->mutex);
  queue->consumer_owned = true;
  if (DebugSurfaceTexture())
    std::fprintf(stderr, "ART SurfaceTexture: consumer-owned texture=%p\n",
                 static_cast<void*>(st));
}

void ASurfaceTexture_releaseConsumerOwnership(ASurfaceTexture* st) {
  if (st == nullptr) return;
  auto queue = st->queue;
  if (queue == nullptr) return;
  std::lock_guard<std::mutex> operation(queue->consumer_mutex);
  std::deque<QueuedBuffer> pending;
  QueuedBuffer current;
  {
    std::lock_guard<std::mutex> lock(queue->mutex);
    queue->consumer_owned = false;
    pending.swap(queue->pending);
    current = std::exchange(queue->current, QueuedBuffer{});
  }
  while (!pending.empty()) {
    QueuedBuffer queued = pending.front();
    pending.pop_front();
    ReleaseQueued(queue->producer, &queued);
  }
  ReleaseQueued(queue->producer, &current);
}

AHardwareBuffer* ASurfaceTexture_dequeueBuffer(
    ASurfaceTexture* st, int* outSlotid, android_dataspace* outDataspace,
    AHdrMetadataType* outHdrType, android_cta861_3_metadata* outCta861_3,
    android_smpte2086_metadata* outSmpte2086, float* outTransformMatrix,
    uint32_t* outTransform, bool* outNewContent,
    ASurfaceTexture_createReleaseFence createFence,
    ASurfaceTexture_fenceWait fenceWait, void* fenceHandle,
    ARect* currentCrop) {
  if (outNewContent != nullptr) *outNewContent = false;
  if (st == nullptr) return nullptr;
  auto queue = st->queue;
  if (queue == nullptr) return nullptr;
  std::lock_guard<std::mutex> operation(queue->consumer_mutex);
  std::deque<QueuedBuffer> stale_buffers;
  QueuedBuffer replaced;
  QueuedBuffer output;
  bool new_content = false;
  {
    std::lock_guard<std::mutex> lock(queue->mutex);
    if (queue->abandoned || !queue->consumer_owned) {
      if (DebugSurfaceTexture())
        std::fprintf(stderr,
                     "ART SurfaceTexture: dequeue unavailable abandoned=%d owned=%d\n",
                     queue->abandoned, queue->consumer_owned);
      return nullptr;
    }
    if (!queue->pending.empty()) {
      if (darwin_art_android_ANativeWindow_get_present_mode(
              queue->producer) == DARWIN_ART_ANDROID_PRESENT_MODE_MAILBOX) {
        stale_buffers.swap(queue->pending);
        replaced = std::exchange(queue->current, stale_buffers.back());
        stale_buffers.pop_back();
      } else {
        replaced = std::exchange(queue->current, queue->pending.front());
        queue->pending.pop_front();
      }
      new_content = true;
    }
    output = queue->current;
    if (output.buffer != nullptr) AHardwareBuffer_acquire(output.buffer);
  }
  if (DebugSurfaceTexture())
    std::fprintf(stderr,
                 "ART SurfaceTexture: dequeue slot=%d frame=%llu new=%d buffer=%p\n",
                 output.slot, static_cast<unsigned long long>(output.frame),
                 new_content, static_cast<void*>(output.buffer));
  for (auto& stale : stale_buffers) ReleaseQueued(queue->producer, &stale);
  if (replaced.buffer != nullptr) {
    int release_fence = -1;
    if (createFence != nullptr) {
      EGLSyncKHR egl_fence = EGL_NO_SYNC_KHR;
      EGLDisplay display = EGL_NO_DISPLAY;
      (void)createFence(true, &egl_fence, &display, &release_fence, fenceHandle);
    }
    ReleaseQueued(queue->producer, &replaced, release_fence);
  }
  if (output.buffer == nullptr) return nullptr;
  if (output.fence >= 0) {
    // A retained frame whose first acquire wait failed is first consumed on
    // this successful retry, even when no newer producer frame was queued.
    new_content = true;
    const int wait = fenceWait != nullptr
                         ? fenceWait(output.fence, fenceHandle)
                         : sync_wait(output.fence, -1);
    if (wait != 0) {
      AHardwareBuffer_release(output.buffer);
      return nullptr;
    }
    {
      std::lock_guard<std::mutex> lock(queue->mutex);
      queue->current.fence = -1;
    }
    ReleaseFence(output.fence);
  }
  AHardwareBuffer_Desc description{};
  AHardwareBuffer_describe(output.buffer, &description);
  if (outSlotid != nullptr) *outSlotid = output.slot;
  if (outDataspace != nullptr) *outDataspace = output.dataspace;
  if (outNewContent != nullptr) *outNewContent = new_content;
  if (outHdrType != nullptr) *outHdrType = static_cast<AHdrMetadataType>(0);
  if (outCta861_3 != nullptr) std::memset(outCta861_3, 0, sizeof(*outCta861_3));
  if (outSmpte2086 != nullptr)
    std::memset(outSmpte2086, 0, sizeof(*outSmpte2086));
  Identity(outTransformMatrix);
  if (outTransform != nullptr) *outTransform = 0;
  if (currentCrop != nullptr) {
    *currentCrop = ARect{0, 0, static_cast<int32_t>(description.width),
                         static_cast<int32_t>(description.height)};
  }
  return output.buffer;
}
}  // namespace android

extern "C" void ASurfaceTexture_release(ASurfaceTexture* st) {
  if (st == nullptr ||
      st->references.fetch_sub(1, std::memory_order_acq_rel) != 1) {
    return;
  }
  darwin_art_android_ANativeWindow_set_owned_queue_callback(
      st->producer, nullptr, nullptr, nullptr);
  auto queue = st->queue;
  std::lock_guard<std::mutex> operation(queue->consumer_mutex);
  std::deque<QueuedBuffer> pending;
  QueuedBuffer current;
  {
    std::lock_guard<std::mutex> lock(queue->mutex);
    queue->abandoned = true;
    pending.swap(queue->pending);
    current = std::exchange(queue->current, QueuedBuffer{});
  }
  while (!pending.empty()) {
    QueuedBuffer queued = pending.front();
    pending.pop_front();
    ReleaseQueued(st->producer, &queued);
  }
  ReleaseQueued(st->producer, &current);
  darwin_art_android_ANativeWindow_release(st->producer);
  delete st;
}

namespace {
struct SurfaceTextureFields {
  jfieldID texture = nullptr;
  jfieldID producer = nullptr;
  jclass clazz = nullptr;
  jmethodID post_event = nullptr;
};
SurfaceTextureFields g_surface_texture_fields;

bool InstallSurfaceTextureCallback(JNIEnv* env, ASurfaceTexture* texture,
                                   jobject weak_self) {
  if (env == nullptr || texture == nullptr || weak_self == nullptr ||
      g_surface_texture_fields.clazz == nullptr ||
      g_surface_texture_fields.post_event == nullptr)
    return false;
  auto state = new (std::nothrow) SurfaceTextureCallbackState();
  if (state == nullptr) return false;
  if (env->GetJavaVM(&state->vm) != JNI_OK || state->vm == nullptr) {
    delete state;
    return false;
  }
  state->weak_self = env->NewGlobalRef(weak_self);
  state->surface_texture_class =
      static_cast<jclass>(env->NewGlobalRef(g_surface_texture_fields.clazz));
  state->post_event = g_surface_texture_fields.post_event;
  state->queue = texture->queue;
  if (state->weak_self == nullptr || state->surface_texture_class == nullptr ||
      state->queue == nullptr || env->ExceptionCheck()) {
    DeleteGlobalRefOnAttachedThread(state->vm, state->weak_self,
                                    state->surface_texture_class);
    delete state;
    return false;
  }
  if (!darwin_art_android_ANativeWindow_set_owned_queue_callback(
          texture->producer, &QueueBuffer, state,
          &ReleaseSurfaceTextureCallbackState)) {
    ReleaseSurfaceTextureCallbackState(state);
    return false;
  }
  return true;
}

ASurfaceTexture* JavaSurfaceTexture(JNIEnv* env, jobject object) {
  if (env == nullptr || object == nullptr ||
      g_surface_texture_fields.texture == nullptr) {
    return nullptr;
  }
  return reinterpret_cast<ASurfaceTexture*>(static_cast<uintptr_t>(
      env->GetLongField(object, g_surface_texture_fields.texture)));
}

void SurfaceTextureNativeInit(JNIEnv* env, jobject object, jboolean detached,
                              jint texture_name, jboolean, jobject weak_self) {
  auto* texture = darwin_art_android_surface_texture_create(
      1, 1, AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
      detached == JNI_TRUE ? 0u : static_cast<uint32_t>(texture_name));
  if (texture == nullptr) return;
  if (!InstallSurfaceTextureCallback(env, texture, weak_self)) {
    ASurfaceTexture_release(texture);
    if (!env->ExceptionCheck()) {
      jclass exception_class = env->FindClass("java/lang/OutOfMemoryError");
      if (exception_class != nullptr) {
        env->ThrowNew(exception_class,
                      "SurfaceTexture frame callback unavailable");
        env->DeleteLocalRef(exception_class);
      }
    }
    return;
  }
  env->SetLongField(object, g_surface_texture_fields.texture,
                    reinterpret_cast<jlong>(texture));
  env->SetLongField(object, g_surface_texture_fields.producer,
                    reinterpret_cast<jlong>(texture->producer));
}

void SurfaceTextureNativeFinalize(JNIEnv* env, jobject object) {
  ASurfaceTexture* texture = JavaSurfaceTexture(env, object);
  env->SetLongField(object, g_surface_texture_fields.texture, 0);
  env->SetLongField(object, g_surface_texture_fields.producer, 0);
  ASurfaceTexture_release(texture);
}

void SurfaceTextureNativeSetDefaultBufferSize(JNIEnv* env, jobject object,
                                              jint width, jint height) {
  if (width > 0 && height > 0) {
    darwin_art_android_surface_texture_set_default_size(
        JavaSurfaceTexture(env, object), static_cast<uint32_t>(width),
        static_cast<uint32_t>(height));
  }
}

void SurfaceTextureNativeUpdateTexImage(JNIEnv* env, jobject object) {
  (void)ASurfaceTexture_updateTexImage(JavaSurfaceTexture(env, object));
}

void SurfaceTextureNativeReleaseTexImage(JNIEnv*, jobject) {}

jint SurfaceTextureNativeDetach(JNIEnv* env, jobject object) {
  return ASurfaceTexture_detachFromGLContext(JavaSurfaceTexture(env, object));
}

jint SurfaceTextureNativeAttach(JNIEnv* env, jobject object, jint texture) {
  return ASurfaceTexture_attachToGLContext(JavaSurfaceTexture(env, object),
                                           static_cast<uint32_t>(texture));
}

void SurfaceTextureNativeGetTransformMatrix(JNIEnv* env, jobject object,
                                            jfloatArray output) {
  if (output == nullptr || env->GetArrayLength(output) < 16) return;
  jfloat* matrix = env->GetFloatArrayElements(output, nullptr);
  if (matrix == nullptr) return;
  ASurfaceTexture_getTransformMatrix(JavaSurfaceTexture(env, object), matrix);
  env->ReleaseFloatArrayElements(output, matrix, 0);
}

jlong SurfaceTextureNativeGetTimestamp(JNIEnv* env, jobject object) {
  return ASurfaceTexture_getTimestamp(JavaSurfaceTexture(env, object));
}

jint SurfaceTextureNativeGetDataSpace(JNIEnv* env, jobject object) {
  ASurfaceTexture* texture = JavaSurfaceTexture(env, object);
  if (texture == nullptr) return HAL_DATASPACE_UNKNOWN;
  auto queue = texture->queue;
  if (queue == nullptr) return HAL_DATASPACE_UNKNOWN;
  std::lock_guard<std::mutex> lock(queue->mutex);
  if (!queue->pending.empty()) return queue->pending.back().dataspace;
  return queue->current.dataspace;
}

void SurfaceTextureNativeRelease(JNIEnv* env, jobject object) {
  darwin_art_android_surface_texture_abandon(JavaSurfaceTexture(env, object));
}

jboolean SurfaceTextureNativeIsReleased(JNIEnv* env, jobject object) {
  ASurfaceTexture* texture = JavaSurfaceTexture(env, object);
  if (texture == nullptr) return JNI_TRUE;
  auto queue = texture->queue;
  if (queue == nullptr) return JNI_TRUE;
  std::lock_guard<std::mutex> lock(queue->mutex);
  return queue->abandoned ? JNI_TRUE : JNI_FALSE;
}
}  // namespace

extern "C" jlong darwin_art_android_surface_texture_acquire_producer(
    JNIEnv* env, jobject object) {
  ASurfaceTexture* texture = JavaSurfaceTexture(env, object);
  if (texture == nullptr || texture->queue == nullptr) return 0;
  {
    std::lock_guard<std::mutex> lock(texture->queue->mutex);
    if (texture->queue->abandoned) return 0;
  }
  darwin_art_android_ANativeWindow_acquire(texture->producer);
  return reinterpret_cast<jlong>(texture->producer);
}

namespace darwin_art {
bool RegisterDarwinSurfaceTextureNatives(JNIEnv* env) {
  jclass clazz = env->FindClass("android/graphics/SurfaceTexture");
  if (clazz == nullptr) return false;
  g_surface_texture_fields.texture =
      env->GetFieldID(clazz, "mSurfaceTexture", "J");
  g_surface_texture_fields.producer = env->GetFieldID(clazz, "mProducer", "J");
  g_surface_texture_fields.clazz =
      static_cast<jclass>(env->NewGlobalRef(clazz));
  g_surface_texture_fields.post_event = env->GetStaticMethodID(
      clazz, "postEventFromNative", "(Ljava/lang/ref/WeakReference;)V");
  if (g_surface_texture_fields.texture == nullptr ||
      g_surface_texture_fields.producer == nullptr ||
      g_surface_texture_fields.clazz == nullptr ||
      g_surface_texture_fields.post_event == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(clazz);
    return false;
  }
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeInit"),
       const_cast<char*>("(ZIZLjava/lang/ref/WeakReference;)V"),
       reinterpret_cast<void*>(&SurfaceTextureNativeInit)},
      {const_cast<char*>("nativeFinalize"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&SurfaceTextureNativeFinalize)},
      {const_cast<char*>("nativeSetDefaultBufferSize"),
       const_cast<char*>("(II)V"),
       reinterpret_cast<void*>(&SurfaceTextureNativeSetDefaultBufferSize)},
      {const_cast<char*>("nativeUpdateTexImage"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&SurfaceTextureNativeUpdateTexImage)},
      {const_cast<char*>("nativeReleaseTexImage"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&SurfaceTextureNativeReleaseTexImage)},
      {const_cast<char*>("nativeDetachFromGLContext"),
       const_cast<char*>("()I"),
       reinterpret_cast<void*>(&SurfaceTextureNativeDetach)},
      {const_cast<char*>("nativeAttachToGLContext"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(&SurfaceTextureNativeAttach)},
      {const_cast<char*>("nativeGetTransformMatrix"),
       const_cast<char*>("([F)V"),
       reinterpret_cast<void*>(&SurfaceTextureNativeGetTransformMatrix)},
      {const_cast<char*>("nativeGetTimestamp"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&SurfaceTextureNativeGetTimestamp)},
      {const_cast<char*>("nativeGetDataSpace"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&SurfaceTextureNativeGetDataSpace)},
      {const_cast<char*>("nativeRelease"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&SurfaceTextureNativeRelease)},
      {const_cast<char*>("nativeIsReleased"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(&SurfaceTextureNativeIsReleased)},
  };
  const bool success =
      env->RegisterNatives(clazz, methods,
                           static_cast<jint>(sizeof(methods) / sizeof(methods[0]))) == JNI_OK;
  env->DeleteLocalRef(clazz);
  return success;
}
}  // namespace darwin_art
