#include "aim_bionic_scanf.h"

#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>
#include <string_view>

#include "aim_bionic_errno.h"
#include "aim_bionic_float_conversion.h"
#include "aim_bionic_numeric.h"
#include "aim_bionic_stdio.h"

extern "C" void aim_bionic_strtold_raw(const char*, char**, void*);

namespace {
constexpr int kEnotsup = 95;
constexpr size_t kNumericBuffer = 513;

struct AndroidVaList {
  void* stack;
  void* gr_top;
  void* vr_top;
  int32_t gr_offs;
  int32_t vr_offs;
};
static_assert(sizeof(AndroidVaList) == 32);
struct Cursor {
  uint8_t* stack;
  uint8_t* gr_top;
  int32_t gr_offs;
};
enum class Length { kNone, kHh, kH, kL, kLl, kJ, kZ, kT, kBigL };
struct Spec {
  bool suppress = false;
  size_t width = 0;
  Length length = Length::kNone;
  char conversion = 0;
  bool scanset[256]{};
};

bool Space(unsigned char c) {
  return c == ' ' || c == '\t' || c == '\n' || c == '\v' || c == '\f' ||
         c == '\r';
}
uint8_t* Align8(uint8_t* p) {
  return reinterpret_cast<uint8_t*>((reinterpret_cast<uintptr_t>(p) + 7) &
                                    ~uintptr_t(7));
}
uint64_t Gp(Cursor* cursor) {
  if (cursor->gr_offs < 0) {
    uint64_t value;
    std::memcpy(&value, cursor->gr_top + cursor->gr_offs, 8);
    cursor->gr_offs += 8;
    return value;
  }
  cursor->stack = Align8(cursor->stack);
  uint64_t value;
  std::memcpy(&value, cursor->stack, 8);
  cursor->stack += 8;
  return value;
}
void* Pointer(Cursor* cursor) {
  return reinterpret_cast<void*>(static_cast<uintptr_t>(Gp(cursor)));
}
void Fail(int error) { aim_bionic_errno_store(error); }

bool ParseScanset(const char*& format, bool table[256]) {
  bool negate = *format == '^';
  if (negate) ++format;
  for (size_t index = 0; index < 256; ++index) table[index] = negate;
  bool any = false;
  unsigned char previous = 0;
  if (*format == ']') {
    table[static_cast<unsigned char>(*format++)] = !negate;
    previous = ']';
    any = true;
  }
  while (*format != '\0' && *format != ']') {
    unsigned char current = static_cast<unsigned char>(*format++);
    if (current == '-' && any && *format != ']' && *format != '\0') {
      unsigned char end = static_cast<unsigned char>(*format++);
      if (previous <= end) {
        for (unsigned value = previous; value <= end; ++value)
          table[value] = !negate;
      } else {
        table[current] = !negate;
        table[end] = !negate;
      }
      previous = end;
    } else {
      table[current] = !negate;
      previous = current;
    }
    any = true;
  }
  if (*format != ']' || !any) return false;
  ++format;
  return true;
}

bool ParseSpec(const char*& format, Spec* spec) {
  if (*format == '*') {
    spec->suppress = true;
    ++format;
  }
  while (*format >= '0' && *format <= '9') {
    unsigned digit = static_cast<unsigned>(*format++ - '0');
    if (spec->width > (std::numeric_limits<size_t>::max() - digit) / 10) {
      return false;
    }
    spec->width = spec->width * 10 + digit;
  }
  if (*format == 'h') {
    spec->length = *++format == 'h' ? (++format, Length::kHh) : Length::kH;
  } else if (*format == 'l') {
    spec->length = *++format == 'l' ? (++format, Length::kLl) : Length::kL;
  } else if (*format == 'j') {
    ++format;
    spec->length = Length::kJ;
  } else if (*format == 'z') {
    ++format;
    spec->length = Length::kZ;
  } else if (*format == 't') {
    ++format;
    spec->length = Length::kT;
  } else if (*format == 'L') {
    ++format;
    spec->length = Length::kBigL;
  } else if (*format == 'q') {
    ++format;
    spec->length = Length::kLl;
  } else if (*format == 'w') {
    ++format;
    bool fast = false;
    if (*format == 'f') {
      fast = true;
      ++format;
    }
    unsigned bits = 0;
    while (*format >= '0' && *format <= '9') {
      unsigned digit = static_cast<unsigned>(*format++ - '0');
      if (bits > (std::numeric_limits<unsigned>::max() - digit) / 10)
        return false;
      bits = bits * 10 + digit;
    }
    if (bits == 8)
      spec->length = Length::kHh;
    else if (bits == 16)
      spec->length = fast ? Length::kLl : Length::kH;
    else if (bits == 32)
      spec->length = fast ? Length::kLl : Length::kNone;
    else if (bits == 64)
      spec->length = Length::kLl;
    else
      return false;
  } else if (*format == 'm') {
    return false;
  }
  spec->conversion = *format++;
  if (spec->conversion == 'D') {
    spec->conversion = 'd';
    spec->length = Length::kL;
  } else if (spec->conversion == 'O') {
    spec->conversion = 'o';
    spec->length = Length::kL;
  }
  if (spec->conversion == '[' && !ParseScanset(format, spec->scanset))
    return false;
  const char* allowed = "diouxXbpaAeEfFgGsc[n%";
  if (spec->conversion == '\0' ||
      std::strchr(allowed, spec->conversion) == nullptr)
    return false;
  if (spec->conversion == '%' &&
      (spec->suppress || spec->width || spec->length != Length::kNone))
    return false;
  const bool floating =
      std::strchr("aAeEfFgG", spec->conversion) != nullptr;
  if (floating && spec->length != Length::kNone &&
      spec->length != Length::kL && spec->length != Length::kBigL)
    return false;
  const bool text = spec->conversion == 's' || spec->conversion == 'c' ||
                    spec->conversion == '[';
  if (text && spec->length != Length::kNone && spec->length != Length::kL)
    return false;
  const bool integer =
      std::strchr("diouxXb", spec->conversion) != nullptr;
  if (integer && spec->length == Length::kBigL) return false;
  if (spec->conversion == 'p' && spec->length != Length::kNone) return false;
  if (spec->conversion == 'n' && spec->length == Length::kBigL) return false;
  return true;
}

size_t Utf8(const unsigned char* input, uint32_t* output) {
  if (input[0] < 0x80) {
    *output = input[0];
    return input[0] == 0 ? 0 : 1;
  }
  unsigned need;
  uint32_t value;
  if ((input[0] & 0xe0) == 0xc0) {
    need = 2;
    value = input[0] & 0x1f;
  } else if ((input[0] & 0xf0) == 0xe0) {
    need = 3;
    value = input[0] & 0x0f;
  } else if ((input[0] & 0xf8) == 0xf0) {
    need = 4;
    value = input[0] & 7;
  } else {
    return 0;
  }
  for (unsigned index = 1; index < need; ++index) {
    if ((input[index] & 0xc0) != 0x80) return 0;
    value = (value << 6) | (input[index] & 0x3f);
  }
  if ((need == 2 && value < 0x80) || (need == 3 && value < 0x800) ||
      (need == 4 && value < 0x10000) || value > 0x10ffff ||
      (value >= 0xd800 && value <= 0xdfff))
    return 0;
  *output = value;
  return need;
}

void StoreSigned(void* output, Length length, int64_t value) {
  switch (length) {
    case Length::kHh: *static_cast<int8_t*>(output) = value; break;
    case Length::kH: *static_cast<int16_t*>(output) = value; break;
    case Length::kNone: *static_cast<int32_t*>(output) = value; break;
    default: *static_cast<int64_t*>(output) = value; break;
  }
}
void StoreUnsigned(void* output, Length length, uint64_t value) {
  switch (length) {
    case Length::kHh: *static_cast<uint8_t*>(output) = value; break;
    case Length::kH: *static_cast<uint16_t*>(output) = value; break;
    case Length::kNone: *static_cast<uint32_t*>(output) = value; break;
    default: *static_cast<uint64_t*>(output) = value; break;
  }
}

struct ConsumptionGuard {
  const char* begin;
  const char*& current;
  size_t* output;
  ~ConsumptionGuard() {
    *output = static_cast<size_t>(current - begin);
  }
};

int Scan(const char* input, const char* format, Cursor cursor,
         size_t* consumed) {
  if (consumed == nullptr) {
    Fail(kEnotsup);
    return -1;
  }
  *consumed = 0;
  if (input == nullptr || format == nullptr) {
    Fail(kEnotsup);
    return -1;
  }
  const char* begin = input;
  ConsumptionGuard guard{begin, input, consumed};
  int assigned = 0;
  while (*format != '\0') {
    if (Space(static_cast<unsigned char>(*format))) {
      while (Space(static_cast<unsigned char>(*format))) ++format;
      while (Space(static_cast<unsigned char>(*input))) ++input;
      continue;
    }
    if (*format != '%') {
      if (*input == '\0') return assigned == 0 ? -1 : assigned;
      if (*input++ != *format++) return assigned;
      continue;
    }
    ++format;
    if (*format == '%') {
      ++format;
      if (*input == '\0') return assigned == 0 ? -1 : assigned;
      if (*input++ != '%') return assigned;
      continue;
    }
    Spec spec;
    if (!ParseSpec(format, &spec)) {
      Fail(kEnotsup);
      return -1;
    }
    if (spec.conversion == 'n') {
      if (!spec.suppress) StoreSigned(Pointer(&cursor), spec.length, input - begin);
      continue;
    }
    const bool no_skip = spec.conversion == 'c' || spec.conversion == '[';
    if (!no_skip) while (Space(static_cast<unsigned char>(*input))) ++input;
    if (*input == '\0') return assigned == 0 ? -1 : assigned;

    if (std::strchr("diouxXbp", spec.conversion) != nullptr) {
      int base = spec.conversion == 'd' || spec.conversion == 'u' ? 10
                 : spec.conversion == 'o'                         ? 8
                 : spec.conversion == 'b'                         ? 2
                 : spec.conversion == 'i'                         ? 0
                                                                  : 16;
      size_t width = spec.width == 0 ? kNumericBuffer - 1
                                     : std::min(spec.width, kNumericBuffer - 1);
      char buffer[kNumericBuffer];
      size_t count = 0;
      while (count < width && input[count] != '\0') {
        buffer[count] = input[count];
        ++count;
      }
      buffer[count] = '\0';
      char* end = nullptr;
      const bool signed_conversion =
          spec.conversion == 'd' || spec.conversion == 'i';
      int64_t signed_value = 0;
      uint64_t unsigned_value = 0;
      const int32_t saved_errno = aim_bionic_errno_load();
      if (signed_conversion)
        signed_value = aim_bionic_strtoll(buffer, &end, base);
      else
        unsigned_value = aim_bionic_strtoull(buffer, &end, base);
      size_t consumed = static_cast<size_t>(end - buffer);
      if (spec.suppress) aim_bionic_errno_store(saved_errno);
      if (consumed == 0) return assigned;
      input += consumed;
      if (!spec.suppress) {
        void* output = Pointer(&cursor);
        if (spec.conversion == 'p')
          *static_cast<void**>(output) = reinterpret_cast<void*>(unsigned_value);
        else if (signed_conversion)
          StoreSigned(output, spec.length, signed_value);
        else
          StoreUnsigned(output, spec.length, unsigned_value);
        ++assigned;
      }
      continue;
    }

    if (std::strchr("aAeEfFgG", spec.conversion) != nullptr) {
      size_t width = spec.width == 0 ? kNumericBuffer - 1
                                     : std::min(spec.width, kNumericBuffer - 1);
      char buffer[kNumericBuffer];
      size_t count = 0;
      while (count < width && input[count] != '\0') buffer[count] = input[count], ++count;
      buffer[count] = '\0';
      char* end = nullptr;
      alignas(16) unsigned char binary128[16]{};
      float f = 0;
      double d = 0;
      const int32_t saved_errno = aim_bionic_errno_load();
      if (spec.length == Length::kBigL)
        aim_bionic_strtold_raw(buffer, &end, binary128);
      else if (spec.length == Length::kL)
        d = aim_bionic_strtod(buffer, &end);
      else
        f = aim_bionic_strtof(buffer, &end);
      size_t consumed = static_cast<size_t>(end - buffer);
      if (spec.suppress) aim_bionic_errno_store(saved_errno);
      if (consumed == 0) return assigned;
      input += consumed;
      if (!spec.suppress) {
        void* output = Pointer(&cursor);
        if (spec.length == Length::kBigL)
          std::memcpy(output, binary128, 16);
        else if (spec.length == Length::kL)
          *static_cast<double*>(output) = d;
        else
          *static_cast<float*>(output) = f;
        ++assigned;
      }
      continue;
    }

    size_t width = spec.width == 0
                       ? (spec.conversion == 'c' ? 1 : SIZE_MAX)
                       : spec.width;
    size_t units = 0;
    const char* cursor_input = input;
    void* output = spec.suppress ? nullptr : Pointer(&cursor);
    while (*cursor_input != '\0' && units < width) {
      unsigned char byte = static_cast<unsigned char>(*cursor_input);
      if (spec.conversion == 's' && Space(byte)) break;
      if (spec.conversion == '[' && !spec.scanset[byte]) break;
      if (spec.length == Length::kL) {
        uint32_t code_point;
        size_t bytes = Utf8(reinterpret_cast<const unsigned char*>(cursor_input),
                            &code_point);
        if (bytes == 0 ||
            (spec.conversion == '[' &&
             (code_point > 255 || !spec.scanset[code_point])))
          break;
        if (output) static_cast<uint32_t*>(output)[units] = code_point;
        cursor_input += bytes;
      } else {
        if (output) static_cast<char*>(output)[units] = *cursor_input;
        ++cursor_input;
      }
      ++units;
    }
    if (units == 0 || (spec.conversion == 'c' && units < width))
      return assigned;
    input = cursor_input;
    if (!spec.suppress) {
      if (spec.conversion != 'c') {
        if (spec.length == Length::kL)
          static_cast<uint32_t*>(output)[units] = 0;
        else
          static_cast<char*>(output)[units] = '\0';
      }
      ++assigned;
    }
  }
  return assigned;
}

Cursor FromVa(const void* opaque) {
  AndroidVaList value{};
  std::memcpy(&value, opaque, sizeof(value));
  return {static_cast<uint8_t*>(value.stack), static_cast<uint8_t*>(value.gr_top),
          value.gr_offs};
}

struct FileScanContext {
  const char* format;
  Cursor cursor;
};

int ScanFileInput(const char* input, size_t length, void* opaque,
                  size_t* consumed) {
  (void)length;
  auto* context = static_cast<FileScanContext*>(opaque);
  if (context == nullptr) {
    Fail(kEnotsup);
    return -1;
  }
  return Scan(input, context->format, context->cursor, consumed);
}
}  // namespace

extern "C" int aim_bionic_sscanf_captured(
    const char* input, const char* format, const uint64_t* gp,
    const uint8_t*, uint8_t* stack) {
  Cursor cursor{stack, reinterpret_cast<uint8_t*>(const_cast<uint64_t*>(gp)) + 64,
                -48};
  size_t consumed = 0;
  const int result = Scan(input, format, cursor, &consumed);
  if (std::getenv("AIM_DEBUG_SCANF") != nullptr) {
    std::fprintf(stderr, "DARWIN sscanf input=%s format=%s result=%d consumed=%zu\n",
                 input == nullptr ? "<null>" : input,
                 format == nullptr ? "<null>" : format, result, consumed);
  }
  return result;
}

extern "C" int aim_bionic_fscanf_captured(
    AimAndroidFile* stream, const char* format, const uint64_t* gp,
    const uint8_t*, uint8_t* stack) {
  FileScanContext context{
      format,
      Cursor{stack,
             reinterpret_cast<uint8_t*>(const_cast<uint64_t*>(gp)) + 64,
             -48}};
  const int result =
      aim_bionic_stdio_scan_core(stream, &ScanFileInput, &context);
  if (std::getenv("AIM_DEBUG_SCANF") != nullptr) {
    std::fprintf(stderr, "DARWIN fscanf format=%s result=%d stream=%p\n",
                 format == nullptr ? "<null>" : format, result,
                 static_cast<void*>(stream));
  }
  return result;
}

extern "C" int aim_bionic_vsscanf(const char* input,
                                          const char* format,
                                          const void* android_va_list) {
  if (android_va_list == nullptr) {
    Fail(kEnotsup);
    return -1;
  }
  size_t consumed = 0;
  return Scan(input, format, FromVa(android_va_list), &consumed);
}

extern "C" void* aim_bionic_scanf_resolve(const char* soname,
                                                   const char* symbol,
                                                   const char* version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      std::string_view(soname) != "libc.so" ||
      std::string_view(version) != "LIBC")
    return nullptr;
  if (std::string_view(symbol) == "sscanf")
    return reinterpret_cast<void*>(aim_bionic_sscanf);
  if (std::string_view(symbol) == "vsscanf")
    return reinterpret_cast<void*>(&aim_bionic_vsscanf);
  if (std::string_view(symbol) == "fscanf")
    return reinterpret_cast<void*>(aim_bionic_fscanf);
  return nullptr;
}

extern "C" const char* aim_bionic_scanf_capability(
    const char* capability) {
  if (capability == nullptr) return "invalid";
  const std::string_view value(capability);
  if (value == "Android-AAPCS64-variadic" || value == "Android-va_list32" ||
      value == "Bionic-string-reader" || value == "Bionic-FILE-reader" ||
      value == "binary128-raw-output")
    return "supported";
  return "unsupported";
}
