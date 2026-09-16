#include "image_consumer_queue.h"

#include <algorithm>
#include <utility>

namespace darwin_art::media {
ImageConsumerQueue::ImageConsumerQueue(uint32_t max_images)
    : max_images_(std::max(1u, max_images)) {}
ImageConsumerQueue::~ImageConsumerQueue() { close(); }

bool ImageConsumerQueue::enqueue(ConsumerFrame frame) {
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (closed_)
      return false;
    // maxImages limits acquired images, not pending frames. The producer's
    // retained BufferQueue slots bound outstanding submissions. Do not turn
    // acquireNext into a latest-frame stream by silently discarding the FIFO.
    pending_.push_back(std::move(frame));
  }
  // Slot/window destruction may reenter producer code; no queue mutex held.
  return true;
}

ImageAcquisition ImageConsumerQueue::acquireNext() {
  ImageAcquisition result;
  std::lock_guard<std::mutex> lock(mutex_);
  if (closed_)
    return result;
  if (acquired_ >= max_images_) {
    result.status = ImageAcquireStatus::MaxImages;
    return result;
  }
  if (pending_.empty())
    return result;
  result.frame = std::move(pending_.front());
  pending_.pop_front();
  ++acquired_;
  result.status = ImageAcquireStatus::Ok;
  return result;
}

ImageAcquisition ImageConsumerQueue::acquireLatest() {
  ImageAcquisition result;
  // Frames discarded by acquireLatest own producer/window resources. Keep
  // them alive in a local container so their destruction cannot reenter the
  // queue while mutex_ is held.
  std::deque<ConsumerFrame> discarded;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (closed_)
      return result;
    const uint32_t margin =
        max_images_ > acquired_ ? max_images_ - acquired_ : 0;
    if (margin == 0) {
      result.status = ImageAcquireStatus::MaxImages;
      return result;
    }
    if (pending_.empty())
      return result;

    if (margin < 2) {
      // AOSP requires two free acquisition slots to acquire and discard an
      // older image while retaining the newest one. Preserve FIFO semantics
      // when that margin is unavailable.
      result.frame = std::move(pending_.front());
      pending_.pop_front();
    } else {
      result.frame = std::move(pending_.back());
      pending_.pop_back();
      discarded.swap(pending_);
    }
    ++acquired_;
    result.status = ImageAcquireStatus::Ok;
  }
  return result;
}

bool ImageConsumerQueue::releaseAcquired() {
  std::lock_guard<std::mutex> lock(mutex_);
  if (acquired_ == 0)
    return false;
  --acquired_;
  return true;
}

void ImageConsumerQueue::close() {
  std::deque<ConsumerFrame> discarded;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    closed_ = true;
    discarded.swap(pending_);
  }
}
} // namespace darwin_art::media
