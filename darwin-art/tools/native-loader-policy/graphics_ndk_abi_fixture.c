#include <android/bitmap.h>
#include <android/data_space.h>
#include <android/imagedecoder.h>

// Test-only ABI consumer, compiled by the Android NDK, never a production API.
struct GraphicsApi {
  __typeof__(&AndroidBitmap_compress) compress;
  __typeof__(&AImageDecoder_createFromBuffer) create;
  __typeof__(&AImageDecoder_setCrop) crop;
  __typeof__(&AImageDecoder_decodeImage) decode;
  __typeof__(&AImageDecoder_delete) destroy;
};
struct Encoded { unsigned char bytes[4096]; size_t size; int calls; };
static bool write_png(void* opaque, const void* data, size_t size) {
  struct Encoded* out = opaque;
  if (size > sizeof(out->bytes) - out->size) return false;
  for (size_t i = 0; i < size; ++i)
    out->bytes[out->size + i] = ((const unsigned char*)data)[i];
  out->size += size;
  ++out->calls;
  return true;
}
int graphics_ndk_abi(const struct GraphicsApi* api) {
  const unsigned char pixels[8] = {255, 0, 0, 255, 0, 0, 255, 255};
  AndroidBitmapInfo info = {2, 1, 8, ANDROID_BITMAP_FORMAT_RGBA_8888,
      ANDROID_BITMAP_FLAGS_ALPHA_OPAQUE};
  struct Encoded encoded;
  encoded.size = 0;
  encoded.calls = 0;
  if (api->compress(&info, ADATASPACE_SRGB, pixels,
      ANDROID_BITMAP_COMPRESS_FORMAT_PNG, 100, &encoded, write_png) != 0) return 1;
  if (!encoded.calls || !encoded.size) return 2;
  AImageDecoder* decoder = 0;
  if (api->create(encoded.bytes, encoded.size, &decoder) != 0 || !decoder) return 3;
  // ARect is passed by value across ELF AAPCS64 -> Darwin arm64 ABI.
  const ARect crop = {1, 0, 2, 1};
  int result = api->crop(decoder, crop);
  unsigned char output[4];
  if (!result) result = api->decode(decoder, output, 4, sizeof(output));
  api->destroy(decoder);
  if (result) return 4;
  for (size_t i = 0; i < sizeof(output); ++i)
    if (output[i] != pixels[4 + i]) return 5;
  return 0;
}
