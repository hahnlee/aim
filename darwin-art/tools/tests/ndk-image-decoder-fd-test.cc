#include <android/imagedecoder.h>
#include "darwin_art_bionic_fs.h"
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <string>
#include <dlfcn.h>
#include <fcntl.h>
#include <unistd.h>
#include <vector>

namespace {
template <typename T> T Symbol(void* library, const char* name) {
  auto symbol = reinterpret_cast<T>(dlsym(library, name));
  if (!symbol) std::fprintf(stderr, "%s: %s\n", name, dlerror());
  assert(symbol);
  return symbol;
}
}

// The facade's exported guest descriptor entries (bionic open/close), not
// its private host-fd adoption core: a guest fd is made the way Android code
// makes one.
extern "C" int darwin_art_bionic_open(const char*, int, uint32_t);
extern "C" int darwin_art_bionic_close(int);

namespace {
constexpr int kAndroidReadOnly = 0;           // O_RDONLY
constexpr int kAndroidDirectory = 040000;     // arm64 O_DIRECTORY
}  // namespace

void TestDecoderFd(void* library, const char* root_path, const std::vector<uint8_t>& png) {
#define API(name) const auto name = Symbol<decltype(&::name)>(library, #name)
  API(darwin_art_bionic_fs_process_install);
  API(darwin_art_bionic_fs_process_uninstall);
  API(darwin_art_bionic_open);
  API(darwin_art_bionic_close);
  API(AImageDecoder_createFromFd);
  API(AImageDecoder_decodeImage);
  API(AImageDecoder_delete);
#undef API
  const std::string image_path = std::string(root_path) + "/fd-image.png";
  FILE* source = std::fopen(image_path.c_str(), "wb");
  assert(source);
  assert(std::fwrite(png.data(), 1, png.size(), source) == png.size());
  assert(std::fclose(source) == 0);
  int root = open(root_path, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
  assert(root >= 0);
  const uint8_t mount[]{'/'};
  assert(darwin_art_bionic_fs_process_install(root, mount, 1, mount, 1) == 0);
  close(root);

  // A host descriptor is not an Android descriptor.
  const int raw_host_fd = open(image_path.c_str(), O_RDONLY | O_CLOEXEC);
  assert(raw_host_fd >= 0);
  AImageDecoder* decoder = nullptr;
  assert(AImageDecoder_createFromFd(raw_host_fd, &decoder) == ANDROID_IMAGE_DECODER_BAD_PARAMETER);
  assert(!decoder && fcntl(raw_host_fd, F_GETFD) >= 0);
  close(raw_host_fd);

  const int guest = darwin_art_bionic_open("/fd-image.png", kAndroidReadOnly, 0);
  assert(guest >= 0);
  assert(AImageDecoder_createFromFd(guest, &decoder) == ANDROID_IMAGE_DECODER_SUCCESS);
  assert(decoder);
  assert(darwin_art_bionic_close(guest) == 0);

  // A guest descriptor that is not a readable image stream is rejected.
  const int directory = darwin_art_bionic_open("/", kAndroidReadOnly | kAndroidDirectory, 0);
  assert(directory >= 0);
  AImageDecoder* rejected = nullptr;
  assert(AImageDecoder_createFromFd(directory, &rejected) != ANDROID_IMAGE_DECODER_SUCCESS);
  assert(!rejected);
  assert(darwin_art_bionic_close(directory) == 0);
  // An already-created decoder owns its stream independently from both the
  // Android fd table and the caller's descriptor lifetime.
  assert(darwin_art_bionic_fs_process_uninstall() == 0);
  uint8_t decoded[8]{};
  const uint8_t expected[]{255, 0, 0, 255, 0, 0, 255, 255};
  assert(AImageDecoder_decodeImage(decoder, decoded, 8, 8) == ANDROID_IMAGE_DECODER_SUCCESS);
  for (size_t i = 0; i < sizeof(decoded); ++i) assert(decoded[i] == expected[i]);
  AImageDecoder_delete(decoder);
  std::puts("NDK decoder guest FD: host rejection, non-image rejection, retained pixels after close/uninstall PASS");
}
