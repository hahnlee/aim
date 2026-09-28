#include "aim_bionic_swprintf.h"

#include "aim_bionic_errno.h"
#include "aim_bionic_format.h"

#include <cfenv>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <limits>

namespace {

constexpr int kEinval = 22;
constexpr int kEnomem = 12;
constexpr int kEnotsup = 95;
constexpr int kEoverflow = 75;
constexpr size_t kMaxText = 8192;

struct Fpi {
  int nbits;
  int emin;
  int emax;
  int rounding;
  int sudden_underflow;
};

struct AndroidVaList {
  void* stack;
  void* gr_top;
  void* vr_top;
  int32_t gr_offs;
  int32_t vr_offs;
};

extern "C" char* aim_aosp_gdtoa(Fpi* fpi, int binary_exponent,
                                          uint32_t* bits, int* kind,
                                          int mode, int digits,
                                          int* decimal_point, char** end);
extern "C" void __freedtoa(char* value);

void Fail(int value) { aim_bionic_errno_store(value); }

bool Equal(const char* left, const char* right) {
  return left != nullptr && right != nullptr && std::strcmp(left, right) == 0;
}

bool IsFormat(const AimAndroidWchar* format, bool binary128) {
  if (format == nullptr || format[0] != '%' || format[1] == 0) return false;
  if (binary128) {
    return format[1] == 'L' && format[2] == 'f' && format[3] == 0;
  }
  return format[1] == 'f' && format[2] == 0;
}

int CopyAscii(AimAndroidWchar* output, size_t capacity,
              const char* text, size_t length) {
  if (output == nullptr || capacity == 0) {
    Fail(kEinval);
    return -1;
  }
  if (length >= capacity || length > static_cast<size_t>(std::numeric_limits<int>::max())) {
    output[capacity - 1] = 0;
    Fail(kEoverflow);
    return -1;
  }
  for (size_t i = 0; i < length; ++i) {
    output[i] = static_cast<unsigned char>(text[i]);
  }
  output[length] = 0;
  return static_cast<int>(length);
}

int FormatDouble(AimAndroidWchar* output, size_t capacity,
                 const uint8_t* fp_registers, uint8_t* stack) {
  char text[1024];
  AndroidVaList arguments{
      stack,
      nullptr,
      const_cast<uint8_t*>(fp_registers) + 128,
      0,
      -128,
  };
  const int result = aim_bionic_vsnprintf(
      text, sizeof(text), "%f", &arguments);
  if (result < 0) return -1;
  return CopyAscii(output, capacity, text, static_cast<size_t>(result));
}

int RoundingMode(bool negative) {
  int rounding = 1;
  switch (std::fegetround()) {
    case FE_TOWARDZERO:
      rounding = 0;
      break;
    case FE_UPWARD:
      rounding = 2;
      break;
    case FE_DOWNWARD:
      rounding = 3;
      break;
    default:
      rounding = 1;
      break;
  }
  if (negative && (rounding == 2 || rounding == 3)) rounding = 5 - rounding;
  return rounding;
}

int FormatBinary128(AimAndroidWchar* output, size_t capacity,
                    const uint8_t* fp_registers) {
  uint64_t low = 0;
  uint64_t high = 0;
  std::memcpy(&low, fp_registers, sizeof(low));
  std::memcpy(&high, fp_registers + sizeof(low), sizeof(high));
  const bool negative = (high >> 63) != 0;
  const uint32_t exponent = static_cast<uint32_t>((high >> 48) & 0x7fff);
  const bool fraction_zero = low == 0 && (high & 0x0000ffffffffffffULL) == 0;

  char text[kMaxText];
  size_t length = 0;
  if (negative) text[length++] = '-';
  if (exponent == 0x7fff) {
    const char* special = fraction_zero ? "inf" : "nan";
    std::memcpy(text + length, special, 3);
    length += 3;
    return CopyAscii(output, capacity, text, length);
  }
  if (exponent == 0 && fraction_zero) {
    std::memcpy(text + length, "0.000000", 8);
    length += 8;
    return CopyAscii(output, capacity, text, length);
  }

  uint32_t bits[4] = {
      static_cast<uint32_t>(low),
      static_cast<uint32_t>(low >> 32),
      static_cast<uint32_t>(high),
      static_cast<uint32_t>((high >> 32) & 0xffff),
  };
  int binary_exponent;
  int kind;
  if (exponent == 0) {
    binary_exponent = -16494;
    kind = 2;
  } else {
    bits[3] |= 1U << 16;
    binary_exponent = static_cast<int>(exponent) - 16383 - 112;
    kind = 1;
  }
  Fpi fpi{113, -16494, 16271, RoundingMode(negative), 0};
  int decimal_point = 0;
  char* end = nullptr;
  char* digits = aim_aosp_gdtoa(&fpi, binary_exponent, bits, &kind,
                                        3, 6, &decimal_point, &end);
  if (digits == nullptr || end == nullptr || end < digits) {
    if (digits != nullptr) __freedtoa(digits);
    Fail(kEnomem);
    return -1;
  }
  const size_t digit_count = static_cast<size_t>(end - digits);
  if (decimal_point > static_cast<int>(kMaxText - 16)) {
    __freedtoa(digits);
    Fail(kEoverflow);
    return -1;
  }

  const int integer_digits = decimal_point > 0 ? decimal_point : 1;
  for (int index = 0; index < integer_digits; ++index) {
    const int source = decimal_point > 0 ? index : -1;
    text[length++] = source >= 0 && static_cast<size_t>(source) < digit_count
                         ? digits[source]
                         : '0';
  }
  text[length++] = '.';
  for (int index = 0; index < 6; ++index) {
    const int source = decimal_point + index;
    text[length++] = source >= 0 && static_cast<size_t>(source) < digit_count
                         ? digits[source]
                         : '0';
  }
  __freedtoa(digits);
  return CopyAscii(output, capacity, text, length);
}

}  // namespace

extern "C" int aim_bionic_swprintf_captured(
    AimAndroidWchar* output, size_t capacity,
    const AimAndroidWchar* format, const uint8_t* fp_registers,
    uint8_t* stack) {
  const int saved_host_errno = errno;
  int result;
  if (output == nullptr || format == nullptr || fp_registers == nullptr || capacity == 0) {
    Fail(kEinval);
    result = -1;
  } else if (IsFormat(format, false)) {
    result = FormatDouble(output, capacity, fp_registers, stack);
  } else if (IsFormat(format, true)) {
    result = FormatBinary128(output, capacity, fp_registers);
  } else {
    Fail(kEnotsup);
    result = -1;
  }
  errno = saved_host_errno;
  return result;
}

extern "C" AimBionicSwprintfFunction aim_bionic_swprintf_resolve(
    const char* soname, const char* symbol, const char* version) {
  if (!Equal(soname, "libc.so") || !Equal(symbol, "swprintf") ||
      !Equal(version, "LIBC")) {
    return nullptr;
  }
  return reinterpret_cast<AimBionicSwprintfFunction>(
      aim_bionic_swprintf);
}
