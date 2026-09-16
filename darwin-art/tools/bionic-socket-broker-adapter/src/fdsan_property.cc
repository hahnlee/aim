#include "fdsan.h"
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <strings.h>

extern "C" const void* darwin_art_bionic___system_property_find(const char*);
extern "C" void darwin_art_bionic___system_property_read_callback(
    const void*, void (*)(void*, const char*, const char*, uint32_t), void*);

namespace {
struct ReadResult { int default_level; int previous_level; };
void Read(void* opaque, const char*, const char* value, uint32_t) {
  auto& result = *static_cast<ReadResult*>(opaque);
  int level = result.default_level;
  // Pinned Bionic09a271af fdsan.cpp: unknown/empty/"0" use the caller's
  // default. They do not unconditionally select DISABLED.
  if (!strcasecmp(value, "1") || !strcasecmp(value, "fatal")) level = 3;
  else if (!strcasecmp(value, "warn")) level = 2;
  else if (!strcasecmp(value, "warn_once")) level = 1;
  else if (*value && strcasecmp(value, "0"))
    std::fprintf(stderr, "fdsan: unknown debug.fdsan value '%s'; applying default\n", value);
  result.previous_level = darwin_art_fdsan_set_error_level(level);
}
}

extern "C" int darwin_art_bionic_android_fdsan_set_error_level_from_property(int default_level) {
  const void* property = darwin_art_bionic___system_property_find("debug.fdsan");
  if (!property) return darwin_art_fdsan_set_error_level(default_level);
  ReadResult result{default_level, darwin_art_fdsan_get_error_level()};
  darwin_art_bionic___system_property_read_callback(property, Read, &result);
  return result.previous_level;
}
