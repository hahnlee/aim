#include "darwin_art_bionic_binary128_conversion.h"

#include <errno.h>
#include <fenv.h>

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <cmath>
#include <limits>
#include <string_view>

#include <androidicuinit/android_icu_init.h>
#include <unicode/uchar.h>
#include <unicode/uversion.h>

#include "darwin_art_bionic_allocator.h"
#include "darwin_art_bionic_errno.h"

extern "C" int darwin_art_aosp_strtorQ(const char*, char**, int, void*);
extern bool android_icu_is_registered();

namespace {

static_assert(sizeof(DarwinArtAndroidWchar) == 4);
static_assert(U_ICU_VERSION_MAJOR_NUM == 76);
static_assert(U_ICU_VERSION_MINOR_NUM == 1);

constexpr int32_t kAndroidEnomem = 12;
constexpr int32_t kAndroidErange = 34;
constexpr std::string_view kAllowedAscii =
    "-+0123456789.xXeEpP()nNaAiIfFtTyY";

struct HostStateGuard {
  int saved_errno{errno};
  fenv_t saved_environment{};
  HostStateGuard() { fegetenv(&saved_environment); }
  ~HostStateGuard() { Restore(); }
  void Restore() const {
    fesetenv(&saved_environment);
    errno = saved_errno;
  }
};

int GdtoaRoundingMode() {
  switch (fegetround()) {
    case FE_TOWARDZERO:
      return 0;
    case FE_TONEAREST:
      return 1;
    case FE_UPWARD:
      return 2;
    case FE_DOWNWARD:
      return 3;
    default:
      std::abort();
  }
}

void ParseAscii(const char* input, char** end_pointer, void* output) {
  HostStateGuard host_state;
  const int rounding = GdtoaRoundingMode();
  errno = 0;
  darwin_art_aosp_strtorQ(input, end_pointer, rounding, output);
  const int parser_errno = errno;
  host_state.Restore();
  if (parser_errno == ERANGE) darwin_art_bionic_errno_store(kAndroidErange);
}

void EnsureAndroidIcu76() {
  static std::once_flag once;
  std::call_once(once, [] {
    if (!android_icu_is_registered()) android_icu_init();
    if (!android_icu_is_registered()) std::abort();
    UVersionInfo version{};
    u_getVersion(version);
    if (version[0] != 76 || version[1] != 1) std::abort();
  });
}

bool IsIcuWhitespace(DarwinArtAndroidWchar code_point) {
  return u_hasBinaryProperty(static_cast<UChar32>(code_point),
                             UCHAR_WHITE_SPACE);
}

bool IsAllowedAscii(DarwinArtAndroidWchar code_point) {
  return code_point <= 0x7f &&
         kAllowedAscii.find(static_cast<char>(code_point)) !=
             std::string_view::npos;
}

}  // namespace

namespace {
double Binary128ToDouble(const void* input) {
  uint64_t words[2]{};
  std::memcpy(words, input, sizeof(words));
  const bool negative = (words[1] >> 63) != 0;
  const unsigned exponent = static_cast<unsigned>((words[1] >> 48) & 0x7fff);
  const uint64_t fraction_high = words[1] & UINT64_C(0x0000ffffffffffff);
  if (exponent == 0x7fff) {
    if (fraction_high != 0 || words[0] != 0)
      return std::numeric_limits<double>::quiet_NaN();
    return negative ? -INFINITY : INFINITY;
  }
  double significand = std::ldexp(static_cast<double>(fraction_high), -48) +
                       std::ldexp(static_cast<double>(words[0]), -112);
  int power = 1 - 16383;
  if (exponent != 0) {
    significand += 1.0;
    power = static_cast<int>(exponent) - 16383;
  }
  const double value = std::ldexp(significand, power);
  return negative ? -value : value;
}

void DoubleToBinary128(double value, void* output) {
  uint64_t words[2]{};
  uint64_t bits = 0;
  std::memcpy(&bits, &value, sizeof(bits));
  const uint64_t sign = bits >> 63;
  const unsigned exponent = static_cast<unsigned>((bits >> 52) & 0x7ff);
  const uint64_t fraction = bits & UINT64_C(0x000fffffffffffff);
  if (exponent == 0x7ff) {
    words[1] = (sign << 63) | (UINT64_C(0x7fff) << 48);
    if (fraction != 0) words[1] |= UINT64_C(0x0000800000000000);
  } else if (exponent == 0 && fraction == 0) {
    words[1] = sign << 63;
  } else {
    int binary_exponent = 0;
    const double normalized = std::frexp(std::fabs(value), &binary_exponent) * 2.0;
    const uint64_t normalized_bits = [&] {
      uint64_t result = 0;
      std::memcpy(&result, &normalized, sizeof(result));
      return result;
    }();
    const uint64_t normalized_fraction =
        normalized_bits & UINT64_C(0x000fffffffffffff);
    const uint64_t quad_exponent =
        static_cast<uint64_t>(binary_exponent - 1 + 16383);
    words[0] = normalized_fraction << 60;
    words[1] = (sign << 63) | (quad_exponent << 48) |
               (normalized_fraction >> 4);
  }
  std::memcpy(output, words, sizeof(words));
}
}  // namespace

extern "C" void darwin_art_bionic_powl_raw(const void* base,
                                             const void* exponent,
                                             void* output) {
  const double result =
      std::pow(Binary128ToDouble(base), Binary128ToDouble(exponent));
  DoubleToBinary128(result, output);
}

extern "C" void darwin_art_bionic_strtold_raw(const char* input,
                                                char** end_pointer,
                                                void* output) {
  ParseAscii(input, end_pointer, output);
}

extern "C" void darwin_art_bionic_strtold_l_raw(const char* input,
                                                  char** end_pointer,
                                                  const void* locale,
                                                  void* output) {
  (void)locale;  // Exact pinned Bionic locale wrapper: ignored, never read.
  ParseAscii(input, end_pointer, output);
}

extern "C" void darwin_art_bionic_wcstold_raw(
    const DarwinArtAndroidWchar* input,
    DarwinArtAndroidWchar** end_pointer,
    void* output) {
  HostStateGuard host_state;
  EnsureAndroidIcu76();
  host_state.Restore();

  const DarwinArtAndroidWchar* const original_input = input;
  while (IsIcuWhitespace(*input)) ++input;
  size_t maximum_length = 0;
  while (IsAllowedAscii(input[maximum_length])) ++maximum_length;
  if (maximum_length == SIZE_MAX) {
    std::memset(output, 0, 16);
    darwin_art_bionic_errno_store(kAndroidEnomem);
    return;
  }
  const DarwinArtBionicAllocationResult allocation =
      darwin_art_bionic_malloc_result(maximum_length + 1);
  if (allocation.pointer == nullptr) {
    std::memset(output, 0, 16);
    darwin_art_bionic_errno_store(allocation.bionic_errno == 0
                                      ? kAndroidEnomem
                                      : allocation.bionic_errno);
    return;
  }
  auto* ascii = static_cast<char*>(allocation.pointer);
  for (size_t index = 0; index < maximum_length; ++index) {
    ascii[index] = static_cast<char>(input[index]);
  }
  ascii[maximum_length] = '\0';

  host_state.Restore();
  char* ascii_end = nullptr;
  ParseAscii(ascii, &ascii_end, output);
  if (ascii_end < ascii ||
      static_cast<size_t>(ascii_end - ascii) > maximum_length) {
    std::abort();
  }
  const size_t consumed = static_cast<size_t>(ascii_end - ascii);
  darwin_art_bionic_free(ascii);
  if (end_pointer != nullptr) {
    *end_pointer = const_cast<DarwinArtAndroidWchar*>(
        consumed == 0 ? original_input : input + consumed);
  }
}

extern "C" void* darwin_art_bionic_binary128_conversion_resolve(
    const char* soname,
    const char* symbol,
    const char* version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      (std::string_view(soname) != "libc.so" &&
       std::string_view(soname) != "libm.so") ||
      std::string_view(version) != "LIBC") {
    return nullptr;
  }
  const std::string_view name(symbol);
  if (std::string_view(soname) == "libm.so" && name == "powl")
    return (void*)darwin_art_bionic_powl;
  if (std::string_view(soname) != "libc.so") return nullptr;
  if (name == "strtold") return (void*)darwin_art_bionic_strtold;
  if (name == "strtold_l") return (void*)darwin_art_bionic_strtold_l;
  if (name == "wcstold") return (void*)darwin_art_bionic_wcstold;
  return nullptr;
}

extern "C" int darwin_art_bionic_binary128_conversion_capability(
    const char* capability) {
  if (capability == nullptr) return 0;
  const std::string_view name(capability);
  return name == "Android-AAPCS64-binary128-q0" ||
         name == "AOSP-gdtoa-strtorQ" ||
         name == "strtold_l-locale-ignored" ||
         name == "Android-unsigned-wchar32" || name == "ICU76-iswspace" ||
         name == "conversion-only-no-binary128-arithmetic";
}
