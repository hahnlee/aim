#pragma once

#include "consumer_buffer.h"
#include <deque>
#include <mutex>

namespace darwin_art::media {
struct ConsumerFrame {
  OwnedConsumerBuffer lease;
  int32_t dataspace = 0;
  int64_t timestamp_ns = 0;
};
enum class ImageAcquireStatus { Ok = 0, NoBuffer = 1, MaxImages = 2 };
struct ImageAcquisition {
  ImageAcquireStatus status = ImageAcquireStatus::NoBuffer;
  ConsumerFrame frame;
};

// Owns pending-frame serialization and acquired-image accounting. JNI/listener
// delivery is outside this owner. Callers retain it until every acquired image
// has returned; closing prevents new acquisitions but does not revoke images.
class ImageConsumerQueue {
public:
  explicit ImageConsumerQueue(uint32_t max_images);
  ~ImageConsumerQueue();
  bool enqueue(ConsumerFrame frame);
  ImageAcquisition acquireNext();
  // Acquires the newest pending frame and returns older pending frames to the
  // producer. With fewer than two free acquisition slots, AOSP's
  // acquireLatest operation has insufficient margin to discard safely and
  // therefore follows acquireNext FIFO behavior.
  ImageAcquisition acquireLatest();
  bool releaseAcquired();
  void close();

private:
  std::mutex mutex_;
  const uint32_t max_images_;
  uint32_t acquired_ = 0;
  bool closed_ = false;
  std::deque<ConsumerFrame> pending_;
};
} // namespace darwin_art::media
