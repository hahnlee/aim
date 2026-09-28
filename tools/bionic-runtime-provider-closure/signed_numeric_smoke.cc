#include "aim_bionic_builtin_adapters.h"
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstring>

extern "C" int32_t aim_bionic_errno_load();
extern "C" void aim_bionic_errno_store(int32_t);

extern "C" int aim_signed_numeric_smoke(AimBionicNamespace* ns) {
  const auto route = aim_bionic_namespace_resolve(ns, "libc.so", "strtoimax", "LIBC");
  if (route.status != AIM_BIONIC_NAMESPACE_OK || !route.address) return 1;
  using Convert = int64_t (*)(const char*, char**, int);
  const auto convert = reinterpret_cast<Convert>(route.address);
  struct Case { const char* text; int base; int64_t value; const char* rest; int error; };
  const Case cases[] = {
      {"-9223372036854775808!", 10, INT64_MIN, "!", 123},
      {"9223372036854775807!", 10, INT64_MAX, "!", 123},
      {"9223372036854775808!", 10, INT64_MAX, "!", 34},
      {"-9223372036854775809!", 10, INT64_MIN, "!", 34},
      {"  -0x2a!", 0, -42, "!", 123},
      {"077!", 0, 63, "!", 123},
      {"xyz", 10, 0, "xyz", 123},
  };
  const int saved_host = errno;
  const int saved_guest = aim_bionic_errno_load();
  bool passed = true;
  for (const auto& item : cases) {
    errno = 12345;
    aim_bionic_errno_store(123);
    char* end = nullptr;
    const auto value = convert(item.text, &end, item.base);
    if (value != item.value || !end || std::strcmp(end, item.rest) != 0 ||
        aim_bionic_errno_load() != item.error || errno != 12345) passed = false;
  }
  aim_bionic_errno_store(saved_guest);
  errno = saved_host;
  if (!passed) return 2;
  std::fprintf(stderr, "strtoimax: sealed ABI signed bounds/endptr/guest errno/host isolation PASS\n");
  return 0;
}
