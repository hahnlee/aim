#include "aim_bionic_formatted_stdio.h"

#include "aim_bionic_allocator.h"
#include "aim_bionic_errno.h"
#include "aim_bionic_format.h"

#include <cerrno>
#include <cstdint>
#include <cstring>

namespace {

constexpr int kE2big = 7;
constexpr int kEfbig = 27;
constexpr int kEinval = 22;
constexpr size_t kMaxFormatBytes = 4096;
constexpr size_t kMaxFormattedBytes = 1024 * 1024;

struct AndroidVaList {
  void* stack;
  void* gr_top;
  void* vr_top;
  int32_t gr_offs;
  int32_t vr_offs;
};

void Fail(int value) {
  aim_bionic_errno_store(value);
}

bool HasBoundedTerminator(const char* text, size_t limit) {
  if (text == nullptr) return false;
  for (size_t i = 0; i < limit; ++i) {
    if (text[i] == '\0') return true;
  }
  return false;
}

int WriteFormatted(AimAndroidFile* file, const char* format,
                   const void* android_va_list) {
  if (file == nullptr || format == nullptr || android_va_list == nullptr) {
    Fail(kEinval);
    return -1;
  }
  if (!HasBoundedTerminator(format, kMaxFormatBytes)) {
    Fail(kE2big);
    return -1;
  }

  // This is a provider-local token validation. It does not flush host stdio.
  if (aim_bionic_stdio_fflush_core(file) != 0) return -1;

  AimBionicAllocationResult allocation =
      aim_bionic_malloc_result(kMaxFormattedBytes + 1);
  if (allocation.pointer == nullptr) {
    Fail(allocation.bionic_errno);
    return -1;
  }

  char* output = static_cast<char*>(allocation.pointer);
  const int count = aim_bionic_vsnprintf(
      output, kMaxFormattedBytes + 1, format, android_va_list);
  if (count < 0) {
    aim_bionic_free(output);
    return -1;
  }
  if (static_cast<size_t>(count) > kMaxFormattedBytes) {
    aim_bionic_free(output);
    Fail(kEfbig);
    return -1;
  }

  const size_t expected = static_cast<size_t>(count);
  const size_t written = expected == 0
                             ? 0
                             : aim_bionic_stdio_fwrite_core(
                                   output, 1, expected, file);
  aim_bionic_free(output);
  if (written != expected) {
    return -1;
  }
  return count;
}

int PreserveHostErrno(AimAndroidFile* file, const char* format,
                      const void* android_va_list) {
  const int saved_host_errno = errno;
  const int result = WriteFormatted(file, format, android_va_list);
  errno = saved_host_errno;
  return result;
}

bool Equal(const char* left, const char* right) {
  return left != nullptr && right != nullptr && std::strcmp(left, right) == 0;
}

}  // namespace

extern "C" int aim_bionic_vfprintf(
    AimAndroidFile* file, const char* format,
    const void* android_va_list) {
  return PreserveHostErrno(file, format, android_va_list);
}

extern "C" int aim_bionic_fprintf_captured(
    AimAndroidFile* file, const char* format, const uint64_t* gp,
    const uint8_t* fp, uint8_t* stack) {
  AndroidVaList android_va_list{
      stack,
      reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(gp) + 64),
      reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(fp) + 128),
      -48,
      -128,
  };
  return aim_bionic_vfprintf(file, format, &android_va_list);
}

extern "C" int aim_bionic_printf_captured(
    const char* format, const uint64_t* gp, const uint8_t* fp, uint8_t* stack) {
  AndroidVaList android_va_list{
      stack,
      reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(gp) + 64),
      reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(fp) + 128),
      -56,
      -128,
  };
  return aim_bionic_vfprintf(aim_bionic_stdout, format,
                                    &android_va_list);
}

extern "C" AimBionicFormattedStdioFunction
aim_bionic_formatted_stdio_resolve(const char* soname,
                                          const char* symbol,
                                          const char* version) {
  if (!Equal(soname, "libc.so") || !Equal(version, "LIBC")) return nullptr;
  if (Equal(symbol, "fprintf")) {
    return reinterpret_cast<AimBionicFormattedStdioFunction>(
        aim_bionic_fprintf);
  }
  if (Equal(symbol, "vfprintf")) {
    return reinterpret_cast<AimBionicFormattedStdioFunction>(
        aim_bionic_vfprintf);
  }
  if (Equal(symbol, "printf")) {
    return reinterpret_cast<AimBionicFormattedStdioFunction>(
        aim_bionic_printf);
  }
  return nullptr;
}

extern "C" const char* aim_bionic_formatted_stdio_capability(
    const char* name) {
  if (Equal(name, "Android-AAPCS64-fprintf")) return "supported";
  if (Equal(name, "Android-va_list-vfprintf")) return "supported";
  if (Equal(name, "provider-local-FILE")) return "supported";
  if (Equal(name, "precommit-semantic-failure-atomic")) return "supported";
  return "unsupported";
}
