#ifndef AIM_BIONIC_LOCALE_H_
#define AIM_BIONIC_LOCALE_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void* AimAndroidLocale;

typedef struct AimAndroidMbState {
  uint8_t sequence[4];
  uint8_t reserved[4];
} AimAndroidMbState;

typedef struct AimAndroidLconv {
  char* decimal_point;
  char* thousands_sep;
  char* grouping;
  char* int_curr_symbol;
  char* currency_symbol;
  char* mon_decimal_point;
  char* mon_thousands_sep;
  char* mon_grouping;
  char* positive_sign;
  char* negative_sign;
  char int_frac_digits;
  char frac_digits;
  char p_cs_precedes;
  char p_sep_by_space;
  char n_cs_precedes;
  char n_sep_by_space;
  char p_sign_posn;
  char n_sign_posn;
  char int_p_cs_precedes;
  char int_p_sep_by_space;
  char int_n_cs_precedes;
  char int_n_sep_by_space;
  char int_p_sign_posn;
  char int_n_sign_posn;
} AimAndroidLconv;

size_t aim_bionic___ctype_get_mb_cur_max(void);
extern const char* aim_bionic__ctype_;
uint32_t aim_bionic_btowc(int byte);
void aim_bionic_freelocale(AimAndroidLocale locale);
int aim_bionic_isalnum(int value);
int aim_bionic_isalpha(int value);
int aim_bionic_islower(int value);
int aim_bionic_isupper(int value);
int aim_bionic_isxdigit(int value);
int aim_bionic_iswalpha_l(uint32_t code_point,
                                 AimAndroidLocale locale);
int aim_bionic_iswalpha(uint32_t code_point);
int aim_bionic_iswblank_l(uint32_t code_point,
                                 AimAndroidLocale locale);
int aim_bionic_iswblank(uint32_t code_point);
int aim_bionic_iswcntrl_l(uint32_t code_point,
                                 AimAndroidLocale locale);
int aim_bionic_iswcntrl(uint32_t code_point);
int aim_bionic_iswdigit_l(uint32_t code_point,
                                 AimAndroidLocale locale);
int aim_bionic_iswdigit(uint32_t code_point);
int aim_bionic_iswlower_l(uint32_t code_point,
                                 AimAndroidLocale locale);
int aim_bionic_iswlower(uint32_t code_point);
int aim_bionic_iswprint_l(uint32_t code_point,
                                 AimAndroidLocale locale);
int aim_bionic_iswprint(uint32_t code_point);
int aim_bionic_iswpunct_l(uint32_t code_point,
                                 AimAndroidLocale locale);
int aim_bionic_iswpunct(uint32_t code_point);
int aim_bionic_iswspace_l(uint32_t code_point,
                                 AimAndroidLocale locale);
int aim_bionic_iswspace(uint32_t code_point);
int aim_bionic_iswupper_l(uint32_t code_point,
                                 AimAndroidLocale locale);
int aim_bionic_iswupper(uint32_t code_point);
int aim_bionic_iswxdigit_l(uint32_t code_point,
                                  AimAndroidLocale locale);
int aim_bionic_iswxdigit(uint32_t code_point);
AimAndroidLconv* aim_bionic_localeconv(void);
size_t aim_bionic_mbrlen(const char* source,
                                size_t length,
                                AimAndroidMbState* state);
size_t aim_bionic_mbrtowc(uint32_t* output,
                                const char* source,
                                size_t length,
                                AimAndroidMbState* state);
size_t aim_bionic_mbsnrtowcs(uint32_t* destination,
                                   const char** source,
                                   size_t source_length,
                                   size_t destination_length,
                                   AimAndroidMbState* state);
size_t aim_bionic_mbsrtowcs(uint32_t* destination,
                                  const char** source,
                                  size_t destination_length,
                                  AimAndroidMbState* state);
int aim_bionic_mbtowc(uint32_t* output,
                            const char* source,
                            size_t length);
AimAndroidLocale aim_bionic_newlocale(
    int category_mask,
    const char* locale_name,
    AimAndroidLocale base);
char* aim_bionic_setlocale(int category, const char* locale_name);
int aim_bionic_strcoll_l(const char* left,
                               const char* right,
                               AimAndroidLocale locale);
int aim_bionic_strcoll(const char* left, const char* right);
size_t aim_bionic_strxfrm_l(char* destination,
                                  const char* source,
                                  size_t length,
                                  AimAndroidLocale locale);
size_t aim_bionic_strxfrm(char* destination,
                                const char* source,
                                size_t length);
int aim_bionic_tolower(int value);
int aim_bionic_tolower_l(int value, AimAndroidLocale locale);
int aim_bionic_toupper(int value);
int aim_bionic_toupper_l(int value, AimAndroidLocale locale);
uint32_t aim_bionic_towlower_l(uint32_t code_point,
                                      AimAndroidLocale locale);
uint32_t aim_bionic_towlower(uint32_t code_point);
uint32_t aim_bionic_towupper_l(uint32_t code_point,
                                      AimAndroidLocale locale);
uint32_t aim_bionic_towupper(uint32_t code_point);
AimAndroidLocale aim_bionic_uselocale(
    AimAndroidLocale locale);
size_t aim_bionic_wcrtomb(char* destination,
                                uint32_t code_point,
                                AimAndroidMbState* state);
int aim_bionic_wcscoll_l(const uint32_t* left,
                               const uint32_t* right,
                               AimAndroidLocale locale);
int aim_bionic_wcscoll(const uint32_t* left, const uint32_t* right);
size_t aim_bionic_wcsnrtombs(char* destination,
                                   const uint32_t** source,
                                   size_t source_length,
                                   size_t destination_length,
                                   AimAndroidMbState* state);
size_t aim_bionic_wcsxfrm_l(uint32_t* destination,
                                  const uint32_t* source,
                                  size_t length,
                                  AimAndroidLocale locale);
size_t aim_bionic_wcsxfrm(uint32_t* destination,
                                const uint32_t* source,
                                size_t length);
int aim_bionic_wctob(uint32_t code_point);
int aim_bionic_wcwidth(uint32_t code_point);

void* aim_bionic_locale_resolve(const char* soname,
                                       const char* symbol,
                                       const char* version);
int aim_bionic_locale_capability(const char* capability);
size_t aim_bionic_locale_live_handle_count(void);

/* Hidden standalone-gate helpers; never exposed by the libc resolver. */
void aim_bionic_locale_test_prepare_host_state(void);
int aim_bionic_locale_test_host_state_is_preserved(void);

#ifdef __cplusplus
}
#endif

#endif  // AIM_BIONIC_LOCALE_H_
