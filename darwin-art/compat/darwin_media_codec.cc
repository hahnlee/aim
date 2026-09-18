#include "darwin_media_codec.h"

#include "darwin_angle_egl.h"
#include "darwin_vp9_decoder.h"

#include <CoreMedia/CoreMedia.h>
#include <CoreVideo/CoreVideo.h>
#include <VideoToolbox/VideoToolbox.h>
#include <vpx/vp8dx.h>
#include <vpx/vpx_decoder.h>

#include <dlfcn.h>

#include <algorithm>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <deque>
#include <iterator>
#include <iostream>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

namespace {

constexpr int32_t kInfoTryAgainLater = -1;
constexpr int32_t kInfoOutputFormatChanged = -2;
constexpr int32_t kInfoOutputBuffersChanged = -3;
constexpr int32_t kBufferFlagEndOfStream = 4;
constexpr int32_t kColorFormatSurface = 0x7f000789;
constexpr int32_t kColorFormatYuv420Flexible = 0x7f420888;
constexpr int32_t kColorFormatYuv420Planar = 0x13;

struct CodecDescription {
  const char* name;
  const char* mime;
  int32_t flags;
};

constexpr CodecDescription kCodecs[] = {
    {"c2.darwin.vp9.decoder", "video/x-vnd.on2.vp9", 8},
    {"c2.darwin.avc.decoder", "video/avc", 8},
    {"c2.darwin.hevc.decoder", "video/hevc", 8},
};

using DecodedFrame = darwin_art::DecodedVideoFrame;

struct DarwinMediaCodec {
  std::mutex mutex;
  std::string mime;
  std::string codec_name;
  bool encoder = false;
  bool configured = false;
  bool started = false;
  bool eos = false;
  bool output_format_pending = false;
  bool output_buffers_pending = false;
  bool input_owned = false;
  bool output_owned = false;
  int32_t width = 0;
  int32_t height = 0;
  std::vector<uint8_t> input;
  std::vector<uint8_t> output_scratch;
  std::deque<DecodedFrame> output;
  // Strong native producer reference. VideoToolbox callbacks run without a
  // JNIEnv, so retaining a Java Surface global and resolving it from that
  // thread cannot implement Android's output-surface contract.
  void* output_window = nullptr;
  CMVideoFormatDescriptionRef format = nullptr;
  VTDecompressionSessionRef session = nullptr;
  darwin_art::Vp9Decoder vp9;
  bool vp9_initialized = false;
};

std::mutex g_registry_mutex;

bool DebugMediaCodec() {
  return std::getenv("DARWIN_ART_DEBUG_MEDIA_CODEC") != nullptr;
}

void Throw(JNIEnv* env, const char* class_name, const char* message) {
  if (env == nullptr || env->ExceptionCheck()) return;
  jclass klass = env->FindClass(class_name);
  if (klass != nullptr) {
    env->ThrowNew(klass, message);
    env->DeleteLocalRef(klass);
  }
}

jfieldID NativeContextField(JNIEnv* env, jobject codec) {
  if (env == nullptr || codec == nullptr) return nullptr;
  jclass klass = env->GetObjectClass(codec);
  if (klass == nullptr) return nullptr;
  jfieldID field = env->GetFieldID(klass, "mNativeContext", "J");
  env->DeleteLocalRef(klass);
  return field;
}

DarwinMediaCodec* GetCodec(JNIEnv* env, jobject codec) {
  jfieldID field = NativeContextField(env, codec);
  if (field == nullptr) return nullptr;
  return reinterpret_cast<DarwinMediaCodec*>(
      static_cast<uintptr_t>(env->GetLongField(codec, field)));
}

void SetCodec(JNIEnv* env, jobject codec, DarwinMediaCodec* state) {
  jfieldID field = NativeContextField(env, codec);
  if (field != nullptr) {
    env->SetLongField(codec, field,
                      static_cast<jlong>(reinterpret_cast<uintptr_t>(state)));
  }
}

void CopyPlane(const uint8_t* source, size_t source_stride, uint8_t* dest,
               size_t dest_stride, size_t rows, size_t width) {
  for (size_t row = 0; row < rows; ++row) {
    std::memcpy(dest + row * dest_stride, source + row * source_stride, width);
  }
}

// Take a strong producer snapshot while holding the codec lock, then perform
// the potentially blocking ANativeWindow lock/post operation without it. This
// matches the AOSP ownership boundary: setOutputSurface() may replace the
// codec's producer concurrently, but cannot retire the producer used by an
// in-flight render.
void PublishSurface(DarwinMediaCodec* codec, const DecodedFrame& frame) {
  // Surface output is optional. The ByteBuffer path remains canonical, while
  // this bridge publishes a tightly packed RGBA frame through the existing
  // ANativeWindow lock/post contract when a Java Surface was supplied.
  if (codec == nullptr || frame.width <= 0 || frame.height <= 0 || frame.bytes.size() <
                                static_cast<size_t>(frame.width) * frame.height * 3 / 2) {
    return;
  }
  void* window = nullptr;
  {
    std::lock_guard<std::mutex> lock(codec->mutex);
    window = codec->output_window;
    if (window != nullptr) darwin_art_android_ANativeWindow_acquire(window);
  }
  if (window == nullptr) return;
  struct NativeBuffer {
    int32_t width;
    int32_t height;
    int32_t stride;
    int32_t format;
    void* bits;
    uint32_t reserved[6];
  } buffer{};
  if (darwin_art_android_ANativeWindow_setBuffersGeometry(
          window, frame.width, frame.height,
          /*AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM=*/1) != 0 ||
      darwin_art_android_ANativeWindow_lock(window, &buffer, nullptr) != 0) {
    darwin_art_android_ANativeWindow_release(window);
    return;
  }
  if (buffer.bits == nullptr) {
    (void)darwin_art_android_ANativeWindow_unlockAndPost(window);
    darwin_art_android_ANativeWindow_release(window);
    return;
  }
  const uint8_t* y_plane = frame.bytes.data();
  const uint8_t* uv_plane = y_plane + static_cast<size_t>(frame.width) * frame.height;
  auto* rgba = static_cast<uint8_t*>(buffer.bits);
  const int32_t stride = buffer.stride > 0 ? buffer.stride : frame.width;
  for (int32_t y = 0; y < frame.height; ++y) {
    for (int32_t x = 0; x < frame.width; ++x) {
      const int32_t yv = y_plane[y * frame.width + x];
      const int32_t uv = (y / 2) * frame.width + (x & ~1);
      const int32_t u = uv_plane[uv] - 128;
      const int32_t v = uv_plane[uv + 1] - 128;
      const int32_t r = std::clamp(yv + ((1436 * v) >> 10), 0, 255);
      const int32_t g = std::clamp(yv - ((352 * u + 731 * v) >> 10), 0, 255);
      const int32_t b = std::clamp(yv + ((1815 * u) >> 10), 0, 255);
      uint8_t* pixel = rgba + (static_cast<size_t>(y) * stride + x) * 4;
      pixel[0] = static_cast<uint8_t>(r);
      pixel[1] = static_cast<uint8_t>(g);
      pixel[2] = static_cast<uint8_t>(b);
      pixel[3] = 255;
    }
  }
  darwin_art_android_ANativeWindow_unlockAndPost(window);
  darwin_art_android_ANativeWindow_release(window);
}

void DecompressionCallback(void* refcon, void*, OSStatus status,
                           VTDecodeInfoFlags, CVImageBufferRef image,
                           CMTime pts, CMTime) {
  auto* codec = static_cast<DarwinMediaCodec*>(refcon);
  if (codec == nullptr || status != noErr || image == nullptr) return;
  auto* pixel = static_cast<CVPixelBufferRef>(image);
  CVPixelBufferLockBaseAddress(pixel, kCVPixelBufferLock_ReadOnly);
  const size_t width = CVPixelBufferGetWidth(pixel);
  const size_t height = CVPixelBufferGetHeight(pixel);
  DecodedFrame frame;
  frame.width = static_cast<int32_t>(width);
  frame.height = static_cast<int32_t>(height);
  frame.pts_us = CMTIME_IS_VALID(pts) && pts.timescale != 0
                    ? static_cast<int64_t>(CMTimeGetSeconds(pts) * 1000000.0)
                    : 0;
  frame.bytes.resize(width * height * 3 / 2);
  uint8_t* y = static_cast<uint8_t*>(CVPixelBufferGetBaseAddressOfPlane(pixel, 0));
  uint8_t* uv = static_cast<uint8_t*>(CVPixelBufferGetBaseAddressOfPlane(pixel, 1));
  if (y != nullptr && uv != nullptr && CVPixelBufferGetPlaneCount(pixel) >= 2) {
    CopyPlane(y, CVPixelBufferGetBytesPerRowOfPlane(pixel, 0), frame.bytes.data(),
              width, height, width);
    CopyPlane(uv, CVPixelBufferGetBytesPerRowOfPlane(pixel, 1),
              frame.bytes.data() + width * height, width, height / 2, width);
  } else {
    frame.bytes.clear();
  }
  CVPixelBufferUnlockBaseAddress(pixel, kCVPixelBufferLock_ReadOnly);
  if (frame.bytes.empty()) return;
  std::lock_guard<std::mutex> lock(codec->mutex);
  codec->output.emplace_back(std::move(frame));
}

void DestroyCodec(DarwinMediaCodec* codec) {
  if (codec == nullptr) return;
  if (codec->session != nullptr) {
    VTDecompressionSessionWaitForAsynchronousFrames(codec->session);
    VTDecompressionSessionInvalidate(codec->session);
    CFRelease(codec->session);
  }
  if (codec->format != nullptr) CFRelease(codec->format);
  if (codec->output_window != nullptr) {
    darwin_art_android_ANativeWindow_release(codec->output_window);
    codec->output_window = nullptr;
  }
  delete codec;
}

bool ReadString(JNIEnv* env, jobject value, std::string* out) {
  if (env == nullptr || value == nullptr || out == nullptr) return false;
  auto* string = static_cast<jstring>(value);
  const char* chars = env->GetStringUTFChars(string, nullptr);
  if (chars == nullptr) return false;
  *out = chars;
  env->ReleaseStringUTFChars(string, chars);
  return true;
}

bool IsByteArray(JNIEnv* env, jobject value) {
  if (env == nullptr || value == nullptr) return false;
  jclass bytes = env->FindClass("[B");
  if (bytes == nullptr) return false;
  const bool result = env->IsInstanceOf(value, bytes);
  env->DeleteLocalRef(bytes);
  return result;
}

std::vector<uint8_t> StripH264StartCode(const std::vector<uint8_t>& input) {
  size_t offset = 0;
  if (input.size() >= 4 && input[0] == 0 && input[1] == 0 && input[2] == 0 &&
      input[3] == 1) {
    offset = 4;
  } else if (input.size() >= 3 && input[0] == 0 && input[1] == 0 &&
             input[2] == 1) {
    offset = 3;
  }
  return std::vector<uint8_t>(input.begin() + offset, input.end());
}

std::vector<uint8_t> NormalizeH264AccessUnit(const uint8_t* data, size_t size) {
  std::vector<uint8_t> input(data, data + size);
  size_t first = std::string::npos;
  for (size_t i = 0; i + 3 < input.size(); ++i) {
    if (input[i] == 0 && input[i + 1] == 0 &&
        ((input[i + 2] == 1) ||
         (input[i + 2] == 0 && input[i + 3] == 1))) {
      first = i;
      break;
    }
  }
  if (first == std::string::npos) return input;
  std::vector<uint8_t> output;
  size_t cursor = first;
  while (cursor < input.size()) {
    size_t start = cursor + 3;
    if (cursor + 3 < input.size() && input[cursor + 2] == 0) start = cursor + 4;
    size_t next = input.size();
    for (size_t i = start; i + 3 < input.size(); ++i) {
      if (input[i] == 0 && input[i + 1] == 0 &&
          (input[i + 2] == 1 ||
           (input[i + 2] == 0 && input[i + 3] == 1))) {
        next = i;
        break;
      }
    }
    if (next > start) {
      const uint32_t length = static_cast<uint32_t>(next - start);
      output.push_back(static_cast<uint8_t>(length >> 24));
      output.push_back(static_cast<uint8_t>(length >> 16));
      output.push_back(static_cast<uint8_t>(length >> 8));
      output.push_back(static_cast<uint8_t>(length));
      output.insert(output.end(), input.begin() + start, input.begin() + next);
    }
    cursor = next;
    if (cursor == input.size()) break;
  }
  return output.empty() ? input : output;
}

bool GetInt(JNIEnv* env, jobject value, int32_t* result) {
  if (env == nullptr || value == nullptr || result == nullptr) return false;
  jclass integer = env->FindClass("java/lang/Integer");
  if (integer == nullptr) return false;
  jmethodID method = env->GetMethodID(integer, "intValue", "()I");
  *result = method == nullptr ? 0 : env->CallIntMethod(value, method);
  env->DeleteLocalRef(integer);
  return method != nullptr && !env->ExceptionCheck();
}

void MediaCodecNativeInit(JNIEnv*, jclass) {}

void MediaCodecNativeSetup(JNIEnv* env, jobject self, jstring name,
                           jboolean name_is_type, jboolean encoder, jint, jint) {
  auto* state = new DarwinMediaCodec();
  state->encoder = encoder == JNI_TRUE;
  std::string value;
  if (name != nullptr) ReadString(env, name, &value);
  state->mime = name_is_type == JNI_TRUE ? value : "";
  state->codec_name = value;
  if (name_is_type == JNI_TRUE) {
    const auto found = std::find_if(
        std::begin(kCodecs), std::end(kCodecs),
        [&value](const CodecDescription& codec) { return value == codec.mime; });
    state->codec_name = found == std::end(kCodecs) ? "" : found->name;
  }
  if (DebugMediaCodec()) {
    std::cerr << "ART Android MediaCodec: setup value=" << value
              << " name_is_type=" << (name_is_type == JNI_TRUE)
              << " codec=" << state->codec_name << " mime=" << state->mime
              << " encoder=" << state->encoder << "\n";
  }
  SetCodec(env, self, state);
}

void MediaCodecNativeConfigure(JNIEnv* env, jobject self, jobjectArray keys,
                               jobjectArray values, jobject surface, jobject,
                               jobject, jint) {
  auto* codec = GetCodec(env, self);
  if (codec == nullptr) {
    Throw(env, "java/lang/IllegalStateException", "codec is not initialized");
    return;
  }
  void* requested_window =
      surface == nullptr
          ? nullptr
          : darwin_art_android_ANativeWindow_fromSurface(env, surface);
  if (surface != nullptr && requested_window == nullptr) {
    Throw(env, "java/lang/IllegalArgumentException",
          "output Surface has no native producer");
    return;
  }
  std::lock_guard<std::mutex> lock(codec->mutex);
  const jsize count = keys == nullptr ? 0 : env->GetArrayLength(keys);
  std::vector<uint8_t> sps;
  std::vector<uint8_t> pps;
  std::vector<uint8_t> vps;
  for (jsize i = 0; i < count; ++i) {
    auto key = static_cast<jstring>(env->GetObjectArrayElement(keys, i));
    auto value = env->GetObjectArrayElement(values, i);
    if (key == nullptr) continue;
    const char* name = env->GetStringUTFChars(key, nullptr);
    if (name == nullptr) continue;
    if (std::strcmp(name, "mime") == 0) ReadString(env, value, &codec->mime);
    else if (std::strcmp(name, "width") == 0) GetInt(env, value, &codec->width);
    else if (std::strcmp(name, "height") == 0) GetInt(env, value, &codec->height);
    else if (std::strcmp(name, "csd-0") == 0 && IsByteArray(env, value)) {
      jbyteArray bytes = static_cast<jbyteArray>(value);
      const jsize size = env->GetArrayLength(bytes);
      sps.resize(size);
      env->GetByteArrayRegion(bytes, 0, size,
                              reinterpret_cast<jbyte*>(sps.data()));
      sps = StripH264StartCode(sps);
    } else if (std::strcmp(name, "csd-1") == 0 && IsByteArray(env, value)) {
      jbyteArray bytes = static_cast<jbyteArray>(value);
      const jsize size = env->GetArrayLength(bytes);
      pps.resize(size);
      env->GetByteArrayRegion(bytes, 0, size,
                              reinterpret_cast<jbyte*>(pps.data()));
      pps = StripH264StartCode(pps);
    } else if (std::strcmp(name, "csd-2") == 0 && IsByteArray(env, value)) {
      jbyteArray bytes = static_cast<jbyteArray>(value);
      const jsize size = env->GetArrayLength(bytes);
      vps.resize(size);
      env->GetByteArrayRegion(bytes, 0, size,
                              reinterpret_cast<jbyte*>(vps.data()));
      vps = StripH264StartCode(vps);
    }
    env->ReleaseStringUTFChars(key, name);
    env->DeleteLocalRef(key);
    env->DeleteLocalRef(value);
  }
  void* previous_window = codec->output_window;
  codec->output_window = requested_window;
  if (previous_window != nullptr) {
    darwin_art_android_ANativeWindow_release(previous_window);
  }
  if (DebugMediaCodec()) {
    std::cerr << "ART Android MediaCodec: configure codec="
              << codec->codec_name << " mime=" << codec->mime << " size="
              << codec->width << "x" << codec->height
              << " surface=" << (surface != nullptr) << "\n";
  }
  const bool is_avc = codec->mime == "video/avc" && !sps.empty() && !pps.empty();
  const bool is_hevc = codec->mime == "video/hevc" && !sps.empty() &&
                       !pps.empty() && !vps.empty();
  const bool is_vp9 = codec->mime == "video/x-vnd.on2.vp9" &&
                      codec->width > 0 && codec->height > 0;
  if (!is_avc && !is_hevc && !is_vp9) {
    Throw(env, "java/lang/IllegalArgumentException",
          "Darwin MediaCodec requires a supported video format");
    return;
  }
  OSStatus status = noErr;
  if (is_avc) {
    const uint8_t* parameter_sets[] = {sps.data(), pps.data()};
    size_t parameter_sizes[] = {sps.size(), pps.size()};
    status = CMVideoFormatDescriptionCreateFromH264ParameterSets(
        kCFAllocatorDefault, 2, parameter_sets, parameter_sizes, 4,
        &codec->format);
  } else if (is_hevc) {
    const uint8_t* parameter_sets[] = {vps.data(), sps.data(), pps.data()};
    size_t parameter_sizes[] = {vps.size(), sps.size(), pps.size()};
    status = CMVideoFormatDescriptionCreateFromHEVCParameterSets(
        kCFAllocatorDefault, 3, parameter_sets, parameter_sizes, 4, nullptr,
        &codec->format);
  } else if (is_vp9) {
    if (!codec->vp9.Initialize(codec->width, codec->height)) {
      Throw(env, "java/lang/UnsupportedOperationException", "VP9 decoder initialization failed");
      return;
    }
    codec->vp9_initialized = true;
    codec->input.resize(4 * 1024 * 1024);
    codec->output_scratch.resize(static_cast<size_t>(codec->width) * codec->height * 3 / 2);
    if (DebugMediaCodec()) std::cerr << "ART Android MediaCodec: VP9 output scratch="
                                     << static_cast<const void*>(codec->output_scratch.data())
                                     << " size=" << codec->output_scratch.size() << "\n";
    codec->configured = true;
    return;
  }
  if (status != noErr) {
    Throw(env, "java/lang/IllegalArgumentException", "invalid H.264 codec config");
    return;
  }
  VTDecompressionOutputCallbackRecord callback = {
      .decompressionOutputCallback = &DecompressionCallback,
      .decompressionOutputRefCon = codec,
  };
  status = VTDecompressionSessionCreate(kCFAllocatorDefault, codec->format,
                                        nullptr, nullptr, &callback,
                                        &codec->session);
  if (status != noErr) {
    Throw(env, "java/lang/UnsupportedOperationException",
          "VideoToolbox cannot create an H.264 decoder");
    return;
  }
  codec->input.resize(4 * 1024 * 1024);
  codec->output_scratch.resize(static_cast<size_t>(codec->width) * codec->height * 3 / 2);
  if (DebugMediaCodec()) std::cerr << "ART Android MediaCodec: output scratch="
                                   << static_cast<const void*>(codec->output_scratch.data())
                                   << " size=" << codec->output_scratch.size() << "\n";
  codec->configured = true;
}

void MediaCodecNativeStart(JNIEnv* env, jobject self) {
  auto* codec = GetCodec(env, self);
  if (codec == nullptr || !codec->configured) {
    Throw(env, "java/lang/IllegalStateException", "codec is not configured");
    return;
  }
  codec->started = true;
  codec->output_format_pending = true;
  codec->output_buffers_pending = true;
}

void MediaCodecNativeStop(JNIEnv* env, jobject self) {
  auto* codec = GetCodec(env, self);
  if (codec != nullptr) codec->started = false;
}

void MediaCodecNativeRelease(JNIEnv* env, jobject self) {
  auto* codec = GetCodec(env, self);
  if (codec == nullptr) return;
  SetCodec(env, self, nullptr);
  DestroyCodec(codec);
}

void MediaCodecNativeFinalize(JNIEnv* env, jobject self) {
  MediaCodecNativeRelease(env, self);
}

void MediaCodecNativeReset(JNIEnv* env, jobject self) {
  auto* codec = GetCodec(env, self);
  if (codec == nullptr) return;
  if (codec->session != nullptr) {
    VTDecompressionSessionFinishDelayedFrames(codec->session);
    VTDecompressionSessionWaitForAsynchronousFrames(codec->session);
  }
  std::lock_guard<std::mutex> lock(codec->mutex);
  codec->output.clear();
  codec->eos = false;
  codec->input_owned = false;
  codec->output_owned = false;
}

jint MediaCodecNativeDequeueInput(JNIEnv* env, jobject self, jlong) {
  auto* codec = GetCodec(env, self);
  if (codec == nullptr) return kInfoTryAgainLater;
  std::lock_guard<std::mutex> lock(codec->mutex);
  if (!codec->started || codec->eos || codec->input_owned || codec->output.size() >= 4) {
    return kInfoTryAgainLater;
  }
  codec->input_owned = true;
  return 0;
}

jint MediaCodecNativeDequeueOutput(JNIEnv* env, jobject self, jobject info, jlong) {
  auto* codec = GetCodec(env, self);
  if (codec == nullptr || info == nullptr) return kInfoTryAgainLater;
  std::lock_guard<std::mutex> lock(codec->mutex);
  if (!codec->started || codec->output_owned || codec->output.empty()) {
    return kInfoTryAgainLater;
  }
  if (codec->output_buffers_pending) {
    codec->output_buffers_pending = false;
    if (DebugMediaCodec()) std::cerr << "ART Android MediaCodec: output buffers changed\n";
    return kInfoOutputBuffersChanged;
  }
  if (codec->output_format_pending) {
    codec->output_format_pending = false;
    if (DebugMediaCodec()) std::cerr << "ART Android MediaCodec: output format changed\n";
    return kInfoOutputFormatChanged;
  }
  const auto& frame = codec->output.front();
  if (frame.bytes.size() > codec->output_scratch.size()) {
    Throw(env, "java/lang/IllegalStateException", "decoded frame exceeds output buffer");
    return kInfoTryAgainLater;
  }
  if (!frame.bytes.empty()) {
    std::memcpy(codec->output_scratch.data(), frame.bytes.data(), frame.bytes.size());
  }
  codec->output_owned = true;
  jclass klass = env->GetObjectClass(info);
  jfieldID offset = env->GetFieldID(klass, "offset", "I");
  jfieldID size = env->GetFieldID(klass, "size", "I");
  jfieldID pts = env->GetFieldID(klass, "presentationTimeUs", "J");
  jfieldID flags = env->GetFieldID(klass, "flags", "I");
  env->SetIntField(info, offset, 0);
  env->SetIntField(info, size, static_cast<jint>(frame.bytes.size()));
  env->SetLongField(info, pts, static_cast<jlong>(frame.pts_us));
  env->SetIntField(info, flags, frame.flags);
  env->DeleteLocalRef(klass);
  return 0;
}

jobject MediaCodecGetBuffer(JNIEnv* env, jobject self, jboolean input,
                             jint index) {
  auto* codec = GetCodec(env, self);
  if (codec == nullptr || index != 0) return nullptr;
  std::lock_guard<std::mutex> lock(codec->mutex);
  if (input == JNI_TRUE) {
    return env->NewDirectByteBuffer(codec->input.data(), codec->input.size());
  }
  if (codec->output.empty()) {
    return codec->output_scratch.empty()
               ? nullptr
               : env->NewDirectByteBuffer(codec->output_scratch.data(),
                                           codec->output_scratch.size());
  }
  // Android's Java wrapper may retain the array returned by getOutputBuffers
  // across dequeues.  Expose one stable backing store rather than a deque
  // element that can be popped/reallocated when the next frame arrives.
  return env->NewDirectByteBuffer(codec->output_scratch.data(),
                                  codec->output_scratch.size());
}

uint8_t g_empty_output_buffer = 0;

jobject NewFormatMap(JNIEnv* env, DarwinMediaCodec* codec) {
  jclass map = env->FindClass("java/util/HashMap");
  if (map == nullptr) return nullptr;
  jobject result = env->NewObject(map, env->GetMethodID(map, "<init>", "()V"));
  jmethodID put = env->GetMethodID(map, "put",
                                   "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;");
  if (result != nullptr && put != nullptr && codec != nullptr) {
    jstring mime = env->NewStringUTF(codec->mime.c_str());
    jstring mime_key = env->NewStringUTF("mime");
    env->CallObjectMethod(result, put, mime_key, mime);
    env->DeleteLocalRef(mime_key);
    env->DeleteLocalRef(mime);
    if (codec->width > 0 && codec->height > 0) {
      jclass integer = env->FindClass("java/lang/Integer");
      jmethodID value_of = integer == nullptr ? nullptr
          : env->GetStaticMethodID(integer, "valueOf", "(I)Ljava/lang/Integer;");
      if (value_of != nullptr) {
        jstring key = env->NewStringUTF("width");
        jobject value = env->CallStaticObjectMethod(integer, value_of, codec->width);
        env->CallObjectMethod(result, put, key, value);
        env->DeleteLocalRef(key);
        env->DeleteLocalRef(value);
        key = env->NewStringUTF("height");
        value = env->CallStaticObjectMethod(integer, value_of, codec->height);
        env->CallObjectMethod(result, put, key, value);
        env->DeleteLocalRef(key);
        env->DeleteLocalRef(value);
      }
      env->DeleteLocalRef(integer);
      const char* keys[] = {"color-format", "stride", "slice-height", "crop-left",
                            "crop-top", "crop-right", "crop-bottom", "color-range",
                            "color-standard", "color-transfer", "rotation-degrees",
                            "max-input-size", "max-width", "max-height"};
      const jint values[] = {kColorFormatYuv420Planar, codec->width, codec->height,
                             0, 0, codec->width - 1, codec->height - 1, 2, 1, 3, 0,
                             static_cast<jint>(codec->input.size()), codec->width,
                             codec->height};
      for (size_t i = 0; i < std::size(keys); ++i) {
        jclass integer_type = env->FindClass("java/lang/Integer");
        jmethodID ctor = integer_type == nullptr ? nullptr
            : env->GetMethodID(integer_type, "<init>", "(I)V");
        jstring key = env->NewStringUTF(keys[i]);
        jobject value = ctor == nullptr ? nullptr : env->NewObject(integer_type, ctor, values[i]);
        env->CallObjectMethod(result, put, key, value);
        env->DeleteLocalRef(key);
        env->DeleteLocalRef(value);
        env->DeleteLocalRef(integer_type);
      }
      jclass floating = env->FindClass("java/lang/Float");
      jmethodID float_ctor = floating == nullptr ? nullptr
          : env->GetMethodID(floating, "<init>", "(F)V");
      jstring frame_rate_key = env->NewStringUTF("frame-rate");
      jobject frame_rate = float_ctor == nullptr ? nullptr
          : env->NewObject(floating, float_ctor, 30.0f);
      env->CallObjectMethod(result, put, frame_rate_key, frame_rate);
      env->DeleteLocalRef(frame_rate_key);
      env->DeleteLocalRef(frame_rate);
      env->DeleteLocalRef(floating);
    }
  }
  env->DeleteLocalRef(map);
  return result;
}

jobject MediaCodecGetFormatNative(JNIEnv* env, jobject self, jboolean) {
  auto* codec = GetCodec(env, self);
  return NewFormatMap(env, codec);
}

jobject MediaCodecGetOutputFormatNative(JNIEnv* env, jobject self, jint) {
  auto* codec = GetCodec(env, self);
  return NewFormatMap(env, codec);
}

jobjectArray MediaCodecGetBuffers(JNIEnv* env, jobject self, jboolean input) {
  jclass buffer = env->FindClass("java/nio/ByteBuffer");
  if (buffer == nullptr) return nullptr;
  jobjectArray result = env->NewObjectArray(1, buffer, nullptr);
  jobject value = MediaCodecGetBuffer(env, self, input, 0);
  if (value == nullptr && input == JNI_FALSE) {
    value = env->NewDirectByteBuffer(&g_empty_output_buffer, 1);
  }
  if (value != nullptr) env->SetObjectArrayElement(result, 0, value);
  env->DeleteLocalRef(value);
  env->DeleteLocalRef(buffer);
  return result;
}

void MediaCodecQueueInput(JNIEnv* env, jobject self, jint index, jint offset,
                          jint size, jlong pts_us, jint flags) {
  auto* codec = GetCodec(env, self);
  if (codec == nullptr || index != 0 || !codec->started ||
      (codec->session == nullptr && !codec->vp9_initialized)) {
    Throw(env, "java/lang/IllegalStateException", "codec is not started");
    return;
  }
  if (offset < 0 || size < 0 || static_cast<size_t>(offset + size) > codec->input.size()) {
    Throw(env, "java/lang/IllegalArgumentException", "input buffer range is invalid");
    return;
  }
  {
    std::lock_guard<std::mutex> lock(codec->mutex);
    codec->input_owned = false;
    if ((flags & kBufferFlagEndOfStream) != 0) codec->eos = true;
  }
  if (size == 0) {
    if ((flags & kBufferFlagEndOfStream) != 0) {
      if (codec->session != nullptr) {
        VTDecompressionSessionFinishDelayedFrames(codec->session);
        VTDecompressionSessionWaitForAsynchronousFrames(codec->session);
      }
      std::lock_guard<std::mutex> lock(codec->mutex);
      DecodedFrame eos;
      eos.pts_us = pts_us;
      eos.flags = kBufferFlagEndOfStream;
      codec->output.emplace_back(std::move(eos));
    }
    return;
  }
  if (codec->vp9_initialized) {
    if (DebugMediaCodec() && size > 0) {
      std::cerr << "ART Android MediaCodec: VP9 input size=" << size << " first="
                << std::hex << static_cast<int>(codec->input[offset]) << " "
                << static_cast<int>(codec->input[offset + (size > 1 ? 1 : 0)])
                << std::dec << " pts=" << pts_us << "\n";
    }
    std::lock_guard<std::mutex> lock(codec->mutex);
    if (!codec->vp9.Decode(codec->input.data() + offset, size, pts_us, &codec->output)) {
      Throw(env, "java/lang/IllegalStateException", "VP9 decode failed");
      return;
    }
    return;
  }
  CMBlockBufferRef block = nullptr;
  const std::vector<uint8_t> access_unit = NormalizeH264AccessUnit(
      codec->input.data() + offset, static_cast<size_t>(size));
  OSStatus status = CMBlockBufferCreateWithMemoryBlock(
      kCFAllocatorDefault, nullptr, access_unit.size(), kCFAllocatorDefault,
      nullptr, 0, access_unit.size(), 0, &block);
  if (status == kCMBlockBufferNoErr) {
    status = CMBlockBufferReplaceDataBytes(access_unit.data(), block, 0,
                                            access_unit.size());
  }
  if (status != kCMBlockBufferNoErr) {
    Throw(env, "java/lang/IllegalStateException", "cannot create media block");
    return;
  }
  CMSampleTimingInfo timing = {
      .duration = kCMTimeInvalid,
      .presentationTimeStamp = CMTimeMake(pts_us, 1000000),
      .decodeTimeStamp = kCMTimeInvalid,
  };
  CMSampleBufferRef sample = nullptr;
  status = CMSampleBufferCreateReady(kCFAllocatorDefault, block, codec->format,
                                     1, 1, &timing, 0, nullptr, &sample);
  if (status == noErr) {
    VTDecodeFrameFlags decode_flags = 0;
    VTDecompressionSessionDecodeFrame(codec->session, sample, decode_flags,
                                      nullptr, nullptr);
    CFRelease(sample);
  }
  CFRelease(block);
  if (status != noErr) {
    Throw(env, "java/lang/IllegalStateException", "VideoToolbox decode failed");
  }
}

void MediaCodecReleaseOutput(JNIEnv* env, jobject self, jint index, jboolean render,
                             jboolean, jlong) {
  auto* codec = GetCodec(env, self);
  if (codec == nullptr || index != 0) return;
  DecodedFrame frame;
  bool publish = false;
  {
    std::lock_guard<std::mutex> lock(codec->mutex);
    if (codec->output_owned && !codec->output.empty()) {
      if (render == JNI_TRUE) {
        frame = std::move(codec->output.front());
        publish = true;
      }
      codec->output.pop_front();
      codec->output_owned = false;
    }
  }
  if (publish) PublishSurface(codec, frame);
}

void MediaCodecNativeConfigureNoop(JNIEnv*, jobject) {}
void MediaCodecNativeSetSurface(JNIEnv* env, jobject self, jobject surface) {
  auto* codec = GetCodec(env, self);
  if (codec == nullptr || surface == nullptr) {
    Throw(env, "java/lang/IllegalArgumentException",
          "codec and output Surface are required");
    return;
  }
  void* replacement =
      darwin_art_android_ANativeWindow_fromSurface(env, surface);
  if (replacement == nullptr) {
    Throw(env, "java/lang/IllegalArgumentException",
          "output Surface has no native producer");
    return;
  }
  void* previous = nullptr;
  {
    std::lock_guard<std::mutex> lock(codec->mutex);
    if (!codec->configured || codec->output_window == nullptr) {
      darwin_art_android_ANativeWindow_release(replacement);
      Throw(env, "java/lang/IllegalStateException",
            "codec is not configured for surface output");
      return;
    }
    previous = codec->output_window;
    codec->output_window = replacement;
  }
  darwin_art_android_ANativeWindow_release(previous);
}
void MediaCodecNativeSetCallback(JNIEnv*, jobject, jobject) {}
void MediaCodecNativeSetParameters(JNIEnv*, jobject, jobjectArray, jobjectArray) {}
void MediaCodecNativeSetAudioPresentation(JNIEnv*, jobject, jint, jint) {}
void MediaCodecNativeSetVideoScalingMode(JNIEnv*, jobject, jint) {}
void MediaCodecSignalEndOfInputStream(JNIEnv*, jobject) {}

void MediaCodecNativeCloseMediaImage(JNIEnv*, jclass, jlong) {}

bool Register(JNIEnv* env, const char* name, JNINativeMethod* methods,
              jint count) {
  jclass klass = env->FindClass(name);
  if (klass == nullptr) return false;
  const bool result = env->RegisterNatives(klass, methods, count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return result;
}

jobject MakeCapabilities(JNIEnv* env, const CodecDescription& codec,
                          const char* mime) {
  jclass caps = env->FindClass("android/media/MediaCodecInfo$CodecCapabilities");
  if (caps == nullptr) return nullptr;
  jmethodID constructor = env->GetMethodID(caps, "<init>", "()V");
  jobject result = constructor == nullptr ? nullptr : env->NewObject(caps, constructor);
  if (result == nullptr) {
    env->DeleteLocalRef(caps);
    return nullptr;
  }
  jfieldID mime_field = env->GetFieldID(caps, "mMime", "Ljava/lang/String;");
  jfieldID profile_levels = env->GetFieldID(caps, "profileLevels", "[Landroid/media/MediaCodecInfo$CodecProfileLevel;");
  jfieldID color_formats = env->GetFieldID(caps, "colorFormats", "[I");
  jfieldID flags = env->GetFieldID(caps, "mFlagsSupported", "I");
  jfieldID max_instances = env->GetFieldID(caps, "mMaxSupportedInstances", "I");
  jfieldID default_format = env->GetFieldID(caps, "mDefaultFormat", "Landroid/media/MediaFormat;");
  jfieldID info_format = env->GetFieldID(caps, "mCapabilitiesInfo", "Landroid/media/MediaFormat;");
  env->SetObjectField(result, mime_field, env->NewStringUTF(mime));
  env->SetIntField(result, flags, 1 | 128 | 256);
  env->SetIntField(result, max_instances, 4);
  jclass format_class = env->FindClass("android/media/MediaFormat");
  jmethodID format_ctor = format_class == nullptr ? nullptr : env->GetMethodID(format_class, "<init>", "()V");
  jobject format = format_ctor == nullptr ? nullptr : env->NewObject(format_class, format_ctor);
  if (format != nullptr) {
    jmethodID set_string = env->GetMethodID(format_class, "setString", "(Ljava/lang/String;Ljava/lang/String;)V");
    if (set_string != nullptr) {
      jstring key = env->NewStringUTF("mime");
      jstring value = env->NewStringUTF(mime);
      env->CallVoidMethod(format, set_string, key, value);
      env->DeleteLocalRef(key);
      env->DeleteLocalRef(value);
    }
    env->SetObjectField(result, default_format, format);
    env->SetObjectField(result, info_format, format);
  }
  env->DeleteLocalRef(format);
  env->DeleteLocalRef(format_class);
  jintArray colors = env->NewIntArray(3);
  const jint color_values[] = {kColorFormatSurface, kColorFormatYuv420Planar,
                               kColorFormatYuv420Flexible};
  env->SetIntArrayRegion(colors, 0, 3, color_values);
  env->SetObjectField(result, color_formats, colors);
  jclass profile = env->FindClass("android/media/MediaCodecInfo$CodecProfileLevel");
  jobjectArray levels = env->NewObjectArray(1, profile, nullptr);
  jobject level = env->NewObject(profile, env->GetMethodID(profile, "<init>", "()V"));
  env->SetIntField(level, env->GetFieldID(profile, "profile", "I"), 1);
  env->SetIntField(level, env->GetFieldID(profile, "level", "I"), 256);
  env->SetObjectArrayElement(levels, 0, level);
  env->SetObjectField(result, profile_levels, levels);
  env->DeleteLocalRef(level);
  env->DeleteLocalRef(levels);
  env->DeleteLocalRef(profile);
  env->DeleteLocalRef(colors);
  env->DeleteLocalRef(caps);
  static_cast<void>(codec);
  return result;
}

jint MediaCodecListCount(JNIEnv*, jclass) {
  if (DebugMediaCodec()) {
    std::cerr << "ART Android MediaCodecList: count=" << std::size(kCodecs)
              << "\n";
  }
  return static_cast<jint>(std::size(kCodecs));
}
jint MediaCodecListFind(JNIEnv* env, jclass, jstring name) {
  if (name == nullptr) return -1;
  const char* value = env->GetStringUTFChars(name, nullptr);
  if (value == nullptr) return -1;
  for (size_t i = 0; i < std::size(kCodecs); ++i) {
    if (std::strcmp(value, kCodecs[i].name) == 0) {
      env->ReleaseStringUTFChars(name, value);
      return static_cast<jint>(i);
    }
  }
  env->ReleaseStringUTFChars(name, value);
  return -1;
}
jint MediaCodecListAttributes(JNIEnv*, jclass, jint index) {
  return index >= 0 && static_cast<size_t>(index) < std::size(kCodecs)
             ? kCodecs[index].flags
             : 0;
}
jstring MediaCodecListName(JNIEnv* env, jclass, jint index) {
  return index >= 0 && static_cast<size_t>(index) < std::size(kCodecs)
             ? env->NewStringUTF(kCodecs[index].name)
             : nullptr;
}
jstring MediaCodecListCanonicalName(JNIEnv* env, jclass, jint index) {
  return MediaCodecListName(env, nullptr, index);
}
jobjectArray MediaCodecListTypes(JNIEnv* env, jclass, jint index) {
  jclass string = env->FindClass("java/lang/String");
  if (string == nullptr) return nullptr;
  jobjectArray result = env->NewObjectArray(1, string, nullptr);
  if (index >= 0 && static_cast<size_t>(index) < std::size(kCodecs)) {
    env->SetObjectArrayElement(result, 0, env->NewStringUTF(kCodecs[index].mime));
  }
  env->DeleteLocalRef(string);
  return result;
}
jobject MediaCodecListCapabilities(JNIEnv* env, jclass, jint index, jstring type) {
  if (index < 0 || static_cast<size_t>(index) >= std::size(kCodecs) || type == nullptr)
    return nullptr;
  const char* value = env->GetStringUTFChars(type, nullptr);
  const bool match = value != nullptr && std::strcmp(value, kCodecs[index].mime) == 0;
  if (DebugMediaCodec()) {
    std::cerr << "ART Android MediaCodecList: capabilities index=" << index
              << " requested=" << (value == nullptr ? "<null>" : value)
              << " codec=" << kCodecs[index].name << " match=" << match
              << "\n";
  }
  if (value != nullptr) env->ReleaseStringUTFChars(type, value);
  return match ? MakeCapabilities(env, kCodecs[index], kCodecs[index].mime) : nullptr;
}
jobject MediaCodecListGlobalSettings(JNIEnv* env, jclass) {
  jclass map = env->FindClass("java/util/HashMap");
  if (map == nullptr) return nullptr;
  jobject result = env->NewObject(map, env->GetMethodID(map, "<init>", "()V"));
  env->DeleteLocalRef(map);
  return result;
}

}  // namespace

namespace darwin_art {

bool RegisterDarwinMediaCodecNatives(JNIEnv* env) {
  JNINativeMethod codec_methods[] = {
      {const_cast<char*>("getBuffer"), const_cast<char*>("(ZI)Ljava/nio/ByteBuffer;"), reinterpret_cast<void*>(&MediaCodecGetBuffer)},
      {const_cast<char*>("getBuffers"), const_cast<char*>("(Z)[Ljava/nio/ByteBuffer;"), reinterpret_cast<void*>(&MediaCodecGetBuffers)},
      {const_cast<char*>("getFormatNative"), const_cast<char*>("(Z)Ljava/util/Map;"), reinterpret_cast<void*>(&MediaCodecGetFormatNative)},
      {const_cast<char*>("getOutputFormatNative"), const_cast<char*>("(I)Ljava/util/Map;"), reinterpret_cast<void*>(&MediaCodecGetOutputFormatNative)},
      {const_cast<char*>("native_configure"), const_cast<char*>("([Ljava/lang/String;[Ljava/lang/Object;Landroid/view/Surface;Landroid/media/MediaCrypto;Landroid/os/IHwBinder;I)V"), reinterpret_cast<void*>(&MediaCodecNativeConfigure)},
      {const_cast<char*>("native_dequeueInputBuffer"), const_cast<char*>("(J)I"), reinterpret_cast<void*>(&MediaCodecNativeDequeueInput)},
      {const_cast<char*>("native_dequeueOutputBuffer"), const_cast<char*>("(Landroid/media/MediaCodec$BufferInfo;J)I"), reinterpret_cast<void*>(&MediaCodecNativeDequeueOutput)},
      {const_cast<char*>("native_finalize"), const_cast<char*>("()V"), reinterpret_cast<void*>(&MediaCodecNativeFinalize)},
      {const_cast<char*>("native_flush"), const_cast<char*>("()V"), reinterpret_cast<void*>(&MediaCodecNativeReset)},
      {const_cast<char*>("native_init"), const_cast<char*>("()V"), reinterpret_cast<void*>(&MediaCodecNativeInit)},
      {const_cast<char*>("native_queueInputBuffer"), const_cast<char*>("(IIIJI)V"), reinterpret_cast<void*>(&MediaCodecQueueInput)},
      {const_cast<char*>("native_release"), const_cast<char*>("()V"), reinterpret_cast<void*>(&MediaCodecNativeRelease)},
      {const_cast<char*>("native_reset"), const_cast<char*>("()V"), reinterpret_cast<void*>(&MediaCodecNativeReset)},
      {const_cast<char*>("native_setSurface"), const_cast<char*>("(Landroid/view/Surface;)V"), reinterpret_cast<void*>(&MediaCodecNativeSetSurface)},
      {const_cast<char*>("native_setup"), const_cast<char*>("(Ljava/lang/String;ZZII)V"), reinterpret_cast<void*>(&MediaCodecNativeSetup)},
      {const_cast<char*>("native_start"), const_cast<char*>("()V"), reinterpret_cast<void*>(&MediaCodecNativeStart)},
      {const_cast<char*>("native_stop"), const_cast<char*>("()V"), reinterpret_cast<void*>(&MediaCodecNativeStop)},
      {const_cast<char*>("releaseOutputBuffer"), const_cast<char*>("(IZZJ)V"), reinterpret_cast<void*>(&MediaCodecReleaseOutput)},
  };
  if (!Register(env, "android/media/MediaCodec", codec_methods,
                static_cast<jint>(std::size(codec_methods)))) {
    return false;
  }
  JNINativeMethod list_methods[] = {
      {const_cast<char*>("findCodecByName"), const_cast<char*>("(Ljava/lang/String;)I"), reinterpret_cast<void*>(&MediaCodecListFind)},
      {const_cast<char*>("getAttributes"), const_cast<char*>("(I)I"), reinterpret_cast<void*>(&MediaCodecListAttributes)},
      {const_cast<char*>("getCanonicalName"), const_cast<char*>("(I)Ljava/lang/String;"), reinterpret_cast<void*>(&MediaCodecListCanonicalName)},
      {const_cast<char*>("getCodecCapabilities"), const_cast<char*>("(ILjava/lang/String;)Landroid/media/MediaCodecInfo$CodecCapabilities;"), reinterpret_cast<void*>(&MediaCodecListCapabilities)},
      {const_cast<char*>("getCodecName"), const_cast<char*>("(I)Ljava/lang/String;"), reinterpret_cast<void*>(&MediaCodecListName)},
      {const_cast<char*>("getSupportedTypes"), const_cast<char*>("(I)[Ljava/lang/String;"), reinterpret_cast<void*>(&MediaCodecListTypes)},
      {const_cast<char*>("native_getCodecCount"), const_cast<char*>("()I"), reinterpret_cast<void*>(&MediaCodecListCount)},
      {const_cast<char*>("native_getGlobalSettings"), const_cast<char*>("()Ljava/util/Map;"), reinterpret_cast<void*>(&MediaCodecListGlobalSettings)},
      {const_cast<char*>("native_init"), const_cast<char*>("()V"), reinterpret_cast<void*>(&MediaCodecNativeInit)},
  };
  return Register(env, "android/media/MediaCodecList", list_methods,
                  static_cast<jint>(std::size(list_methods)));
}

}  // namespace darwin_art
