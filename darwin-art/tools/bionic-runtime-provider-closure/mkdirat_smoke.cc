#include "darwin_art_bionic_builtin_adapters.h"
#include <cerrno>
#include <cstdint>
#include <cstdio>

extern "C" int32_t darwin_art_bionic_errno_load();
extern "C" void darwin_art_bionic_errno_store(int32_t);

// The closure's root is read-only. Writable-directory creation is covered by
// filesystem owner tests; this gate verifies actual versioned ABI/error routing.
extern "C" int darwin_art_mkdirat_smoke(DarwinArtBionicNamespace* ns) {
  const auto result = darwin_art_bionic_namespace_resolve(ns, "libc.so", "mkdirat", "LIBC");
  if (result.status != DARWIN_ART_BIONIC_NAMESPACE_OK || !result.address) return 1;
  using Mkdir = int (*)(int, const char*, uint32_t);
  const auto create = reinterpret_cast<Mkdir>(result.address);
  const int saved_host = errno;
  const int saved_guest = darwin_art_bionic_errno_load();
  errno = 12345;
  const int invalid = create(-12345, "relative", 0700);
  const int invalid_error = darwin_art_bionic_errno_load();
  const bool host_preserved = errno == 12345;
  darwin_art_bionic_errno_store(saved_guest);
  errno = saved_host;
  if (invalid != -1 || invalid_error != 9 || !host_preserved) return 2;
  std::fprintf(stderr, "mkdirat: sealed ABI invalid guest dirfd/errno isolation PASS\n");
  return 0;
}
