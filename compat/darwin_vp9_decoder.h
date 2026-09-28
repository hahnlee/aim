#pragma once

#include <vpx/vp8dx.h>
#include <vpx/vpx_decoder.h>
#include <dlfcn.h>
#include <cstdint>
#include <cstring>
#include <deque>
#include <mutex>
#include <vector>

namespace aim {

struct DecodedVideoFrame {
  std::vector<uint8_t> bytes;
  int32_t width = 0;
  int32_t height = 0;
  int64_t pts_us = 0;
  int32_t flags = 0;
};

// Shared native decoder owner used by both Java MediaCodec and libmediandk.
// Frames own tightly packed I420 bytes; callers retain dequeued frames until
// releaseOutputBuffer, independently of subsequent compressed input.
class Vp9Decoder {
 public:
  Vp9Decoder() = default;
  Vp9Decoder(const Vp9Decoder&) = delete;
  Vp9Decoder& operator=(const Vp9Decoder&) = delete;
  ~Vp9Decoder() { if (initialized_) Api().destroy(&context_); }

  bool Initialize(int width, int height) {
    if (width <= 0 || height <= 0 || width > 16384 || height > 16384) return false;
    std::lock_guard<std::mutex> lock(mutex_);
    auto& api = Api();
    if (!api.handle) return false;
    if (initialized_) {
      api.destroy(&context_);
      initialized_ = false;
      context_ = {};
    }
    vpx_codec_dec_cfg_t config{};
    config.threads = 2;
    config.w = width;
    config.h = height;
    initialized_ = api.init(&context_, api.iface(), &config, 0,
                            VPX_DECODER_ABI_VERSION) == VPX_CODEC_OK;
    return initialized_;
  }

  bool Decode(const uint8_t* bytes, size_t size, int64_t pts,
              std::deque<DecodedVideoFrame>* frames) {
    std::lock_guard<std::mutex> lock(mutex_);
    auto& api = Api();
    if (!initialized_ || frames == nullptr || size > UINT32_MAX) return false;
    if (size == 0) return true;
    if (api.decode(&context_, bytes, static_cast<unsigned>(size), nullptr, 0) != VPX_CODEC_OK) {
      return false;
    }
    vpx_codec_iter_t iterator = nullptr;
    while (vpx_image_t* image = api.get_frame(&context_, &iterator)) {
      if (image->fmt != VPX_IMG_FMT_I420 || image->d_w == 0 || image->d_h == 0 ||
          !image->planes[0] || !image->planes[1] || !image->planes[2]) return false;
      DecodedVideoFrame frame;
      frame.width = image->d_w;
      frame.height = image->d_h;
      frame.pts_us = pts;
      const size_t widths[] = {image->d_w, (image->d_w + 1) / 2, (image->d_w + 1) / 2};
      const size_t heights[] = {image->d_h, (image->d_h + 1) / 2, (image->d_h + 1) / 2};
      frame.bytes.resize(widths[0] * heights[0] + 2 * widths[1] * heights[1]);
      size_t offset = 0;
      for (size_t plane = 0; plane < 3; ++plane) {
        for (size_t row = 0; row < heights[plane]; ++row) {
          std::memcpy(frame.bytes.data() + offset + row * widths[plane],
                      image->planes[plane] + row * image->stride[plane], widths[plane]);
        }
        offset += widths[plane] * heights[plane];
      }
      frames->emplace_back(std::move(frame));
    }
    return true;
  }

 private:
  struct VpxApi {
    void* handle = nullptr;
    decltype(&vpx_codec_vp9_dx) iface = nullptr;
    decltype(&vpx_codec_dec_init_ver) init = nullptr;
    decltype(&vpx_codec_decode) decode = nullptr;
    decltype(&vpx_codec_get_frame) get_frame = nullptr;
    decltype(&vpx_codec_destroy) destroy = nullptr;
  };
  static VpxApi& Api() {
    static VpxApi api = [] {
      VpxApi value;
      value.handle = dlopen("/opt/homebrew/opt/libvpx/lib/libvpx.dylib", RTLD_LAZY | RTLD_LOCAL);
      if (!value.handle) return value;
#define VPX_LOAD(field, symbol) value.field = reinterpret_cast<decltype(value.field)>(dlsym(value.handle, #symbol))
      VPX_LOAD(iface, vpx_codec_vp9_dx);
      VPX_LOAD(init, vpx_codec_dec_init_ver);
      VPX_LOAD(decode, vpx_codec_decode);
      VPX_LOAD(get_frame, vpx_codec_get_frame);
      VPX_LOAD(destroy, vpx_codec_destroy);
#undef VPX_LOAD
      if (!value.iface || !value.init || !value.decode || !value.get_frame || !value.destroy) {
        dlclose(value.handle);
        return VpxApi{};
      }
      return value;
    }();
    return api;
  }
  std::mutex mutex_;
  vpx_codec_ctx_t context_{};
  bool initialized_ = false;
};

}  // namespace aim
