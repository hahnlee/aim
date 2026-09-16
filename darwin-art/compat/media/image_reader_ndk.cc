#include "image_reader_ndk.h"

#include "consumer_buffer.h"
#include "image_consumer_queue.h"
#include "../darwin_angle_egl.h"

#include <android/hardware_buffer.h>
#include <media/NdkImage.h>
#include <media/NdkImageReader.h>
#include <media/NdkMediaError.h>

#include <algorithm>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstring>
#include <limits>
#include <memory>
#include <mutex>
#include <new>
#include <thread>
#include <utility>

extern "C" int darwin_art_bionic_socket_broker_close(int);
extern "C" int sync_wait(int fd, int timeout_ms);

struct ANativeWindow;

struct AImage {
  std::shared_ptr<void> state;
  darwin_art::media::OwnedConsumerBuffer lease;
  int32_t width = 0;
  int32_t height = 0;
  int32_t format = 0;
  int64_t timestamp_ns = 0;
  int32_t dataspace = 0;
  bool valid = false;
  bool linked = false;
  AImage* previous = nullptr;
  AImage* next = nullptr;
};

struct AImageReader {
  std::shared_ptr<void> state;
};

namespace darwin_art::media {

constexpr int32_t kMaxImageDimension = 16384;

struct ReaderState;

struct ObserverContext {
  std::shared_ptr<ReaderState> state;
  void* producer = nullptr;
};

struct ReaderState final : std::enable_shared_from_this<ReaderState> {
  ReaderState(int32_t width_value, int32_t height_value, int32_t format_value,
              uint64_t usage_value, int32_t max_images_value)
      : width(width_value),
        height(height_value),
        format(format_value),
        usage(usage_value),
        queue(static_cast<uint32_t>(max_images_value)) {}

  ~ReaderState() { Close(); }

  bool Start() noexcept {
    try {
      std::shared_ptr<ReaderState> self = shared_from_this();
      callback_thread = std::thread([self] { self->CallbackLoop(); });
      return true;
    } catch (...) {
      return false;
    }
  }

  void SetReader(AImageReader* reader_value) noexcept {
    std::lock_guard lock(mutex);
    reader = reader_value;
  }

  bool InstallProducer(void* producer_value) noexcept {
    if (producer_value == nullptr) return false;
    auto* context = new (std::nothrow)
        ObserverContext{shared_from_this(), producer_value};
    if (context == nullptr) return false;
    // The observer may outlive this ReaderState's producer reference. Keep a
    // separate window reference in the context until the last captured
    // callback retires, so a late callback can return its borrowed slot.
    darwin_art_android_ANativeWindow_acquire(producer_value);
    {
      std::lock_guard lock(mutex);
      producer = producer_value;
      observer_context = context;
      observer_retired = false;
    }
    if (!darwin_art_android_ANativeWindow_set_owned_queue_callback(
            producer, &QueueCallback, context, &ReleaseObserverContext)) {
      {
        std::lock_guard lock(mutex);
        observer_context = nullptr;
        producer = nullptr;
        observer_retired = true;
        state_changed.notify_all();
      }
      ReleaseObserverContext(context);
      return false;
    }
    return true;
  }

  void Close() noexcept;

  bool BeginAcquire() noexcept {
    std::lock_guard lock(mutex);
    if (closed) return false;
    ++active_acquisitions;
    return true;
  }

  void EndAcquire() noexcept {
    std::lock_guard lock(mutex);
    if (active_acquisitions != 0) --active_acquisitions;
    state_changed.notify_all();
  }

  bool Publish(AImage* image) noexcept {
    if (image == nullptr) return false;
    std::lock_guard lock(mutex);
    if (closed) return false;
    image->valid = true;
    image->linked = true;
    image->previous = nullptr;
    image->next = acquired_head;
    if (acquired_head != nullptr) acquired_head->previous = image;
    acquired_head = image;
    return true;
  }

  bool Detach(AImage* image, OwnedConsumerBuffer* lease,
              int release_fence, bool* adopted) noexcept {
    if (lease == nullptr || image == nullptr) return false;
    std::lock_guard lock(mutex);
    if (!image->linked || image->state.get() != static_cast<void*>(this))
      return false;
    if (adopted != nullptr) *adopted = false;
    if (release_fence >= 0) {
      if (image->lease.adoptFence(release_fence)) {
        if (adopted != nullptr) *adopted = true;
      }
    }
    UnlinkLocked(image);
    image->valid = false;
    *lease = std::move(image->lease);
    return true;
  }

  bool IsValid(const AImage* image) const noexcept {
    if (image == nullptr) return false;
    std::lock_guard lock(mutex);
    return !closed && image->linked && image->valid &&
           image->state.get() == static_cast<const void*>(this);
  }

  void OnFrame(void* callback_producer, AHardwareBuffer* buffer, int32_t slot,
               int fence,
               int32_t dataspace_value) noexcept;

 private:
  friend struct ObserverContext;

  static void QueueCallback(void* opaque, AHardwareBuffer* buffer,
                            int32_t slot, int fence, int32_t dataspace) {
    auto* context = static_cast<ObserverContext*>(opaque);
    if (context == nullptr || context->state == nullptr) {
      if (fence >= 0) (void)darwin_art_bionic_socket_broker_close(fence);
      return;
    }
    context->state->OnFrame(context->producer, buffer, slot, fence,
                            dataspace);
  }

  static void ReleaseObserverContext(void* opaque) noexcept {
    auto* context = static_cast<ObserverContext*>(opaque);
    if (context == nullptr) return;
    std::shared_ptr<ReaderState> state = std::move(context->state);
    void* producer = std::exchange(context->producer, nullptr);
    if (state != nullptr) state->ObserverRetired();
    delete context;
    if (producer != nullptr)
      darwin_art_android_ANativeWindow_release(producer);
  }

  void ObserverRetired() noexcept {
    std::lock_guard lock(mutex);
    observer_retired = true;
    state_changed.notify_all();
  }

  void CallbackLoop() noexcept {
    for (;;) {
      AImageReader_ImageListener callback{};
      AImageReader* reader_value = nullptr;
      {
        std::unique_lock lock(mutex);
        state_changed.wait(lock, [this] {
          return stopping || pending_callbacks != 0;
        });
        if (stopping) return;
        --pending_callbacks;
        callback = listener;
        reader_value = reader;
      }
      if (callback.onImageAvailable != nullptr && reader_value != nullptr) {
        try {
          callback.onImageAvailable(callback.context, reader_value);
        } catch (...) {
          // A C callback must not unwind into the reader's worker thread.
        }
      }
    }
  }

  void UnlinkLocked(AImage* image) noexcept {
    if (image->previous != nullptr)
      image->previous->next = image->next;
    else if (acquired_head == image)
      acquired_head = image->next;
    if (image->next != nullptr) image->next->previous = image->previous;
    image->previous = nullptr;
    image->next = nullptr;
    image->linked = false;
  }

 public:
  const int32_t width;
  const int32_t height;
  const int32_t format;
  const uint64_t usage;
  ImageConsumerQueue queue;

  mutable std::mutex mutex;
  std::condition_variable state_changed;
  AImageReader* reader = nullptr;
  void* producer = nullptr;
  ObserverContext* observer_context = nullptr;
  AImageReader_ImageListener listener{};
  AImage* acquired_head = nullptr;
  size_t active_acquisitions = 0;
  size_t pending_callbacks = 0;
  bool closed = false;
  bool closing = false;
  bool close_complete = false;
  bool stopping = false;
  bool observer_retired = true;
  std::thread callback_thread;
};

void ReaderState::OnFrame(void* callback_producer, AHardwareBuffer* buffer,
                          int32_t slot, int fence,
                          int32_t dataspace_value) noexcept {
  void* producer_value = nullptr;
  void* rejected_producer = nullptr;
  bool reject_slot = false;
  bool notify = false;
  {
    std::lock_guard lock(mutex);
    if (closed || buffer == nullptr || callback_producer == nullptr) {
      rejected_producer = callback_producer;
      reject_slot = callback_producer != nullptr && slot >= 0;
    } else {
      ++active_acquisitions;
      producer_value = callback_producer;
    }
  }
  if (producer_value == nullptr) {
    if (reject_slot) {
      (void)darwin_art_android_ANativeWindow_release_consumer_slot(
          rejected_producer, slot, fence);
    } else if (fence >= 0) {
      (void)darwin_art_bionic_socket_broker_close(fence);
    }
    return;
  }

  AHardwareBuffer_acquire(buffer);
  ConsumerFrame frame{
      .lease = OwnedConsumerBuffer(producer_value, slot, fence, buffer),
      .dataspace = dataspace_value,
      .timestamp_ns = std::chrono::duration_cast<std::chrono::nanoseconds>(
                           std::chrono::steady_clock::now().time_since_epoch())
                           .count(),
  };
  bool enqueued = false;
  try {
    enqueued = queue.enqueue(std::move(frame));
  } catch (...) {
    // frame's lease returns the exact producer slot and fence.
    enqueued = false;
  }
  if (enqueued) {
    std::lock_guard lock(mutex);
    if (!closed && listener.onImageAvailable != nullptr) {
      ++pending_callbacks;
      notify = true;
    }
  }
  {
    std::lock_guard lock(mutex);
    if (active_acquisitions != 0) --active_acquisitions;
    state_changed.notify_all();
  }
  if (notify) state_changed.notify_one();
}

void ReaderState::Close() noexcept {
  bool on_callback_thread = false;
  void* producer_value = nullptr;
  AImage* acquired = nullptr;
  {
    std::unique_lock lock(mutex);
    if (close_complete) return;
    if (closing) {
      on_callback_thread = callback_thread.joinable() &&
                           callback_thread.get_id() == std::this_thread::get_id();
      if (on_callback_thread) return;
      state_changed.wait(lock, [this] { return close_complete; });
      return;
    }
    closing = true;
    closed = true;
    listener = {};
    stopping = true;
    producer_value = producer;
  }
  state_changed.notify_all();

  // Registration replacement is intentionally non-blocking. In-flight
  // callbacks retain the observer context and its producer reference. They
  // reject and return late frames after this state is closed.
  if (producer_value != nullptr) {
    (void)darwin_art_android_ANativeWindow_set_owned_queue_callback(
        producer_value, nullptr, nullptr, nullptr);
  }
  {
    std::unique_lock lock(mutex);
    // Captured callbacks may retire after Close returns. The observer context
    // owns its producer reference and handles those callbacks' slot returns.
    state_changed.wait(lock, [this] { return active_acquisitions == 0; });
    acquired = acquired_head;
    acquired_head = nullptr;
    for (AImage* image = acquired; image != nullptr; image = image->next) {
      image->valid = false;
      image->linked = false;
      image->previous = nullptr;
    }
  }

  // Queue destruction and image lease returns happen outside state locks.
  queue.close();
  for (AImage* image = acquired; image != nullptr;) {
    AImage* next = image->next;
    image->next = nullptr;
    image->lease.reset();
    (void)queue.releaseAcquired();
    image = next;
  }

  on_callback_thread = callback_thread.joinable() &&
                       callback_thread.get_id() == std::this_thread::get_id();
  if (on_callback_thread) {
    callback_thread.detach();
  } else if (callback_thread.joinable()) {
    callback_thread.join();
  }

  {
    std::lock_guard lock(mutex);
    producer = nullptr;
    observer_context = nullptr;
    reader = nullptr;
    close_complete = true;
    closing = false;
  }
  state_changed.notify_all();
  if (producer_value != nullptr)
    darwin_art_android_ANativeWindow_release(producer_value);
}

bool IsSupportedFormatUsage(int32_t format, uint64_t usage) noexcept {
  return format == AIMAGE_FORMAT_RGBA_8888 &&
         usage == AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE;
}

std::shared_ptr<ReaderState> AsState(const std::shared_ptr<void>& erased) {
  if (erased == nullptr) return {};
  return std::shared_ptr<ReaderState>(erased,
                                      static_cast<ReaderState*>(erased.get()));
}

media_status_t AcquireImage(AImageReader* reader, AImage** out, int* fence_out,
                            bool latest, bool asynchronous) noexcept {
  if (out == nullptr || reader == nullptr ||
      (asynchronous && fence_out == nullptr))
    return AMEDIA_ERROR_INVALID_PARAMETER;
  *out = nullptr;
  if (fence_out != nullptr) *fence_out = -1;
  if (reader->state == nullptr) return AMEDIA_ERROR_INVALID_OBJECT;
  std::shared_ptr<ReaderState> state = AsState(reader->state);
  if (!state->BeginAcquire()) return AMEDIA_ERROR_INVALID_OBJECT;

  ImageAcquisition acquisition = latest ? state->queue.acquireLatest()
                                        : state->queue.acquireNext();
  if (acquisition.status != ImageAcquireStatus::Ok) {
    state->EndAcquire();
    return acquisition.status == ImageAcquireStatus::MaxImages
               ? AMEDIA_IMGREADER_MAX_IMAGES_ACQUIRED
               : AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE;
  }

  ConsumerFrame frame = std::move(acquisition.frame);
  if (!asynchronous && frame.lease.fence() >= 0) {
    const int fence = frame.lease.fence();
    if (sync_wait(fence, -1) != 0) {
      frame.lease.reset();
      (void)state->queue.releaseAcquired();
      state->EndAcquire();
      return AMEDIA_ERROR_UNKNOWN;
    }
    const int consumed = frame.lease.takeFence();
    if (consumed >= 0) (void)darwin_art_bionic_socket_broker_close(consumed);
  }

  auto* image = new (std::nothrow) AImage();
  if (image == nullptr) {
    frame.lease.reset();
    (void)state->queue.releaseAcquired();
    state->EndAcquire();
    return AMEDIA_ERROR_UNKNOWN;
  }
  image->state = state;
  image->lease = std::move(frame.lease);
  image->width = static_cast<int32_t>(
      image->lease.buffer() == nullptr ? state->width
                                       : [&] {
                                           AHardwareBuffer_Desc desc{};
                                           AHardwareBuffer_describe(
                                               image->lease.buffer(), &desc);
                                           return desc.width;
                                         }());
  image->height = static_cast<int32_t>(
      image->lease.buffer() == nullptr ? state->height
                                       : [&] {
                                           AHardwareBuffer_Desc desc{};
                                           AHardwareBuffer_describe(
                                               image->lease.buffer(), &desc);
                                           return desc.height;
                                         }());
  image->format = state->format;
  image->dataspace = frame.dataspace;
  image->timestamp_ns = frame.timestamp_ns;
  if (!state->Publish(image)) {
    image->lease.reset();
    delete image;
    (void)state->queue.releaseAcquired();
    state->EndAcquire();
    return AMEDIA_ERROR_INVALID_OBJECT;
  }
  if (asynchronous) *fence_out = image->lease.takeFence();
  state->EndAcquire();
  *out = image;
  return AMEDIA_OK;
}

bool ValidImage(const AImage* image) noexcept {
  std::shared_ptr<ReaderState> state = image == nullptr
                                           ? nullptr
                                           : AsState(image->state);
  return state != nullptr && state->IsValid(image);
}

void DeleteImage(AImage* image, int release_fence) noexcept {
  if (image == nullptr) {
    if (release_fence >= 0)
      (void)darwin_art_bionic_socket_broker_close(release_fence);
    return;
  }
  std::shared_ptr<ReaderState> state = AsState(image->state);
  OwnedConsumerBuffer lease;
  bool adopted = false;
  const bool detached = state != nullptr &&
                        state->Detach(image, &lease, release_fence, &adopted);
  if (release_fence >= 0 && !adopted)
    (void)darwin_art_bionic_socket_broker_close(release_fence);
  if (detached) {
    lease.reset();
    (void)state->queue.releaseAcquired();
  }
  delete image;
}

extern "C" media_status_t AImageReader_new(
    int32_t, int32_t, int32_t, int32_t, AImageReader** out) {
  if (out != nullptr) *out = nullptr;
  // The native window backing has no CPU-read allocation contract.
  return out == nullptr ? AMEDIA_ERROR_INVALID_PARAMETER
                        : AMEDIA_ERROR_UNSUPPORTED;
}

extern "C" media_status_t AImageReader_newWithUsage(
    int32_t width, int32_t height, int32_t format, uint64_t usage,
    int32_t max_images, AImageReader** out) {
  if (out == nullptr || width <= 0 || height <= 0 || max_images <= 0 ||
      width > kMaxImageDimension || height > kMaxImageDimension ||
      !IsSupportedFormatUsage(format, usage)) {
    if (out != nullptr) *out = nullptr;
    return AMEDIA_ERROR_INVALID_PARAMETER;
  }
  *out = nullptr;
  std::shared_ptr<ReaderState> state;
  try {
    state = std::make_shared<ReaderState>(width, height, format, usage,
                                          max_images);
  } catch (...) {
    return AMEDIA_ERROR_UNKNOWN;
  }
  auto* reader = new (std::nothrow) AImageReader();
  if (reader == nullptr) return AMEDIA_ERROR_UNKNOWN;
  reader->state = state;
  state->SetReader(reader);
  if (!state->Start()) {
    state->Close();
    delete reader;
    return AMEDIA_ERROR_UNKNOWN;
  }
  void* window = darwin_art_android_ANativeWindow_create(width, height, format);
  if (window == nullptr || !state->InstallProducer(window)) {
    if (window != nullptr)
      darwin_art_android_ANativeWindow_release(window);
    state->Close();
    delete reader;
    return AMEDIA_ERROR_UNKNOWN;
  }
  *out = reader;
  return AMEDIA_OK;
}

extern "C" void AImageReader_delete(AImageReader* reader) {
  if (reader == nullptr) return;
  std::shared_ptr<ReaderState> state = AsState(reader->state);
  reader->state.reset();
  if (state != nullptr) state->Close();
  delete reader;
}

extern "C" media_status_t AImageReader_getWindow(AImageReader* reader,
                                                   ANativeWindow** out) {
  if (reader == nullptr || out == nullptr) return AMEDIA_ERROR_INVALID_PARAMETER;
  *out = nullptr;
  std::shared_ptr<ReaderState> state = AsState(reader->state);
  if (state == nullptr) return AMEDIA_ERROR_INVALID_OBJECT;
  std::lock_guard lock(state->mutex);
  if (state->closed || state->producer == nullptr)
    return AMEDIA_ERROR_INVALID_OBJECT;
  *out = static_cast<ANativeWindow*>(state->producer);
  return AMEDIA_OK;
}

extern "C" media_status_t AImageReader_setImageListener(
    AImageReader* reader, AImageReader_ImageListener* listener) {
  if (reader == nullptr) return AMEDIA_ERROR_INVALID_PARAMETER;
  std::shared_ptr<ReaderState> state = AsState(reader->state);
  if (state == nullptr) return AMEDIA_ERROR_INVALID_OBJECT;
  {
    std::lock_guard lock(state->mutex);
    if (state->closed) return AMEDIA_ERROR_INVALID_OBJECT;
    state->listener = listener == nullptr ? AImageReader_ImageListener{}
                                          : *listener;
  }
  state->state_changed.notify_one();
  return AMEDIA_OK;
}

extern "C" media_status_t AImageReader_acquireNextImage(
    AImageReader* reader, AImage** image) {
  return AcquireImage(reader, image, nullptr, false, false);
}

extern "C" media_status_t AImageReader_acquireLatestImage(
    AImageReader* reader, AImage** image) {
  return AcquireImage(reader, image, nullptr, true, false);
}

extern "C" media_status_t AImageReader_acquireNextImageAsync(
    AImageReader* reader, AImage** image, int* fence) {
  return AcquireImage(reader, image, fence, false, true);
}

extern "C" media_status_t AImageReader_acquireLatestImageAsync(
    AImageReader* reader, AImage** image, int* fence) {
  return AcquireImage(reader, image, fence, true, true);
}

extern "C" void AImage_delete(AImage* image) { DeleteImage(image, -1); }

extern "C" void AImage_deleteAsync(AImage* image, int release_fence) {
  DeleteImage(image, release_fence);
}

extern "C" media_status_t AImage_getWidth(const AImage* image,
                                           int32_t* out) {
  if (image == nullptr || out == nullptr) return AMEDIA_ERROR_INVALID_PARAMETER;
  if (!ValidImage(image)) return AMEDIA_ERROR_INVALID_OBJECT;
  *out = image->width;
  return AMEDIA_OK;
}

extern "C" media_status_t AImage_getHeight(const AImage* image,
                                            int32_t* out) {
  if (image == nullptr || out == nullptr) return AMEDIA_ERROR_INVALID_PARAMETER;
  if (!ValidImage(image)) return AMEDIA_ERROR_INVALID_OBJECT;
  *out = image->height;
  return AMEDIA_OK;
}

extern "C" media_status_t AImage_getFormat(const AImage* image,
                                            int32_t* out) {
  if (image == nullptr || out == nullptr) return AMEDIA_ERROR_INVALID_PARAMETER;
  if (!ValidImage(image)) return AMEDIA_ERROR_INVALID_OBJECT;
  *out = image->format;
  return AMEDIA_OK;
}

extern "C" media_status_t AImage_getCropRect(const AImage* image,
                                              AImageCropRect* out) {
  if (image == nullptr || out == nullptr) return AMEDIA_ERROR_INVALID_PARAMETER;
  if (!ValidImage(image)) return AMEDIA_ERROR_INVALID_OBJECT;
  *out = AImageCropRect{0, 0, image->width, image->height};
  return AMEDIA_OK;
}

extern "C" media_status_t AImage_getTimestamp(const AImage* image,
                                               int64_t* out) {
  if (image == nullptr || out == nullptr) return AMEDIA_ERROR_INVALID_PARAMETER;
  if (!ValidImage(image)) return AMEDIA_ERROR_INVALID_OBJECT;
  *out = image->timestamp_ns;
  return AMEDIA_OK;
}

extern "C" media_status_t AImage_getNumberOfPlanes(const AImage* image,
                                                    int32_t* out) {
  if (image == nullptr || out == nullptr) return AMEDIA_ERROR_INVALID_PARAMETER;
  if (!ValidImage(image)) return AMEDIA_ERROR_INVALID_OBJECT;
  *out = 1;
  return AMEDIA_OK;
}

extern "C" media_status_t AImage_getPlanePixelStride(
    const AImage* image, int plane, int32_t* out) {
  if (image == nullptr || out == nullptr || plane != 0)
    return AMEDIA_ERROR_INVALID_PARAMETER;
  if (!ValidImage(image)) return AMEDIA_ERROR_INVALID_OBJECT;
  *out = 4;
  return AMEDIA_OK;
}

extern "C" media_status_t AImage_getPlaneRowStride(
    const AImage* image, int plane, int32_t* out) {
  if (image == nullptr || out == nullptr || plane != 0)
    return AMEDIA_ERROR_INVALID_PARAMETER;
  if (!ValidImage(image)) return AMEDIA_ERROR_INVALID_OBJECT;
  AHardwareBuffer_Desc desc{};
  AHardwareBuffer_describe(image->lease.buffer(), &desc);
  if (desc.stride > static_cast<uint32_t>(std::numeric_limits<int32_t>::max()) / 4)
    return AMEDIA_ERROR_UNKNOWN;
  *out = static_cast<int32_t>(desc.stride * 4);
  return AMEDIA_OK;
}

extern "C" media_status_t AImage_getPlaneData(const AImage* image, int plane,
                                               uint8_t** data,
                                               int* length) {
  if (image == nullptr || data == nullptr || length == nullptr || plane != 0)
    return AMEDIA_ERROR_INVALID_PARAMETER;
  if (!ValidImage(image)) return AMEDIA_ERROR_INVALID_OBJECT;
  *data = nullptr;
  *length = 0;
  // The native window allocates GPU-only buffers; no CPU lock/data pointer is
  // exposed until a CPU-readable backing is implemented at its owner.
  return AMEDIA_IMGREADER_CANNOT_LOCK_IMAGE;
}

extern "C" media_status_t AImage_getHardwareBuffer(
    const AImage* image, AHardwareBuffer** out) {
  if (image == nullptr || out == nullptr) return AMEDIA_ERROR_INVALID_PARAMETER;
  *out = nullptr;
  if (!ValidImage(image)) return AMEDIA_ERROR_INVALID_OBJECT;
  *out = image->lease.buffer();
  return *out == nullptr ? AMEDIA_ERROR_INVALID_OBJECT : AMEDIA_OK;
}

extern "C" media_status_t AImage_getDataSpace(const AImage* image,
                                               int32_t* out) {
  if (image == nullptr || out == nullptr) return AMEDIA_ERROR_INVALID_PARAMETER;
  if (!ValidImage(image)) return AMEDIA_ERROR_INVALID_OBJECT;
  *out = image->dataspace;
  return AMEDIA_OK;
}

void* ImageReaderSymbol(const char* symbol) {
  if (symbol == nullptr) return nullptr;
#define IMAGE_READER_FUNCTION(name) \
  if (std::strcmp(symbol, #name) == 0) return reinterpret_cast<void*>(&name)
  IMAGE_READER_FUNCTION(AImageReader_new);
  IMAGE_READER_FUNCTION(AImageReader_newWithUsage);
  IMAGE_READER_FUNCTION(AImageReader_delete);
  IMAGE_READER_FUNCTION(AImageReader_getWindow);
  IMAGE_READER_FUNCTION(AImageReader_setImageListener);
  IMAGE_READER_FUNCTION(AImageReader_acquireNextImage);
  IMAGE_READER_FUNCTION(AImageReader_acquireLatestImage);
  IMAGE_READER_FUNCTION(AImageReader_acquireNextImageAsync);
  IMAGE_READER_FUNCTION(AImageReader_acquireLatestImageAsync);
  IMAGE_READER_FUNCTION(AImage_delete);
  IMAGE_READER_FUNCTION(AImage_deleteAsync);
  IMAGE_READER_FUNCTION(AImage_getWidth);
  IMAGE_READER_FUNCTION(AImage_getHeight);
  IMAGE_READER_FUNCTION(AImage_getFormat);
  IMAGE_READER_FUNCTION(AImage_getCropRect);
  IMAGE_READER_FUNCTION(AImage_getTimestamp);
  IMAGE_READER_FUNCTION(AImage_getNumberOfPlanes);
  IMAGE_READER_FUNCTION(AImage_getPlanePixelStride);
  IMAGE_READER_FUNCTION(AImage_getPlaneRowStride);
  IMAGE_READER_FUNCTION(AImage_getPlaneData);
  IMAGE_READER_FUNCTION(AImage_getHardwareBuffer);
  IMAGE_READER_FUNCTION(AImage_getDataSpace);
#undef IMAGE_READER_FUNCTION
  return nullptr;
}

}  // namespace darwin_art::media
