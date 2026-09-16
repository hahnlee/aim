#include "darwin_art_bionic_stdio.h"
#include "darwin_art_bionic_provider_namespace.h"
#include <cstdio>
#include <cstdint>

extern "C" int darwin_art_fortify_stream_smoke(DarwinArtBionicNamespace* instance) {
  const auto route = darwin_art_bionic_namespace_resolve(
      instance, "libc.so", "__fread_chk", "LIBC_N");
  if (route.status != DARWIN_ART_BIONIC_NAMESPACE_OK || route.address == 0) return 1;
  using Read = size_t (*)(void*, size_t, size_t, DarwinArtAndroidFile*, size_t);
  const auto checked_read = reinterpret_cast<Read>(route.address);
  auto* stream = darwin_art_bionic_fopen("/fixture", "r");
  if (!stream) return 2;
  char byte = 0;
  int result = 0;
  if (checked_read(&byte, 1, 1, stream, 1) != 1 || byte != 'x') result = 3;
  if (checked_read(&byte, 1, 1, stream, 1) != 0) result = 4;
  if (checked_read(&byte, SIZE_MAX, 2, stream, 1) != 0) result = 5;
  if (darwin_art_bionic_fclose(stream) != 0) result = 6;
  if (result == 0) std::fprintf(stderr, "fortified stream: namespace read/EOF/overflow PASS\n");
  return result;
}
