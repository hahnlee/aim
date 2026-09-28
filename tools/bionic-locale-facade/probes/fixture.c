#include <errno.h>
#include <limits.h>
#include <locale.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>
#include <wctype.h>

static int StringEquals(const char* left, const char* right) {
  while (*left == *right && *left != '\0') {
    ++left;
    ++right;
  }
  return *left == *right;
}

extern int imported_isalnum(int) __asm__("isalnum");
extern int imported_isalpha(int) __asm__("isalpha");
extern int imported_islower(int) __asm__("islower");
extern int imported_isupper(int) __asm__("isupper");
extern int imported_isxdigit(int) __asm__("isxdigit");
extern int imported_tolower(int) __asm__("tolower");
extern int imported_toupper(int) __asm__("toupper");
extern int imported_tolower_l(int, locale_t) __asm__("tolower_l");
extern int imported_toupper_l(int, locale_t) __asm__("toupper_l");

static int (*volatile call_isalnum)(int) = imported_isalnum;
static int (*volatile call_isalpha)(int) = imported_isalpha;
static int (*volatile call_islower)(int) = imported_islower;
static int (*volatile call_isupper)(int) = imported_isupper;
static int (*volatile call_isxdigit)(int) = imported_isxdigit;
static int (*volatile call_tolower)(int) = imported_tolower;
static int (*volatile call_toupper)(int) = imported_toupper;
static int (*volatile call_tolower_l)(int, locale_t) = imported_tolower_l;
static int (*volatile call_toupper_l)(int, locale_t) = imported_toupper_l;

__attribute__((visibility("default"))) int bionic_locale_fixture_basic(void) {
  errno = 777;
  if (!StringEquals(setlocale(LC_ALL, 0), "C.UTF-8") ||
      __ctype_get_mb_cur_max() != 4 || errno != 777)
    return 1;
  if (!StringEquals(setlocale(LC_CTYPE, "C"), "C") ||
      __ctype_get_mb_cur_max() != 1)
    return 2;
  if (!StringEquals(setlocale(LC_ALL, "POSIX"), "C") ||
      !StringEquals(setlocale(LC_ALL, ""), "C.UTF-8") ||
      !StringEquals(setlocale(LC_ALL, "en_US.UTF-8"), "C.UTF-8"))
    return 3;
  if (!call_isalnum('7') || !call_isalpha('A') || !call_islower('a') ||
      !call_isupper('A') || !call_isxdigit('f') ||
      call_tolower('A') != 'a' || call_toupper('a') != 'A')
    return 5;
  locale_t invalid_locale = (locale_t)(uintptr_t)0x12345678U;
  if (call_tolower_l('A', invalid_locale) != 'a' ||
      call_toupper_l('a', invalid_locale) != 'A' ||
      call_tolower_l(EOF, invalid_locale) != EOF ||
      call_toupper_l(EOF, invalid_locale) != EOF)
    return 13;
  errno = 0;
  if (setlocale(LC_ALL, "ko_KR.UTF-8") != 0 || errno != ENOENT) return 4;
  errno = 0;
  if (setlocale(99, "C") != 0 || errno != EINVAL) return 5;

  locale_t c = newlocale(LC_ALL_MASK, "C", 0);
  locale_t utf8 = newlocale(LC_ALL_MASK, "C.UTF-8", 0);
  if (c == 0 || utf8 == 0 || c == utf8) return 6;
  if (uselocale(c) != LC_GLOBAL_LOCALE || __ctype_get_mb_cur_max() != 1)
    return 7;
  if (uselocale(utf8) != c || __ctype_get_mb_cur_max() != 4) return 8;
  if (uselocale(0) != utf8 || uselocale(LC_GLOBAL_LOCALE) != utf8 ||
      __ctype_get_mb_cur_max() != 4)
    return 9;

  struct lconv* values = localeconv();
  if (values == 0 || values->decimal_point[0] != '.' ||
      values->decimal_point[1] != '\0' || values->thousands_sep[0] != '\0' ||
      values->int_frac_digits != CHAR_MAX ||
      values->int_n_sign_posn != CHAR_MAX)
    return 10;
  freelocale(c);
  freelocale(utf8);

  errno = 0;
  if (newlocale(LC_ALL_MASK, "de_DE.UTF-8", 0) != 0 || errno != ENOENT)
    return 11;
  errno = 0;
  if (newlocale((int)0x40000000, "C", 0) != 0 || errno != EINVAL)
    return 12;
  return 42;
}

__attribute__((visibility("default"))) int bionic_locale_fixture_multibyte(void) {
  mbstate_t state = {};
  wchar_t wide = 0;
  if (mbrtowc(&wide, "\xf0\x9f", 2, &state) != (size_t)-2) return 20;
  if (mbrtowc(&wide, "\x98\x80", 2, &state) != 2 ||
      (uint32_t)wide != 0x1f600)
    return 21;
  errno = 0;
  if (mbrtowc(&wide, "\xe0\x80\x80", 3, &state) != (size_t)-1 ||
      errno != EILSEQ)
    return 22;
  if (mbrtowc(&wide, "A", 1, &state) != 1 || wide != L'A') return 23;
  if (mbrtowc(&wide, "", 0, &state) != (size_t)-2) return 24;
  if (mbrlen("\xc2\xa2", 2, &state) != 2) return 25;
  errno = 0;
  if (mbtowc(&wide, "\xe2", 1) != -1 || errno != EILSEQ) return 26;
  if (mbtowc(0, 0, 0) != 0 || mbtowc(&wide, "Z", 1) != 1 || wide != L'Z')
    return 27;
  if (btowc('Q') != L'Q' || btowc(EOF) != WEOF || btowc(0xe2) != WEOF)
    return 28;

  char encoded[8] = {};
  state = (mbstate_t){};
  if (wcrtomb(encoded, (wchar_t)0x1f600, &state) != 4 ||
      (unsigned char)encoded[0] != 0xf0 ||
      (unsigned char)encoded[3] != 0x80)
    return 29;
  if (wcrtomb(encoded, (wchar_t)0xd800, &state) != 3) return 30;
  errno = 0;
  if (wcrtomb(encoded, (wchar_t)0x200000, &state) != (size_t)-1 ||
      errno != EILSEQ)
    return 31;
  if (wctob(L'R') != 'R' || wctob((wint_t)0x1f600) != EOF) return 32;

  const char input[] = "A\xc2\xa2\xf0\x9f\x98\x80";
  const char* input_cursor = input;
  wchar_t wide_output[8] = {};
  state = (mbstate_t){};
  if (mbsrtowcs(wide_output, &input_cursor, 8, &state) != 3 ||
      input_cursor != 0 || wide_output[0] != L'A' ||
      (uint32_t)wide_output[1] != 0xa2 ||
      (uint32_t)wide_output[2] != 0x1f600)
    return 33;

  const char incomplete[] = "\xe2\x82";
  input_cursor = incomplete;
  state = (mbstate_t){};
  errno = 0;
  if (mbsnrtowcs(wide_output, &input_cursor, 2, 8, &state) != (size_t)-1 ||
      errno != EILSEQ || input_cursor != incomplete + 2)
    return 34;

  const wchar_t wide_input[] = {L'A', (wchar_t)0xa2, (wchar_t)0x1f600, 0};
  const wchar_t* wide_cursor = wide_input;
  char byte_output[16] = {};
  state = (mbstate_t){};
  if (wcsnrtombs(byte_output, &wide_cursor, 4, sizeof(byte_output), &state) !=
          7 ||
      wide_cursor != 0 || byte_output[0] != 'A' ||
      (unsigned char)byte_output[1] != 0xc2 ||
      (unsigned char)byte_output[3] != 0xf0)
    return 35;
  return 42;
}

__attribute__((visibility("default"))) int bionic_locale_fixture_collation(void) {
  locale_t locale = newlocale(LC_ALL_MASK, "C.UTF-8", 0);
  if (locale == 0) return 40;
  if (strcoll_l("abc", "abd", locale) >= 0) return 41;
  char transformed[3] = {'x', 'x', 'x'};
  if (strxfrm_l(transformed, "abcd", sizeof(transformed), locale) != 4 ||
      transformed[0] != 'a' || transformed[1] != 'b' || transformed[2] != 0)
    return 42;
  if (strxfrm(transformed, "abcd", sizeof(transformed)) != 4 ||
      transformed[0] != 'a' || transformed[1] != 'b' || transformed[2] != 0)
    return 47;
  const wchar_t high[] = {(wchar_t)0xffffffffU, 0};
  const wchar_t low[] = {(wchar_t)0x7fffffffU, 0};
  if (wcscoll_l(high, low, locale) <= 0) return 43;
  if (wcscoll(high, low) <= 0) return 45;
  wchar_t wide_transform[2] = {(wchar_t)'x', (wchar_t)'x'};
  const wchar_t wide_source[] = {L'a', L'b', L'c', 0};
  if (wcsxfrm_l(wide_transform, wide_source, 2, locale) != 3 ||
      wide_transform[0] != L'a' || wide_transform[1] != 0)
    return 44;
  if (wcsxfrm(wide_transform, wide_source, 2) != 3 ||
      wide_transform[0] != L'a' || wide_transform[1] != 0)
    return 46;
  freelocale(locale);
  return 42;
}

__attribute__((visibility("default"))) int bionic_locale_fixture_wctype(void) {
  volatile uintptr_t raw_null = 0;
  locale_t null_locale = (locale_t)raw_null;
  locale_t invalid_locale = (locale_t)(uintptr_t)0x12345678U;
  if (!iswalpha_l(L'A', null_locale) ||
      !iswupper_l(L'A', LC_GLOBAL_LOCALE) ||
      iswlower_l(L'A', invalid_locale) || towlower_l(L'A', invalid_locale) != L'a')
    return 60;
  if (!iswalpha_l((wint_t)0x03b1, invalid_locale) ||
      !iswlower_l((wint_t)0x03b1, invalid_locale) ||
      towupper_l((wint_t)0x03b1, invalid_locale) != (wint_t)0x0391)
    return 61;
  if (!iswblank_l(L'\t', null_locale) ||
      !iswcntrl_l(L'\n', null_locale) ||
      !iswdigit_l((wint_t)0x0660, null_locale) ||
      !iswprint_l((wint_t)0x1f600, null_locale) ||
      !iswpunct_l((wint_t)0x2014, null_locale) ||
      !iswspace_l((wint_t)0x2003, null_locale) ||
      !iswxdigit_l((wint_t)0xff21, null_locale))
    return 62;
  if (!iswupper_l((wint_t)0x10400, null_locale) ||
      towlower_l((wint_t)0x10400, null_locale) != (wint_t)0x10428)
    return 63;
  if (!iswalpha(L'A') || !iswblank(L'\t') || !iswcntrl(L'\n') ||
      !iswdigit(L'7') || !iswlower(L'a') || !iswprint(L'A') ||
      !iswpunct(L'!') || !iswupper(L'A') || !iswxdigit(L'f') ||
      towupper(L'a') != L'A')
    return 65;

  const wint_t invalid[] = {WEOF, (wint_t)0xd800, (wint_t)0xfdd0,
                            (wint_t)0x110000, (wint_t)0x80000000U};
  for (size_t index = 0; index < sizeof(invalid) / sizeof(invalid[0]); ++index) {
    const wint_t value = invalid[index];
    if (iswalpha_l(value, invalid_locale) || iswblank_l(value, invalid_locale) ||
        iswcntrl_l(value, invalid_locale) || iswdigit_l(value, invalid_locale) ||
        iswlower_l(value, invalid_locale) || iswprint_l(value, invalid_locale) ||
        iswpunct_l(value, invalid_locale) || iswspace_l(value, invalid_locale) ||
        iswupper_l(value, invalid_locale) || iswxdigit_l(value, invalid_locale) ||
        towlower_l(value, invalid_locale) != value ||
        towupper_l(value, invalid_locale) != value)
      return 64;
  }
  return 42;
}

__attribute__((visibility("default"))) int bionic_locale_fixture_thread(int utf8) {
  locale_t locale = newlocale(LC_ALL_MASK, utf8 ? "C.UTF-8" : "C", 0);
  if (locale == 0 || uselocale(locale) != LC_GLOBAL_LOCALE) return 50;
  for (int iteration = 0; iteration < 10000; ++iteration) {
    if (__ctype_get_mb_cur_max() != (utf8 ? 4U : 1U)) return 51;
  }
  wchar_t output = 0;
  if (mbrtowc(&output, "\xf0", 1, 0) != (size_t)-2 ||
      mbrtowc(&output, "\x9f\x98\x80", 3, 0) != 3 ||
      (uint32_t)output != 0x1f600)
    return 52;
  if (!iswalpha_l((wint_t)0x03b1, locale) ||
      towupper_l((wint_t)0x03b1, locale) != (wint_t)0x0391)
    return 54;
  if (uselocale(LC_GLOBAL_LOCALE) != locale) return 53;
  freelocale(locale);
  return 42;
}
