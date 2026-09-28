#pragma once

#include <cstdio>

// Keep the small liblog surface used by ART and libziparchive present without
// pulling Android's logging daemon into the native Darwin process.
// SafetyNet event reporting has no Darwin equivalent and is intentionally a
// no-op in this compatibility process.
inline int android_errorWriteLog(int, const char*) {
  return 0;
}

#define ALOGE(...) do { std::fprintf(stderr, __VA_ARGS__); std::fputc('\n', stderr); } while (0)
#define ALOGW(...) do { std::fprintf(stderr, __VA_ARGS__); std::fputc('\n', stderr); } while (0)
#define ALOGD(...) do { } while (0)
#define ALOGV(...) do { } while (0)
