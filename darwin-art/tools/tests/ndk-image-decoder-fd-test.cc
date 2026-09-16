#include <android/imagedecoder.h>
#include "darwin_art_bionic_fs.h"
#include <cassert>
#include <cstdint>
#include <cstdio>
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

void TestDecoderFd(void* library, const char* root_path, const std::vector<uint8_t>& png) {
#define API(name) const auto name = Symbol<decltype(&::name)>(library, #name)
  API(darwin_art_bionic_fs_process_install);
  API(darwin_art_bionic_fs_process_uninstall);
  API(darwin_art_bionic_fs_adopt_host_fd_core);
  API(darwin_art_bionic_fs_close_core);
  API(AImageDecoder_createFromFd);
  API(AImageDecoder_decodeImage);
  API(AImageDecoder_delete);
#undef API
  int root = open(root_path, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
  assert(root >= 0);
  const uint8_t mount[]{'/'};
  assert(darwin_art_bionic_fs_process_install(root, mount, 1, mount, 1) == 0);
  close(root);
  FILE* source = tmpfile();
  assert(source);
  assert(fwrite(png.data(), 1, png.size(), source) == png.size());
  assert(fflush(source) == 0 && fseek(source, 0, SEEK_SET) == 0);
  const int raw_host_fd = fileno(source);
  AImageDecoder* decoder = nullptr;
  assert(AImageDecoder_createFromFd(raw_host_fd, &decoder) == ANDROID_IMAGE_DECODER_BAD_PARAMETER);
  assert(!decoder && fcntl(raw_host_fd, F_GETFD) >= 0);
  const int guest = darwin_art_bionic_fs_adopt_host_fd_core(dup(raw_host_fd));
  assert(guest >= 0 && guest != raw_host_fd);
  assert(AImageDecoder_createFromFd(guest, &decoder) == ANDROID_IMAGE_DECODER_SUCCESS);
  assert(decoder);
  assert(darwin_art_bionic_fs_close_core(guest) == 0);
  fclose(source);

  int pipes[2];
  assert(pipe(pipes) == 0);
  const int pipe_guest = darwin_art_bionic_fs_adopt_host_fd_core(pipes[0]);
  assert(pipe_guest >= 0);
  AImageDecoder* rejected = nullptr;
  assert(AImageDecoder_createFromFd(pipe_guest, &rejected) == ANDROID_IMAGE_DECODER_BAD_PARAMETER);
  assert(!rejected);
  assert(darwin_art_bionic_fs_close_core(pipe_guest) == 0);
  close(pipes[1]);
  // An already-created decoder owns its stream independently from both the
  // Android fd table and the caller's descriptor lifetime.
  assert(darwin_art_bionic_fs_process_uninstall() == 0);
  uint8_t decoded[8]{};
  const uint8_t expected[]{255, 0, 0, 255, 0, 0, 255, 255};
  assert(AImageDecoder_decodeImage(decoder, decoded, 8, 8) == ANDROID_IMAGE_DECODER_SUCCESS);
  for (size_t i = 0; i < sizeof(decoded); ++i) assert(decoded[i] == expected[i]);
  AImageDecoder_delete(decoder);
  std::puts("NDK decoder real Rust FD owner: host rejection, pipe rejection, retained pixels after close/uninstall PASS");
}
