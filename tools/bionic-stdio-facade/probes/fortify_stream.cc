#include "aim_bionic_stdio.h"
#include "aim_bionic_provider_namespace.h"
#include <cstdio>
#include <cstdint>

extern "C" int aim_fortify_stream_smoke(AimBionicNamespace* instance) {
  const auto route = aim_bionic_namespace_resolve(
      instance, "libc.so", "__fread_chk", "LIBC_N");
  if (route.status != AIM_BIONIC_NAMESPACE_OK || route.address == 0) return 1;
  using Read = size_t (*)(void*, size_t, size_t, AimAndroidFile*, size_t);
  const auto checked_read = reinterpret_cast<Read>(route.address);
  auto* stream = aim_bionic_fopen("/fixture", "r");
  if (!stream) return 2;
  char byte = 0;
  int result = 0;
  if (checked_read(&byte, 1, 1, stream, 1) != 1 || byte != 'x') result = 3;
  if (checked_read(&byte, 1, 1, stream, 1) != 0) result = 4;
  if (checked_read(&byte, SIZE_MAX, 2, stream, 1) != 0) result = 5;
  if (aim_bionic_fclose(stream) != 0) result = 6;
  if (result == 0) std::fprintf(stderr, "fortified stream: namespace read/EOF/overflow PASS\n");
  return result;
}
