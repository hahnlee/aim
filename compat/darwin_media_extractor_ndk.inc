// NDK AMediaExtractor/AMediaDataSource implementation.  This file is
// included by darwin_android_media_ndk.cc after AMediaFormat is defined.

#include "darwin_media_extractor.h"

#include <algorithm>
#include <limits>
#include <mutex>
#include <new>
#include <string>
#include <sys/types.h>

extern "C" int aim_bionic_open(const char*, int, uint32_t);
extern "C" intptr_t aim_bionic_read(int, void*, size_t);
extern "C" int64_t aim_bionic_lseek(int, int64_t, int);
extern "C" int aim_bionic_close(int);
extern "C" intptr_t aim_bionic_pread(int, void*, size_t, int64_t);

// off64_t is the Android spelling; int64_t has the same AAPCS64 ABI and is
// available in the host build headers used by this facade.
using AMediaDataSourceReadAt = ssize_t (*)(void*, int64_t, void*, size_t);
using AMediaDataSourceGetSize = ssize_t (*)(void*);
using AMediaDataSourceClose = void (*)(void*);
using AMediaDataSourceGetAvailableSize = ssize_t (*)(void*, int64_t);

struct AMediaDataSource {
  std::mutex mutex;
  void* userdata = nullptr;
  AMediaDataSourceReadAt read_at = nullptr;
  AMediaDataSourceGetSize get_size = nullptr;
  AMediaDataSourceClose close = nullptr;
  AMediaDataSourceGetAvailableSize get_available_size = nullptr;
  bool closed = false;
};

struct AMediaExtractor {
  std::mutex mutex;
  aim::MediaExtractorState state;
};

namespace aim_media_extractor_detail {

constexpr size_t kReadChunk = 1024u * 1024u;

void Reset(aim::MediaExtractorState* state) {
  if (state != nullptr) *state = aim::MediaExtractorState{};
}

bool ReadFd(int fd, int64_t offset, int64_t length, std::vector<uint8_t>* output) {
  if (output == nullptr || fd < 0 || offset < 0 || length <= 0 ||
      static_cast<uint64_t>(length) >
          aim::kMediaExtractorMaxBytes) {
    return false;
  }
  output->assign(static_cast<size_t>(length), 0);
  size_t total = 0;
  while (total < output->size()) {
    const size_t request = std::min(kReadChunk, output->size() - total);
    if (offset > std::numeric_limits<int64_t>::max() -
                     static_cast<int64_t>(total)) {
      output->clear();
      return false;
    }
    const intptr_t count = aim_bionic_pread(
        fd, output->data() + total, request,
        offset + static_cast<int64_t>(total));
    if (count <= 0 || static_cast<uintptr_t>(count) > request) {
      output->clear();
      return false;
    }
    total += static_cast<size_t>(count);
  }
  return true;
}

bool ReadPath(const char* location, std::vector<uint8_t>* output) {
  if (location == nullptr || output == nullptr) return false;
  std::string path(location);
  if (path.rfind("file://", 0) == 0) path.erase(0, 7);
  else if (path.rfind("file:", 0) == 0) path.erase(0, 5);
  if (path.empty()) return false;
  const int fd = aim_bionic_open(path.c_str(), 0, 0);
  if (fd < 0) return false;
  const int64_t end = aim_bionic_lseek(fd, 0, 2 /* SEEK_END */);
  bool success = false;
  if (end > 0 && static_cast<uint64_t>(end) <=
                      aim::kMediaExtractorMaxBytes &&
      aim_bionic_lseek(fd, 0, 0 /* SEEK_SET */) == 0) {
    output->assign(static_cast<size_t>(end), 0);
    size_t total = 0;
    while (total < output->size()) {
      const intptr_t count = aim_bionic_read(
          fd, output->data() + total,
          std::min(kReadChunk, output->size() - total));
      if (count <= 0 || static_cast<uintptr_t>(count) > output->size() - total)
        break;
      total += static_cast<size_t>(count);
    }
    success = total == output->size();
    if (!success) output->clear();
  }
  aim_bionic_close(fd);
  return success;
}

bool ReadCustom(AMediaDataSource* source, std::vector<uint8_t>* output) {
  if (source == nullptr || output == nullptr) return false;
  AMediaDataSourceGetSize get_size = nullptr;
  AMediaDataSourceReadAt read_at = nullptr;
  void* userdata = nullptr;
  {
    std::lock_guard<std::mutex> lock(source->mutex);
    if (source->closed) return false;
    get_size = source->get_size;
    read_at = source->read_at;
    userdata = source->userdata;
  }
  if (get_size == nullptr || read_at == nullptr) return false;
  const ssize_t signed_size = get_size(userdata);
  if (signed_size <= 0 || static_cast<uint64_t>(signed_size) >
                              aim::kMediaExtractorMaxBytes) {
    return false;
  }
  output->assign(static_cast<size_t>(signed_size), 0);
  size_t total = 0;
  while (total < output->size()) {
    const size_t request = std::min(kReadChunk, output->size() - total);
    const ssize_t count = read_at(userdata, static_cast<int64_t>(total),
                                  output->data() + total, request);
    if (count <= 0 || static_cast<uint64_t>(count) > request) {
      output->clear();
      return false;
    }
    total += static_cast<size_t>(count);
  }
  return true;
}

bool ParseBytes(const std::vector<uint8_t>& bytes,
                aim::MediaExtractorState* state) {
  if (state == nullptr || bytes.empty() ||
      bytes.size() > aim::kMediaExtractorMaxBytes) {
    return false;
  }
  aim::ParseWebm(bytes, state);
  return state->has_source;
}

void CloseDataSource(AMediaDataSource* source) {
  if (source == nullptr) return;
  AMediaDataSourceClose close = nullptr;
  void* userdata = nullptr;
  {
    std::lock_guard<std::mutex> lock(source->mutex);
    if (source->closed) return;
    source->closed = true;
    close = source->close;
    userdata = source->userdata;
  }
  if (close != nullptr) close(userdata);
}

AMediaFormat* MakeFormat(const aim::MediaExtractorState& state) {
  if (!state.has_source) return nullptr;
  AMediaFormat* format = AMediaFormat_new();
  if (format == nullptr) return nullptr;
  AMediaFormat_setString(format, "mime", "video/x-vnd.on2.vp9");
  AMediaFormat_setInt32(format, "width", state.width);
  AMediaFormat_setInt32(format, "height", state.height);
  AMediaFormat_setFloat(format, "frame-rate", 30.0f);
  AMediaFormat_setInt64(format, "durationUs", state.duration_us);
  return format;
}

bool HasCurrentSample(const aim::MediaExtractorState& state) {
  return state.selected_track == 0 && state.sample_index < state.samples.size();
}

}  // namespace aim_media_extractor_detail

extern "C" AMediaDataSource* AMediaDataSource_new() {
  return new (std::nothrow) AMediaDataSource();
}

extern "C" void AMediaDataSource_delete(AMediaDataSource* source) {
  if (source == nullptr) return;
  aim_media_extractor_detail::CloseDataSource(source);
  delete source;
}

extern "C" void AMediaDataSource_setUserdata(AMediaDataSource* source,
                                               void* userdata) {
  if (source == nullptr) return;
  std::lock_guard<std::mutex> lock(source->mutex);
  if (!source->closed) source->userdata = userdata;
}

extern "C" void AMediaDataSource_setReadAt(AMediaDataSource* source,
                                             AMediaDataSourceReadAt callback) {
  if (source == nullptr) return;
  std::lock_guard<std::mutex> lock(source->mutex);
  if (!source->closed) source->read_at = callback;
}

extern "C" void AMediaDataSource_setGetSize(
    AMediaDataSource* source, AMediaDataSourceGetSize callback) {
  if (source == nullptr) return;
  std::lock_guard<std::mutex> lock(source->mutex);
  if (!source->closed) source->get_size = callback;
}

extern "C" void AMediaDataSource_setClose(AMediaDataSource* source,
                                            AMediaDataSourceClose callback) {
  if (source == nullptr) return;
  std::lock_guard<std::mutex> lock(source->mutex);
  if (!source->closed) source->close = callback;
}

extern "C" void AMediaDataSource_setGetAvailableSize(
    AMediaDataSource* source, AMediaDataSourceGetAvailableSize callback) {
  if (source == nullptr) return;
  std::lock_guard<std::mutex> lock(source->mutex);
  if (!source->closed) source->get_available_size = callback;
}

extern "C" void AMediaDataSource_close(AMediaDataSource* source) {
  aim_media_extractor_detail::CloseDataSource(source);
}

extern "C" AMediaExtractor* AMediaExtractor_new() {
  return new (std::nothrow) AMediaExtractor();
}

extern "C" media_status_t AMediaExtractor_delete(AMediaExtractor* extractor) {
  if (extractor == nullptr) return AMEDIA_ERROR_INVALID_OBJECT;
  delete extractor;
  return AMEDIA_OK;
}

extern "C" media_status_t AMediaExtractor_setDataSourceFd(
    AMediaExtractor* extractor, int fd, int64_t offset, int64_t length) {
  if (extractor == nullptr) return AMEDIA_ERROR_INVALID_OBJECT;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  aim_media_extractor_detail::Reset(&extractor->state);
  std::vector<uint8_t> bytes;
  if (!aim_media_extractor_detail::ReadFd(
          fd, static_cast<int64_t>(offset), static_cast<int64_t>(length),
          &bytes) ||
      !aim_media_extractor_detail::ParseBytes(bytes,
                                                     &extractor->state)) {
    return AMEDIA_ERROR_MALFORMED;
  }
  return AMEDIA_OK;
}

extern "C" media_status_t AMediaExtractor_setDataSource(
    AMediaExtractor* extractor, const char* location) {
  if (extractor == nullptr) return AMEDIA_ERROR_INVALID_OBJECT;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  aim_media_extractor_detail::Reset(&extractor->state);
  std::vector<uint8_t> bytes;
  if (!aim_media_extractor_detail::ReadPath(location, &bytes) ||
      !aim_media_extractor_detail::ParseBytes(bytes,
                                                     &extractor->state)) {
    return AMEDIA_ERROR_MALFORMED;
  }
  return AMEDIA_OK;
}

extern "C" media_status_t AMediaExtractor_setDataSourceCustom(
    AMediaExtractor* extractor, AMediaDataSource* source) {
  if (extractor == nullptr) return AMEDIA_ERROR_INVALID_OBJECT;
  if (source == nullptr) return AMEDIA_ERROR_INVALID_PARAMETER;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  aim_media_extractor_detail::Reset(&extractor->state);
  std::vector<uint8_t> bytes;
  if (!aim_media_extractor_detail::ReadCustom(source, &bytes) ||
      !aim_media_extractor_detail::ParseBytes(bytes,
                                                     &extractor->state)) {
    return AMEDIA_ERROR_MALFORMED;
  }
  return AMEDIA_OK;
}

extern "C" size_t AMediaExtractor_getTrackCount(AMediaExtractor* extractor) {
  if (extractor == nullptr) return 0;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  return extractor->state.has_source ? 1u : 0u;
}

extern "C" AMediaFormat* AMediaExtractor_getTrackFormat(
    AMediaExtractor* extractor, size_t index) {
  if (extractor == nullptr || index != 0) return nullptr;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  return aim_media_extractor_detail::MakeFormat(extractor->state);
}

extern "C" media_status_t AMediaExtractor_selectTrack(
    AMediaExtractor* extractor, size_t index) {
  if (extractor == nullptr) return AMEDIA_ERROR_INVALID_OBJECT;
  if (index != 0) return AMEDIA_ERROR_INVALID_PARAMETER;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  if (!extractor->state.has_source) return AMEDIA_ERROR_INVALID_OPERATION;
  extractor->state.selected_track = 0;
  extractor->state.sample_index = 0;
  return AMEDIA_OK;
}

extern "C" media_status_t AMediaExtractor_unselectTrack(
    AMediaExtractor* extractor, size_t index) {
  if (extractor == nullptr) return AMEDIA_ERROR_INVALID_OBJECT;
  if (index != 0) return AMEDIA_ERROR_INVALID_PARAMETER;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  extractor->state.selected_track = -1;
  return AMEDIA_OK;
}

extern "C" ssize_t AMediaExtractor_readSampleData(AMediaExtractor* extractor,
                                                    uint8_t* buffer,
                                                    size_t capacity) {
  if (extractor == nullptr) return -1;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  if (buffer == nullptr || !aim_media_extractor_detail::HasCurrentSample(
                                extractor->state))
    return -1;
  const auto& sample = extractor->state.samples[extractor->state.sample_index];
  if (sample.data.size() > capacity) return -1;
  if (!sample.data.empty())
    std::memcpy(buffer, sample.data.data(), sample.data.size());
  return static_cast<ssize_t>(sample.data.size());
}

extern "C" uint32_t AMediaExtractor_getSampleFlags(
    AMediaExtractor* extractor) {
  if (extractor == nullptr) return 0;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  return aim_media_extractor_detail::HasCurrentSample(extractor->state)
             ? extractor->state.samples[extractor->state.sample_index].flags
             : 0;
}

extern "C" int AMediaExtractor_getSampleTrackIndex(
    AMediaExtractor* extractor) {
  if (extractor == nullptr) return -1;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  return aim_media_extractor_detail::HasCurrentSample(extractor->state)
             ? 0
             : -1;
}

extern "C" int64_t AMediaExtractor_getSampleTime(
    AMediaExtractor* extractor) {
  if (extractor == nullptr) return -1;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  return aim_media_extractor_detail::HasCurrentSample(extractor->state)
             ? extractor->state.samples[extractor->state.sample_index].pts_us
             : -1;
}

extern "C" bool AMediaExtractor_advance(AMediaExtractor* extractor) {
  if (extractor == nullptr) return false;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  if (!aim_media_extractor_detail::HasCurrentSample(extractor->state))
    return false;
  if (extractor->state.sample_index + 1 >= extractor->state.samples.size()) {
    extractor->state.sample_index = extractor->state.samples.size();
    return false;
  }
  ++extractor->state.sample_index;
  return true;
}

extern "C" media_status_t AMediaExtractor_seekTo(AMediaExtractor* extractor,
                                                  int64_t seek_us, int mode) {
  if (extractor == nullptr) return AMEDIA_ERROR_INVALID_OBJECT;
  if (mode < 0 || mode > 2) return AMEDIA_ERROR_INVALID_PARAMETER;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  if (!extractor->state.has_source) return AMEDIA_ERROR_INVALID_OPERATION;
  size_t next = extractor->state.samples.size();
  for (size_t i = 0; i < extractor->state.samples.size(); ++i) {
    const auto& sample = extractor->state.samples[i];
    if (sample.pts_us >= seek_us && (sample.flags & 1u) != 0) {
      next = i;
      break;
    }
  }
  size_t previous = 0;
  bool found_previous = false;
  for (size_t i = 0; i < extractor->state.samples.size(); ++i) {
    const auto& sample = extractor->state.samples[i];
    if (sample.pts_us > seek_us) break;
    if ((sample.flags & 1u) != 0) {
      previous = i;
      found_previous = true;
    }
  }
  if (!found_previous) previous = 0;
  if (mode == 0) {
    extractor->state.sample_index = previous;
  } else if (mode == 1) {
    extractor->state.sample_index = next;
  } else if (next == extractor->state.samples.size()) {
    extractor->state.sample_index = previous;
  } else if (!found_previous ||
             seek_us - extractor->state.samples[previous].pts_us <=
                 extractor->state.samples[next].pts_us - seek_us) {
    extractor->state.sample_index = previous;
  } else {
    extractor->state.sample_index = next;
  }
  return AMEDIA_OK;
}

extern "C" ssize_t AMediaExtractor_getSampleSize(AMediaExtractor* extractor) {
  if (extractor == nullptr) return -1;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  return aim_media_extractor_detail::HasCurrentSample(extractor->state)
             ? static_cast<ssize_t>(extractor->state
                                        .samples[extractor->state.sample_index]
                                        .data.size())
             : -1;
}

extern "C" int64_t AMediaExtractor_getCachedDuration(
    AMediaExtractor* extractor) {
  if (extractor == nullptr) return -1;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  return extractor->state.has_source ? extractor->state.duration_us : -1;
}

extern "C" AMediaFormat* AMediaExtractor_getFileFormat(
    AMediaExtractor* extractor) {
  if (extractor == nullptr) return nullptr;
  std::lock_guard<std::mutex> lock(extractor->mutex);
  return aim_media_extractor_detail::MakeFormat(extractor->state);
}
