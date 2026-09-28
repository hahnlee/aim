#include "aim_bionic_wide_float.h"

#include <errno.h>
#include <fenv.h>

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <string_view>

#include <androidicuinit/android_icu_init.h>
#include <unicode/uchar.h>
#include <unicode/uversion.h>

#include "aim_bionic_allocator.h"
#include "aim_bionic_errno.h"
#include "aim_bionic_float_conversion.h"

extern bool android_icu_is_registered();

namespace {

static_assert(sizeof(AimAndroidWchar) == 4);
static_assert(U_ICU_VERSION_MAJOR_NUM == 76);
static_assert(U_ICU_VERSION_MINOR_NUM == 1);

constexpr int32_t kAndroidEnomem = 12;
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

bool IsIcuWhitespace(AimAndroidWchar code_point) {
  return u_hasBinaryProperty(static_cast<UChar32>(code_point),
                             UCHAR_WHITE_SPACE);
}

bool IsAllowedAscii(AimAndroidWchar code_point) {
  if (code_point > 0x7f) return false;
  return kAllowedAscii.find(static_cast<char>(code_point)) !=
         std::string_view::npos;
}

template <typename Result>
Result ParseWide(Result (*parser)(const char*, char**),
                 const AimAndroidWchar* input,
                 AimAndroidWchar** end_pointer) {
  HostStateGuard host_state;
  EnsureAndroidIcu76();
  host_state.Restore();

  const AimAndroidWchar* const original_input = input;
  while (IsIcuWhitespace(*input)) ++input;

  size_t maximum_length = 0;
  while (IsAllowedAscii(input[maximum_length])) ++maximum_length;
  if (maximum_length == SIZE_MAX) {
    aim_bionic_errno_store(kAndroidEnomem);
    return Result{};
  }

  const AimBionicAllocationResult allocation =
      aim_bionic_malloc_result(maximum_length + 1);
  if (allocation.pointer == nullptr) {
    aim_bionic_errno_store(allocation.bionic_errno == 0
                                      ? kAndroidEnomem
                                      : allocation.bionic_errno);
    return Result{};
  }
  auto* ascii = static_cast<char*>(allocation.pointer);
  for (size_t index = 0; index < maximum_length; ++index) {
    ascii[index] = static_cast<char>(input[index]);
  }
  ascii[maximum_length] = '\0';

  // ICU initialization and classification must not change the caller's
  // rounding mode or exception mask seen by the pinned gdtoa parser.
  host_state.Restore();
  char* ascii_end = nullptr;
  const Result result = parser(ascii, &ascii_end);
  if (ascii_end < ascii ||
      static_cast<size_t>(ascii_end - ascii) > maximum_length) {
    std::abort();
  }
  const size_t consumed = static_cast<size_t>(ascii_end - ascii);
  aim_bionic_free(ascii);

  if (end_pointer != nullptr) {
    *end_pointer = const_cast<AimAndroidWchar*>(
        consumed == 0 ? original_input : input + consumed);
  }
  return result;
}

}  // namespace

extern "C" double aim_bionic_wcstod(
    const AimAndroidWchar* input,
    AimAndroidWchar** end_pointer) {
  return ParseWide(aim_bionic_strtod, input, end_pointer);
}

extern "C" float aim_bionic_wcstof(
    const AimAndroidWchar* input,
    AimAndroidWchar** end_pointer) {
  return ParseWide(aim_bionic_strtof, input, end_pointer);
}

extern "C" void* aim_bionic_wide_float_resolve(
    const char* soname,
    const char* symbol,
    const char* version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      std::string_view(soname) != "libc.so" ||
      std::string_view(version) != "LIBC") {
    return nullptr;
  }
  if (std::string_view(symbol) == "wcstod") {
    return reinterpret_cast<void*>(&aim_bionic_wcstod);
  }
  if (std::string_view(symbol) == "wcstof") {
    return reinterpret_cast<void*>(&aim_bionic_wcstof);
  }
  // Android long double is IEEE binary128 while Darwin arm64 long double is
  // binary64. A raw function pointer cannot safely implement wcstold.
  return nullptr;
}

extern "C" int aim_bionic_wide_float_capability(
    const char* capability) {
  if (capability == nullptr) return 0;
  const std::string_view name(capability);
  return name == "wcstod-binary64" || name == "wcstof-binary32" ||
         name == "Android-unsigned-wchar32" || name == "ICU76-iswspace" ||
         name == "AOSP-gdtoa" || name == "Bionic-allowed-ASCII-span";
}

extern "C" void aim_bionic_wide_float_test_prepare_host_state(void) {
  errno = 31'993;
  fesetround(FE_UPWARD);
  feraiseexcept(FE_DIVBYZERO);
}

extern "C" int
aim_bionic_wide_float_test_host_state_is_preserved(void) {
  return errno == 31'993 && fegetround() == FE_UPWARD &&
         (fetestexcept(FE_DIVBYZERO) != 0);
}
