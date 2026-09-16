#include <android/imagedecoder.h>
#include <cassert>
#include <cstdio>
#include <cstdint>
#include <dlfcn.h>
#include <fstream>
#include <iterator>
#include <vector>

namespace {
template <typename T> T Symbol(void* library, const char* name) {
  auto symbol = reinterpret_cast<T>(dlsym(library, name));
  assert(symbol);
  return symbol;
}
std::vector<uint8_t> Read(const char* path) {
  std::ifstream input(path, std::ios::binary);
  assert(input);
  return {std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>()};
}
}
void TestDecoderAnimation(void* library, const char* image_path, const char* reference_path) {
#define API(name) const auto name = Symbol<decltype(&::name)>(library, #name)
  API(AImageDecoder_createFromBuffer);
  API(AImageDecoder_isAnimated);
  API(AImageDecoder_decodeImage);
  API(AImageDecoder_advanceFrame);
  API(AImageDecoder_rewind);
  API(AImageDecoder_getFrameInfo);
  API(AImageDecoderFrameInfo_create);
  API(AImageDecoderFrameInfo_getDuration);
  API(AImageDecoderFrameInfo_delete);
  API(AImageDecoder_delete);
#undef API
  auto bytes = Read(image_path);
  auto reference = Read(reference_path);
  constexpr size_t frame_bytes = 11 * 29 * 4;
  assert(reference.size() == 3 * frame_bytes);
  AImageDecoder* decoder = nullptr;
  assert(AImageDecoder_createFromBuffer(bytes.data(), bytes.size(), &decoder) == 0);
  assert(decoder && AImageDecoder_isAnimated(decoder));
  auto* info = AImageDecoderFrameInfo_create();
  assert(info);
  const int64_t durations[]{1'000'000'000, 500'000'000, 1'000'000'000};
  std::vector<uint8_t> decoded(frame_bytes);
  for (size_t frame = 0; frame < 3; ++frame) {
    assert(AImageDecoder_getFrameInfo(decoder, info) == 0);
    assert(AImageDecoderFrameInfo_getDuration(info) == durations[frame]);
    assert(AImageDecoder_decodeImage(decoder, decoded.data(), 11 * 4, decoded.size()) == 0);
    for (size_t byte = 0; byte < frame_bytes; ++byte) {
      if (decoded[byte] != reference[frame * frame_bytes + byte]) {
        std::fprintf(stderr, "animation frame=%zu byte=%zu actual=%u expected=%u\n",
            frame, byte, unsigned(decoded[byte]), unsigned(reference[frame * frame_bytes + byte]));
        assert(false);
      }
    }
    const int advanced = AImageDecoder_advanceFrame(decoder);
    assert(advanced == (frame == 2 ? ANDROID_IMAGE_DECODER_FINISHED : ANDROID_IMAGE_DECODER_SUCCESS));
  }
  assert(AImageDecoder_decodeImage(decoder, decoded.data(), 44, decoded.size()) == ANDROID_IMAGE_DECODER_FINISHED);
  assert(AImageDecoder_rewind(decoder) == 0);
  assert(AImageDecoder_decodeImage(decoder, decoded.data(), 44, decoded.size()) == 0);
  for (size_t i = 0; i < frame_bytes; ++i) assert(decoded[i] == reference[i]);
  AImageDecoderFrameInfo_delete(info);
  AImageDecoder_delete(decoder);
  std::puts("NDK animated WebP: three reference frames, durations, finish and rewind PASS");
}
