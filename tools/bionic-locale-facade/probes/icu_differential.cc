#include "aim_bionic_locale.h"

#include <androidicuinit/android_icu_init.h>
#include <unicode/uchar.h>
#include <unicode/uversion.h>

#include <array>
#include <cstdint>
#include <cstdio>

extern bool android_icu_is_registered();

namespace {

UChar32 IcuCodePoint(uint32_t value) {
  return static_cast<UChar32>(value);
}

bool SameTruth(int left, int right) { return (left != 0) == (right != 0); }

bool Check(uint32_t value, AimAndroidLocale locale) {
  const UChar32 code_point = IcuCodePoint(value);
  return SameTruth(aim_bionic_iswalpha_l(value, locale),
                   u_hasBinaryProperty(code_point, UCHAR_ALPHABETIC)) &&
         SameTruth(aim_bionic_iswblank_l(value, locale),
                   u_hasBinaryProperty(code_point, UCHAR_POSIX_BLANK)) &&
         SameTruth(aim_bionic_iswcntrl_l(value, locale),
                   u_charType(code_point) == U_CONTROL_CHAR) &&
         SameTruth(aim_bionic_iswdigit_l(value, locale),
                   u_isdigit(code_point)) &&
         SameTruth(aim_bionic_iswlower_l(value, locale),
                   u_hasBinaryProperty(code_point, UCHAR_LOWERCASE)) &&
         SameTruth(aim_bionic_iswprint_l(value, locale),
                   u_hasBinaryProperty(code_point, UCHAR_POSIX_PRINT)) &&
         SameTruth(aim_bionic_iswpunct_l(value, locale),
                   u_ispunct(code_point)) &&
         SameTruth(aim_bionic_iswspace_l(value, locale),
                   u_hasBinaryProperty(code_point, UCHAR_WHITE_SPACE)) &&
         SameTruth(aim_bionic_iswupper_l(value, locale),
                   u_hasBinaryProperty(code_point, UCHAR_UPPERCASE)) &&
         SameTruth(aim_bionic_iswxdigit_l(value, locale),
                   u_hasBinaryProperty(code_point, UCHAR_POSIX_XDIGIT)) &&
         aim_bionic_towlower_l(value, locale) ==
             static_cast<uint32_t>(value < 0x80
                                       ? (value >= 'A' && value <= 'Z'
                                              ? value | 0x20
                                              : value)
                                       : u_tolower(code_point)) &&
         aim_bionic_towupper_l(value, locale) ==
             static_cast<uint32_t>(value < 0x80
                                       ? (value >= 'a' && value <= 'z'
                                              ? value ^ 0x20
                                              : value)
                                       : u_toupper(code_point));
}

}  // namespace

int main() {
  if (!android_icu_is_registered()) android_icu_init();
  UVersionInfo version{};
  u_getVersion(version);
  if (version[0] != 76 || version[1] != 1) return 1;

  constexpr std::array<uint32_t, 34> kCodePoints = {
      0,          9,          10,         31,         32,
      '0',        '9',        'A',        'F',        'G',
      'a',        'f',        'g',        0x7f,       0x80,
      0xa0,       0x391,      0x3b1,      0x660,      0x2003,
      0x2014,     0x10400,    0x10428,    0x1f600,    0xd7ff,
      0xd800,     0xdfff,     0xe000,      0xfdd0,     0x10ffff,
      0x110000,   0x7fffffff, 0x80000000, UINT32_MAX,
  };
  constexpr std::array<uintptr_t, 4> kLocales = {
      0, UINTPTR_MAX, 0x12345678U, 0xfeedfaceU};
  for (uint32_t code_point : kCodePoints) {
    for (uintptr_t raw_locale : kLocales) {
      if (!Check(code_point,
                 reinterpret_cast<AimAndroidLocale>(raw_locale))) {
        std::fprintf(stderr, "ICU differential failed U+%08x locale=%zx\n",
                     code_point, static_cast<size_t>(raw_locale));
        return 2;
      }
    }
  }
  std::puts("bionic-locale-icu-differential: PASS ICU=76.1 codepoints=34 locales=null+global+invalid Bionic-_l=locale-ignored");
  return 0;
}
