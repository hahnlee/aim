#include "loader/namespace_handles.h"
#include "loader/android_dlext_types.h"
#include "loader/graphics_ndk_image.h"
#include <android/bitmap.h>
#include <android/data_space.h>
#include <android/imagedecoder.h>
#include <cassert>
#include <cstdio>
#include <cstring>
#include <vector>

extern "C" void* darwin_art_linker_dlsym(void*, const char*);
extern "C" int darwin_art_linker_dlclose(void*);
void TestGraphicsNdkElfAbi(void* library);
static bool Write(void* context, const void* bytes, size_t count) {
  auto& output = *static_cast<std::vector<unsigned char>*>(context);
  auto* first = static_cast<const unsigned char*>(bytes);
  output.insert(output.end(), first, first + count);
  return true;
}
void TestSystemGraphicsNdk(darwin_art::loader::NamespaceHandles& owner) {
  android_dlextinfo request{};
  request.flags = ANDROID_DLEXT_USE_NAMESPACE;
  request.library_namespace = owner.Exported("default");
  void* library = android_dlopen_ext("libjnigraphics.so", 2, &request);
  if (!library) std::fprintf(stderr, "graphics NDK open: %s\n", darwin_art_linker_dlerror());
  assert(library);
  TestGraphicsNdkElfAbi(library);
#define API(name) auto name = reinterpret_cast<decltype(&::name)>(darwin_art_linker_dlsym(library, #name)); assert(name)
  API(AndroidBitmap_compress);
  API(AImageDecoder_createFromBuffer);
  API(AImageDecoder_decodeImage);
  API(AImageDecoder_delete);
#undef API
  const unsigned char pixels[]{255, 0, 0, 255, 0, 0, 255, 255};
  AndroidBitmapInfo info{2, 1, 8, ANDROID_BITMAP_FORMAT_RGBA_8888, ANDROID_BITMAP_FLAGS_ALPHA_OPAQUE};
  std::vector<unsigned char> encoded;
  assert(AndroidBitmap_compress(&info, ADATASPACE_SRGB, pixels,
      ANDROID_BITMAP_COMPRESS_FORMAT_PNG, 100, &encoded, Write) == 0);
  AImageDecoder* decoder = nullptr;
  assert(AImageDecoder_createFromBuffer(encoded.data(), encoded.size(), &decoder) == 0);
  unsigned char decoded[8]{};
  assert(AImageDecoder_decodeImage(decoder, decoded, 8, 8) == 0);
  assert(std::memcmp(decoded, pixels, sizeof(pixels)) == 0);
  AImageDecoder_delete(decoder);
  for (const char* denied : {"malloc", "AAssetManager_open", "ABitmap_compress", "JNI_OnLoad"}) {
    assert(!darwin_art_linker_dlsym(library, denied));
  }
  LinkerImageLease* image = nullptr;
  assert(owner.FindResident(request.library_namespace, "libjnigraphics.so", &image) == 0);
  assert(darwin_art::loader::IsGraphicsNdkImage(image));
  uintptr_t address = 1;
  std::string error;
  assert(darwin_art::loader::ResolveGraphicsNdkImage(image, "AndroidBitmap_compress",
      "WRONG_VERSION", &address, &error) == 1 && address == 0);
  assert(darwin_art::loader::ResolveGraphicsNdkImage(image, "AndroidBitmap_compress",
      "LIBJNIGRAPHICS", &address, &error) == 0 && address);
  void* payload = reinterpret_cast<void*>(1);
  assert(darwin_art_linker_image_typed_payload(image, DARWIN_ART_IMAGE_MACHO, &payload) == -4 && !payload);
  darwin_art_linker_image_release(image);
  assert(darwin_art_linker_dlclose(library) == 0);
  library = android_dlopen_ext("libjnigraphics.so", 2, &request);
  assert(library && darwin_art_linker_dlsym(library, "AndroidBitmap_compress") == reinterpret_cast<void*>(address));
  assert(darwin_art_linker_dlclose(library) == 0);
  std::puts("graphics NDK namespace: original PNG roundtrip, scoped exports/version, typed owner and reopen PASS");
}
