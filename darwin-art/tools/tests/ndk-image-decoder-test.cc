#include <android/bitmap.h>
#include <android/data_space.h>
#include <android/imagedecoder.h>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <dlfcn.h>
#include <vector>

void TestDecoderFd(void*, const char*, const std::vector<uint8_t>&);
void TestDecoderAnimation(void*, const char*, const char*);

template <typename T> T Symbol(void* library, const char* name) {
  auto symbol = reinterpret_cast<T>(dlsym(library, name));
  if (!symbol) std::fprintf(stderr, "%s: %s\n", name, dlerror());
  assert(symbol);
  return symbol;
}
static bool Write(void* context, const void* data, size_t size) {
  auto& out = *static_cast<std::vector<uint8_t>*>(context);
  auto* first = static_cast<const uint8_t*>(data);
  out.insert(out.end(), first, first + size);
  return true;
}
int main(int argc, char** argv) {
  assert(argc == 5);
  void* library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
  if (!library) std::fprintf(stderr, "%s\n", dlerror());
  assert(library);
#define API(name) const auto name = Symbol<decltype(&::name)>(library, #name)
  API(AndroidBitmap_compress);
  API(AImageDecoder_createFromBuffer);
  API(AImageDecoder_getHeaderInfo);
  API(AImageDecoderHeaderInfo_getWidth);
  API(AImageDecoderHeaderInfo_getHeight);
  API(AImageDecoderHeaderInfo_getAndroidBitmapFormat);
  API(AImageDecoder_setAndroidBitmapFormat);
  API(AImageDecoder_getMinimumStride);
  API(AImageDecoder_decodeImage);
  API(AImageDecoder_delete);
#undef API
  const uint8_t invalid[]{0, 1, 2, 3, 4, 5, 6, 7};
  AImageDecoder* decoder = reinterpret_cast<AImageDecoder*>(uintptr_t{1});
  assert(AImageDecoder_createFromBuffer(invalid, sizeof(invalid), &decoder) ==
         ANDROID_IMAGE_DECODER_INCOMPLETE);
  assert(!decoder);
  uint8_t malformed[128];
  for (auto& byte : malformed) byte = 0xff;
  const int malformed_result = AImageDecoder_createFromBuffer(malformed, sizeof(malformed), &decoder);
  // SkCodec reports kUnimplemented when a full sniff buffer has no matching
  // decoder; AOSP maps that to UNSUPPORTED_FORMAT, not INVALID_INPUT.
  assert(malformed_result == ANDROID_IMAGE_DECODER_UNSUPPORTED_FORMAT);
  assert(!decoder);
  const uint8_t pixels[]{255, 0, 0, 255, 0, 0, 255, 255};
  const AndroidBitmapInfo info{2, 1, 8, ANDROID_BITMAP_FORMAT_RGBA_8888,
                               ANDROID_BITMAP_FLAGS_ALPHA_OPAQUE};
  std::vector<uint8_t> png;
  assert(AndroidBitmap_compress(&info, ADATASPACE_SRGB, pixels,
      ANDROID_BITMAP_COMPRESS_FORMAT_PNG, 100, &png, Write) == ANDROID_BITMAP_RESULT_SUCCESS);
  assert(AImageDecoder_createFromBuffer(png.data(), png.size(), &decoder) ==
         ANDROID_IMAGE_DECODER_SUCCESS);
  assert(decoder);
  const auto* header = AImageDecoder_getHeaderInfo(decoder);
  assert(AImageDecoderHeaderInfo_getWidth(header) == 2);
  assert(AImageDecoderHeaderInfo_getHeight(header) == 1);
  assert(AImageDecoderHeaderInfo_getAndroidBitmapFormat(header) == ANDROID_BITMAP_FORMAT_RGBA_8888);
  assert(AImageDecoder_getMinimumStride(decoder) == 8);
  uint8_t decoded[8]{};
  assert(AImageDecoder_decodeImage(decoder, decoded, 8, 7) == ANDROID_IMAGE_DECODER_BAD_PARAMETER);
  assert(AImageDecoder_decodeImage(decoder, decoded, 8, 8) == ANDROID_IMAGE_DECODER_SUCCESS);
  for (size_t i = 0; i < sizeof(decoded); ++i) {
    if (decoded[i] != pixels[i])
      std::fprintf(stderr, "decoder RGBA byte %zu: expected=%u actual=%u\n", i,
                   static_cast<unsigned>(pixels[i]), static_cast<unsigned>(decoded[i]));
    assert(decoded[i] == pixels[i]);
  }
  AImageDecoder_delete(decoder);
  decoder = nullptr;
  assert(AImageDecoder_createFromBuffer(png.data(), png.size(), &decoder) == ANDROID_IMAGE_DECODER_SUCCESS);
  assert(AImageDecoder_setAndroidBitmapFormat(decoder, ANDROID_BITMAP_FORMAT_RGBA_8888) ==
         ANDROID_IMAGE_DECODER_SUCCESS);
  assert(AImageDecoder_decodeImage(decoder, decoded, 8, 8) == ANDROID_IMAGE_DECODER_SUCCESS);
  for (size_t i = 0; i < sizeof(decoded); ++i) assert(decoded[i] == pixels[i]);
  AImageDecoder_delete(decoder);
  std::puts("original NDK decoder: malformed input, bounds, PNG dimensions and RGBA pixels PASS");
  TestDecoderFd(library, argv[2], png);
  TestDecoderAnimation(library, argv[3], argv[4]);
}
