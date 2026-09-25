#include "darwin_android_media_ndk.h"
#include "media/format.h"
#include "media/image_reader_ndk.h"

#include "darwin_vp9_decoder.h"

#include <CoreMedia/CoreMedia.h>
#include <media/NdkMediaFormat.h>
#include <VideoToolbox/VideoToolbox.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <cstdlib>
#include <deque>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>
#include <sys/types.h>

struct AMediaCodec {
  std::mutex mutex;
  std::string name;
  std::string mime;
  bool configured = false;
  bool started = false;
  bool input_owned = false;
  bool output_owned = false;
  bool output_format_pending = false;
  bool eos = false;
  int32_t width = 0;
  int32_t height = 0;
  darwin_art::Vp9Decoder vp9;
  std::vector<uint8_t> input;
  // The dequeued slot retains full capacity across zero-sized EOS buffers.
  // Android reports valid bytes in BufferInfo, not in getOutputBuffer capacity.
  std::vector<uint8_t> output_buffer;
  std::deque<darwin_art::DecodedVideoFrame> output;
  CMVideoFormatDescriptionRef format = nullptr;
  VTDecompressionSessionRef session = nullptr;
};
struct AMediaCodecBufferInfo {
  int32_t offset;
  int32_t size;
  int64_t presentationTimeUs;
  uint32_t flags;
};
struct AMediaCodecOnAsyncNotifyCallback {
  void (*on_async_input_available)(AMediaCodec*, void*, int32_t);
  void (*on_async_output_available)(AMediaCodec*, void*, int32_t, void*);
  void (*on_async_format_changed)(AMediaCodec*, void*, AMediaFormat*);
  void (*on_async_error)(AMediaCodec*, void*, int32_t, int32_t, const char*);
};
struct ANativeWindow;
struct AMediaCrypto;

namespace {

constexpr int32_t kMediaErrorUnsupported = -1010;
constexpr int32_t kMediaErrorInvalidObject = -10000;


}  // namespace

#include "darwin_media_extractor_ndk.inc"

std::vector<uint8_t> StripCodecStartCode(const std::vector<uint8_t>& value) {
  size_t offset = 0;
  if (value.size() >= 4 && value[0] == 0 && value[1] == 0 && value[2] == 0 &&
      value[3] == 1) offset = 4;
  else if (value.size() >= 3 && value[0] == 0 && value[1] == 0 && value[2] == 1)
    offset = 3;
  return std::vector<uint8_t>(value.begin() + offset, value.end());
}

void NdkDecodeCallback(void* refcon, void*, OSStatus status, VTDecodeInfoFlags,
                       CVImageBufferRef image, CMTime pts, CMTime) {
  auto* codec = static_cast<AMediaCodec*>(refcon);
  if (codec == nullptr || status != noErr || image == nullptr) return;
  auto pixel = static_cast<CVPixelBufferRef>(image);
  CVPixelBufferLockBaseAddress(pixel, kCVPixelBufferLock_ReadOnly);
  const size_t width = CVPixelBufferGetWidth(pixel);
  const size_t height = CVPixelBufferGetHeight(pixel);
  std::vector<uint8_t> frame(width * height * 3 / 2);
  if (CVPixelBufferGetPlaneCount(pixel) >= 2) {
    auto* y = static_cast<uint8_t*>(CVPixelBufferGetBaseAddressOfPlane(pixel, 0));
    auto* uv = static_cast<uint8_t*>(CVPixelBufferGetBaseAddressOfPlane(pixel, 1));
    const size_t y_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel, 0);
    const size_t uv_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel, 1);
    for (size_t row = 0; row < height; ++row)
      std::memcpy(frame.data() + row * width, y + row * y_stride, width);
    for (size_t row = 0; row < height / 2; ++row)
      std::memcpy(frame.data() + width * height + row * width,
                  uv + row * uv_stride, width);
  } else {
    frame.clear();
  }
  CVPixelBufferUnlockBaseAddress(pixel, kCVPixelBufferLock_ReadOnly);
  if (!frame.empty()) {
    std::lock_guard<std::mutex> lock(codec->mutex);
    darwin_art::DecodedVideoFrame decoded;
    decoded.bytes = std::move(frame);
    decoded.width = width;
    decoded.height = height;
    decoded.pts_us = CMTIME_IS_VALID(pts) ? CMTimeGetSeconds(pts) * 1000000 : 0;
    codec->output.push_back(std::move(decoded));
  }
}

extern "C" AMediaCodec* AMediaCodec_createCodecByName(const char* name) {
  if (name == nullptr || (std::strcmp(name, "c2.darwin.avc.decoder") != 0 &&
                          std::strcmp(name, "c2.darwin.hevc.decoder") != 0 &&
                          std::strcmp(name, "c2.darwin.vp9.decoder") != 0))
    return nullptr;
  auto* codec = new (std::nothrow) AMediaCodec();
  if (codec != nullptr) codec->name = name;
  return codec;
}

extern "C" AMediaCodec* AMediaCodec_createDecoderByType(const char* mime) {
  if (mime == nullptr) return nullptr;
  if (std::strcmp(mime, "video/hevc") == 0)
    return AMediaCodec_createCodecByName("c2.darwin.hevc.decoder");
  if (std::strcmp(mime, "video/avc") == 0)
    return AMediaCodec_createCodecByName("c2.darwin.avc.decoder");
  if (std::strcmp(mime, "video/x-vnd.on2.vp9") == 0)
    return AMediaCodec_createCodecByName("c2.darwin.vp9.decoder");
  if (std::getenv("DARWIN_ART_DEBUG_MEDIA_CODEC"))
    fprintf(stderr, "ART NDK MediaCodec: no decoder for mime=%s\n", mime);
  return nullptr;
}

extern "C" AMediaCodec* AMediaCodec_createEncoderByType(const char*) {
  return nullptr;
}

extern "C" int32_t AMediaCodec_delete(AMediaCodec* codec) {
  if (codec == nullptr) return kMediaErrorInvalidObject;
  if (codec->session != nullptr) {
    VTDecompressionSessionWaitForAsynchronousFrames(codec->session);
    VTDecompressionSessionInvalidate(codec->session);
    CFRelease(codec->session);
  }
  if (codec->format != nullptr) CFRelease(codec->format);
  delete codec;
  return 0;
}
extern "C" int32_t AMediaCodec_configure(AMediaCodec* codec,
                                          const AMediaFormat* format,
                                          ANativeWindow* surface,
                                          AMediaCrypto* crypto,
                                          uint32_t flags) {
  if (codec == nullptr || format == nullptr) return kMediaErrorInvalidObject;
  if (surface != nullptr || crypto != nullptr || flags != 0 || codec->configured) {
    return kMediaErrorUnsupported;
  }
  auto* input_format = const_cast<AMediaFormat*>(format);
  const char* mime = nullptr;
  int32_t width = 0;
  int32_t height = 0;
  if (!AMediaFormat_getString(input_format, "mime", &mime) ||
      !AMediaFormat_getInt32(input_format, "width", &width) ||
      !AMediaFormat_getInt32(input_format, "height", &height) ||
      width <= 0 || height <= 0 || width > 16384 || height > 16384)
    return kMediaErrorUnsupported;
  codec->width = width;
  codec->height = height;
  codec->mime = mime;
  if (codec->mime == "video/x-vnd.on2.vp9") {
    if (!codec->vp9.Initialize(codec->width, codec->height)) return kMediaErrorUnsupported;
    codec->input.resize(4 * 1024 * 1024);
    codec->output_buffer.resize(size_t(codec->width) * codec->height +
        2 * size_t((codec->width + 1) / 2) * ((codec->height + 1) / 2));
    codec->configured = true;
    if (std::getenv("DARWIN_ART_DEBUG_MEDIA_CODEC")) {
      fprintf(stderr, "ART NDK MediaCodec: VP9 configured %dx%d\n", codec->width, codec->height);
    }
    return 0;
  }
  void* sps_data = nullptr;
  void* pps_data = nullptr;
  void* vps_data = nullptr;
  size_t sps_size = 0, pps_size = 0, vps_size = 0;
  const bool hevc = codec->name.find("hevc") != std::string::npos;
  if (!AMediaFormat_getBuffer(input_format, "csd-0", &sps_data, &sps_size) ||
      !AMediaFormat_getBuffer(input_format, "csd-1", &pps_data, &pps_size) ||
      (hevc && !AMediaFormat_getBuffer(input_format, "csd-2", &vps_data, &vps_size)))
    return kMediaErrorUnsupported;
  auto copy_parameter_set = [](void* data, size_t size) {
    const auto* bytes = static_cast<const uint8_t*>(data);
    return size == 0 ? std::vector<uint8_t>{}
                     : std::vector<uint8_t>(bytes, bytes + size);
  };
  const auto sps = StripCodecStartCode(copy_parameter_set(sps_data, sps_size));
  const auto pps = StripCodecStartCode(copy_parameter_set(pps_data, pps_size));
  OSStatus status = noErr;
  if (!hevc) {
    const uint8_t* sets[] = {sps.data(), pps.data()};
    size_t sizes[] = {sps.size(), pps.size()};
    status = CMVideoFormatDescriptionCreateFromH264ParameterSets(
        kCFAllocatorDefault, 2, sets, sizes, 4, &codec->format);
  } else {
    const auto vps = StripCodecStartCode(copy_parameter_set(vps_data, vps_size));
    const uint8_t* sets[] = {vps.data(), sps.data(), pps.data()};
    size_t sizes[] = {vps.size(), sps.size(), pps.size()};
    status = CMVideoFormatDescriptionCreateFromHEVCParameterSets(
        kCFAllocatorDefault, 3, sets, sizes, 4, nullptr, &codec->format);
  }
  if (status != noErr) return kMediaErrorUnsupported;
  VTDecompressionOutputCallbackRecord callback = {
      .decompressionOutputCallback = &NdkDecodeCallback,
      .decompressionOutputRefCon = codec};
  status = VTDecompressionSessionCreate(kCFAllocatorDefault, codec->format,
                                         nullptr, nullptr, &callback,
                                         &codec->session);
  if (status != noErr) return kMediaErrorUnsupported;
  codec->input.resize(4 * 1024 * 1024);
  codec->output_buffer.resize(size_t(codec->width) * codec->height * 3 / 2);
  codec->configured = true;
  return 0;
}
extern "C" int32_t AMediaCodec_start(AMediaCodec* codec) {
  if (codec == nullptr) return kMediaErrorInvalidObject;
  if (!codec->configured) return kMediaErrorUnsupported;
  codec->started = true;
  codec->output_format_pending = true;
  return 0;
}
extern "C" int32_t AMediaCodec_stop(AMediaCodec* codec) {
  if (codec == nullptr) return kMediaErrorInvalidObject;
  codec->started = false;
  return 0;
}
extern "C" int32_t AMediaCodec_flush(AMediaCodec* codec) {
  if (codec == nullptr) return kMediaErrorInvalidObject;
  if (codec->session) {
    VTDecompressionSessionFinishDelayedFrames(codec->session);
    VTDecompressionSessionWaitForAsynchronousFrames(codec->session);
  }
  std::lock_guard<std::mutex> lock(codec->mutex);
  codec->output.clear();
  codec->input_owned = codec->output_owned = codec->eos = false;
  if (codec->mime == "video/x-vnd.on2.vp9" && !codec->vp9.Initialize(codec->width, codec->height)) {
    return kMediaErrorUnsupported;
  }
  return 0;
}
extern "C" int32_t AMediaCodec_setAsyncNotifyCallback(
    AMediaCodec* codec, AMediaCodecOnAsyncNotifyCallback, void*) {
  return codec == nullptr ? kMediaErrorInvalidObject : kMediaErrorUnsupported;
}
extern "C" int32_t AMediaCodec_setParameters(AMediaCodec* codec,
                                              const AMediaFormat*) {
  return codec == nullptr ? kMediaErrorInvalidObject : kMediaErrorUnsupported;
}
extern "C" uint8_t* AMediaCodec_getInputBuffer(AMediaCodec* codec, size_t index,
                                                 size_t* size) {
  if (size != nullptr) *size = 0;
  if (codec == nullptr || index != 0 || !codec->started) return nullptr;
  if (size != nullptr) *size = codec->input.size();
  return codec->input.data();
}
extern "C" uint8_t* AMediaCodec_getOutputBuffer(AMediaCodec* codec, size_t index,
                                                  size_t* size) {
  if (size != nullptr) *size = 0;
  if (codec == nullptr || index != 0) return nullptr;
  std::lock_guard<std::mutex> lock(codec->mutex);
  if (!codec->output_owned || codec->output.empty()) return nullptr;
  if (size != nullptr) *size = codec->output_buffer.size();
  return codec->output_buffer.data();
}
extern "C" AMediaFormat* AMediaCodec_getInputFormat(AMediaCodec*) {
  return AMediaFormat_new();
}
extern "C" AMediaFormat* AMediaCodec_getOutputFormat(AMediaCodec* codec) {
  if (!codec) return nullptr;
  std::lock_guard<std::mutex> lock(codec->mutex);
  auto* format = AMediaFormat_new();
  if (format == nullptr) return nullptr;
  AMediaFormat_setString(format, "mime", "video/raw");
  const std::pair<const char*, int32_t> fields[] = {
      {"width", codec->width}, {"height", codec->height},
      {"stride", codec->width}, {"slice-height", codec->height},
      {"color-format", codec->mime == "video/x-vnd.on2.vp9" ? 0x13 : 0x15},
      {"crop-left", 0}, {"crop-top", 0}, {"crop-right", codec->width - 1},
      {"crop-bottom", codec->height - 1}, {"color-range", 2},
      {"color-standard", 1}, {"color-transfer", 3}, {"rotation-degrees", 0}};
  for (const auto& [name, value] : fields) AMediaFormat_setInt32(format, name, value);
  return format;
}
extern "C" AMediaFormat* AMediaCodec_getBufferFormat(AMediaCodec* codec, size_t) {
  return AMediaCodec_getOutputFormat(codec);
}
extern "C" ssize_t AMediaCodec_dequeueInputBuffer(AMediaCodec* codec, int64_t) {
  if (!codec) return -1;
  std::lock_guard<std::mutex> lock(codec->mutex);
  if (!codec->started || codec->input_owned || codec->eos || codec->output.size() >= 4) return -1;
  codec->input_owned = true;
  return 0;
}
extern "C" ssize_t AMediaCodec_dequeueOutputBuffer(
    AMediaCodec* codec, AMediaCodecBufferInfo* info, int64_t) {
  if (codec == nullptr || info == nullptr) return -1;
  std::lock_guard<std::mutex> lock(codec->mutex);
  if (!codec->started || codec->output_owned || codec->output.empty()) return -1;
  const auto& next = codec->output.front();
  if (next.width > 0 && next.height > 0 &&
      (next.width != codec->width || next.height != codec->height)) {
    codec->width = next.width;
    codec->height = next.height;
    codec->output_format_pending = true;
  }
  if (codec->output_format_pending) {
    codec->output_format_pending = false;
    return -2;
  }
  codec->output_owned = true;
  info->offset = 0;
  info->size = static_cast<int32_t>(codec->output.front().bytes.size());
  info->presentationTimeUs = codec->output.front().pts_us;
  info->flags = codec->output.front().flags;
  if (!codec->output.front().bytes.empty()) {
    codec->output_buffer.swap(codec->output.front().bytes);
  }
  if (std::getenv("DARWIN_ART_DEBUG_MEDIA_CODEC")) {
    fprintf(stderr, "ART NDK MediaCodec: output size=%d pts=%lld flags=%u\n",
            info->size, static_cast<long long>(info->presentationTimeUs), info->flags);
  }
  return 0;
}
extern "C" int32_t AMediaCodec_queueInputBuffer(AMediaCodec* codec,
                                                 size_t index,
                                                 size_t offset,
                                                 size_t size,
                                                 uint64_t pts,
                                                 uint32_t flags) {
  if (codec == nullptr) return kMediaErrorInvalidObject;
  if (index != 0 || !codec->started || offset > codec->input.size() ||
      size > codec->input.size() - offset) return kMediaErrorUnsupported;
  {
    std::lock_guard<std::mutex> lock(codec->mutex);
    if (!codec->input_owned || codec->eos) return kMediaErrorUnsupported;
    codec->input_owned = false;
    codec->eos = (flags & 4) != 0;
    if (codec->mime == "video/x-vnd.on2.vp9") {
      if (!codec->vp9.Decode(codec->input.data() + offset, size, pts, &codec->output)) {
        return kMediaErrorUnsupported;
      }
      if (codec->eos) {
        darwin_art::DecodedVideoFrame eos;
        eos.pts_us = pts;
        eos.flags = 4;
        codec->output.emplace_back(std::move(eos));
      }
      return 0;
    }
  }
  if (!codec->session) return kMediaErrorUnsupported;
  CMBlockBufferRef block = nullptr;
  OSStatus status = CMBlockBufferCreateWithMemoryBlock(
      kCFAllocatorDefault, nullptr, size, kCFAllocatorDefault, nullptr, 0,
      size, 0, &block);
  if (status == kCMBlockBufferNoErr)
    status = CMBlockBufferReplaceDataBytes(codec->input.data() + offset, block,
                                            0, size);
  if (status != kCMBlockBufferNoErr) return kMediaErrorUnsupported;
  CMSampleTimingInfo timing = {kCMTimeInvalid, CMTimeMake(pts, 1000000),
                               kCMTimeInvalid};
  CMSampleBufferRef sample = nullptr;
  status = CMSampleBufferCreateReady(kCFAllocatorDefault, block, codec->format,
                                      1, 1, &timing, 0, nullptr, &sample);
  if (status == noErr) {
    VTDecompressionSessionDecodeFrame(codec->session, sample, 0, nullptr,
                                      nullptr);
    CFRelease(sample);
  }
  CFRelease(block);
  return status == noErr ? 0 : kMediaErrorUnsupported;
}
extern "C" int32_t AMediaCodec_releaseOutputBuffer(AMediaCodec* codec,
                                                    size_t index,
                                                    bool) {
  if (codec == nullptr) return kMediaErrorInvalidObject;
  if (index != 0) return kMediaErrorUnsupported;
  std::lock_guard<std::mutex> lock(codec->mutex);
  if (!codec->output_owned || codec->output.empty()) return kMediaErrorUnsupported;
  codec->output.pop_front();
  codec->output_owned = false;
  return 0;
}
extern "C" int32_t AMediaCodec_releaseOutputBufferAtTime(AMediaCodec* codec,
                                                          size_t index, int64_t) {
  return AMediaCodec_releaseOutputBuffer(codec, index, true);
}
extern "C" int32_t AMediaCodec_getName(AMediaCodec* codec, char** name) {
  if (name == nullptr) return kMediaErrorInvalidObject;
  *name = nullptr;
  if (codec == nullptr) return kMediaErrorInvalidObject;
  *name = static_cast<char*>(std::malloc(codec->name.size() + 1));
  if (*name == nullptr) return -12;
  std::memcpy(*name, codec->name.c_str(), codec->name.size() + 1);
  return 0;
}
extern "C" void AMediaCodec_releaseName(AMediaCodec*, char* name) {
  std::free(name);
}

void* darwin_art_android_media_ndk_symbol(const char* symbol) {
  if (symbol == nullptr) return nullptr;
  if (void* address = darwin_art::media::FormatSymbol(symbol)) return address;
  if (void* address = darwin_art::media::ImageReaderSymbol(symbol)) return address;
#define MEDIA_FUNCTION(name)                         \
  if (std::strcmp(symbol, #name) == 0) {             \
    return reinterpret_cast<void*>(&name);           \
  }
  MEDIA_FUNCTION(AMediaCodec_configure)
  MEDIA_FUNCTION(AMediaCodec_createDecoderByType)
  MEDIA_FUNCTION(AMediaCodec_createCodecByName)
  MEDIA_FUNCTION(AMediaCodec_createEncoderByType)
  MEDIA_FUNCTION(AMediaCodec_delete)
  MEDIA_FUNCTION(AMediaCodec_dequeueInputBuffer)
  MEDIA_FUNCTION(AMediaCodec_dequeueOutputBuffer)
  MEDIA_FUNCTION(AMediaCodec_flush)
  MEDIA_FUNCTION(AMediaCodec_getBufferFormat)
  MEDIA_FUNCTION(AMediaCodec_getInputBuffer)
  MEDIA_FUNCTION(AMediaCodec_getInputFormat)
  MEDIA_FUNCTION(AMediaCodec_getName)
  MEDIA_FUNCTION(AMediaCodec_getOutputBuffer)
  MEDIA_FUNCTION(AMediaCodec_getOutputFormat)
  MEDIA_FUNCTION(AMediaCodec_queueInputBuffer)
  MEDIA_FUNCTION(AMediaCodec_releaseName)
  MEDIA_FUNCTION(AMediaCodec_releaseOutputBuffer)
  MEDIA_FUNCTION(AMediaCodec_releaseOutputBufferAtTime)
  MEDIA_FUNCTION(AMediaCodec_setAsyncNotifyCallback)
  MEDIA_FUNCTION(AMediaCodec_setParameters)
  MEDIA_FUNCTION(AMediaCodec_start)
  MEDIA_FUNCTION(AMediaCodec_stop)
  MEDIA_FUNCTION(AMediaExtractor_new)
  MEDIA_FUNCTION(AMediaExtractor_delete)
  MEDIA_FUNCTION(AMediaExtractor_setDataSourceFd)
  MEDIA_FUNCTION(AMediaExtractor_setDataSource)
  MEDIA_FUNCTION(AMediaExtractor_setDataSourceCustom)
  MEDIA_FUNCTION(AMediaExtractor_getTrackCount)
  MEDIA_FUNCTION(AMediaExtractor_getTrackFormat)
  MEDIA_FUNCTION(AMediaExtractor_selectTrack)
  MEDIA_FUNCTION(AMediaExtractor_unselectTrack)
  MEDIA_FUNCTION(AMediaExtractor_readSampleData)
  MEDIA_FUNCTION(AMediaExtractor_getSampleFlags)
  MEDIA_FUNCTION(AMediaExtractor_getSampleTrackIndex)
  MEDIA_FUNCTION(AMediaExtractor_getSampleTime)
  MEDIA_FUNCTION(AMediaExtractor_advance)
  MEDIA_FUNCTION(AMediaExtractor_seekTo)
  MEDIA_FUNCTION(AMediaDataSource_new)
  MEDIA_FUNCTION(AMediaDataSource_delete)
  MEDIA_FUNCTION(AMediaDataSource_setUserdata)
  MEDIA_FUNCTION(AMediaDataSource_setReadAt)
  MEDIA_FUNCTION(AMediaDataSource_setGetSize)
  MEDIA_FUNCTION(AMediaDataSource_setClose)
  MEDIA_FUNCTION(AMediaDataSource_setGetAvailableSize)
#undef MEDIA_FUNCTION
  return nullptr;
}
