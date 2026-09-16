#include <android/bitmap.h>
#include <android/data_space.h>
#include <CoreGraphics/CoreGraphics.h>
#include <ImageIO/ImageIO.h>
#include <dlfcn.h>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <vector>

namespace {
bool Write(void* context, const void* bytes, size_t length) {
  auto& output = *static_cast<std::vector<uint8_t>*>(context);
  const auto* first = static_cast<const uint8_t*>(bytes);
  output.insert(output.end(), first, first + length);
  return true;
}
bool Reject(void*, const void*, size_t) { return false; }
template <typename Function>
Function Symbol(void* library, const char* name) {
  auto function = reinterpret_cast<Function>(dlsym(library, name));
  if (!function) std::fprintf(stderr, "%s: %s\n", name, dlerror());
  assert(function);
  return function;
}
}

int main(int argc, char** argv) {
  assert(argc == 2);
  void* library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
  if (!library) std::fprintf(stderr, "%s\n", dlerror());
  assert(library);
  const auto info = Symbol<decltype(&AndroidBitmap_getInfo)>(library, "AndroidBitmap_getInfo");
  const auto lock = Symbol<decltype(&AndroidBitmap_lockPixels)>(library, "AndroidBitmap_lockPixels");
  const auto unlock = Symbol<decltype(&AndroidBitmap_unlockPixels)>(library, "AndroidBitmap_unlockPixels");
  const auto space = Symbol<decltype(&AndroidBitmap_getDataSpace)>(library, "AndroidBitmap_getDataSpace");
  const auto hardware = Symbol<decltype(&AndroidBitmap_getHardwareBuffer)>(library, "AndroidBitmap_getHardwareBuffer");
  const auto compress = Symbol<decltype(&AndroidBitmap_compress)>(library, "AndroidBitmap_compress");
  assert(info(nullptr, nullptr, nullptr) == ANDROID_BITMAP_RESULT_BAD_PARAMETER);
  assert(lock(nullptr, nullptr, nullptr) == ANDROID_BITMAP_RESULT_BAD_PARAMETER);
  assert(unlock(nullptr, nullptr) == ANDROID_BITMAP_RESULT_BAD_PARAMETER);
  assert(space(nullptr, nullptr) == ADATASPACE_UNKNOWN);
  assert(hardware(nullptr, nullptr, nullptr) == ANDROID_BITMAP_RESULT_BAD_PARAMETER);

  const AndroidBitmapInfo bitmap{2, 1, 8, ANDROID_BITMAP_FORMAT_RGBA_8888,
                                ANDROID_BITMAP_FLAGS_ALPHA_OPAQUE};
  const uint8_t pixels[]{255, 0, 0, 255, 0, 0, 255, 255};
  std::vector<uint8_t> encoded;
  assert(compress(&bitmap, ADATASPACE_SRGB, pixels, ANDROID_BITMAP_COMPRESS_FORMAT_PNG,
                  -1, &encoded, Write) == ANDROID_BITMAP_RESULT_BAD_PARAMETER);
  assert(encoded.empty());
  const int compressed = compress(&bitmap, ADATASPACE_SRGB, pixels,
      ANDROID_BITMAP_COMPRESS_FORMAT_PNG, 100, &encoded, Write);
  if (compressed != ANDROID_BITMAP_RESULT_SUCCESS)
    std::fprintf(stderr, "PNG compression status=%d bytes=%zu\n", compressed, encoded.size());
  assert(compressed == ANDROID_BITMAP_RESULT_SUCCESS);
  assert(encoded.size() > 8 && encoded[0] == 0x89 && encoded[1] == 'P');
  CFDataRef data = CFDataCreate(nullptr, encoded.data(), encoded.size());
  CGImageSourceRef source = CGImageSourceCreateWithData(data, nullptr);
  assert(source);
  CGImageRef image = CGImageSourceCreateImageAtIndex(source, 0, nullptr);
  assert(image && CGImageGetWidth(image) == 2 && CGImageGetHeight(image) == 1);
  uint8_t decoded[8]{};
  CGColorSpaceRef colors = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
  CGContextRef context = CGBitmapContextCreate(decoded, 2, 1, 8, 8, colors,
      static_cast<CGBitmapInfo>(kCGImageAlphaPremultipliedLast) | kCGBitmapByteOrder32Big);
  assert(context);
  CGContextDrawImage(context, CGRectMake(0, 0, 2, 1), image);
  for (size_t i = 0; i < sizeof(pixels); ++i) {
    if (decoded[i] != pixels[i])
      std::fprintf(stderr, "RGBA byte %zu: expected=%u actual=%u\n", i,
                   static_cast<unsigned>(pixels[i]), static_cast<unsigned>(decoded[i]));
    assert(decoded[i] == pixels[i]);
  }
  CGContextRelease(context);
  CGColorSpaceRelease(colors);
  CGImageRelease(image);
  CFRelease(source);
  CFRelease(data);
  assert(compress(&bitmap, ADATASPACE_SRGB, pixels, ANDROID_BITMAP_COMPRESS_FORMAT_PNG,
                  100, nullptr, Reject) == ANDROID_BITMAP_RESULT_JNI_EXCEPTION);
  // This process uses runtime-owned global graphics state: no dlclose while
  // those owners remain live. Process exit tears down the diagnostic itself.
  std::puts("original NDK Bitmap: six exports, validation, PNG pixels, callback failure PASS");
}
