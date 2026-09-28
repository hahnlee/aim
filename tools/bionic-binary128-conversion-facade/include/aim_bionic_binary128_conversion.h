#ifndef AIM_BIONIC_BINARY128_CONVERSION_H_
#define AIM_BIONIC_BINARY128_CONVERSION_H_

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef uint32_t AimAndroidWchar;

/* These three symbols have the Android arm64 long-double return ABI. They are
 * deliberately not declared as C functions: Darwin long double is binary64.
 * Their addresses may only be called by Android AAPCS64 code. */
extern const unsigned char aim_bionic_strtold[];
extern const unsigned char aim_bionic_strtold_l[];
extern const unsigned char aim_bionic_wcstold[];
extern const unsigned char aim_bionic_powl[];

void* aim_bionic_binary128_conversion_resolve(const char* soname,
                                                      const char* symbol,
                                                      const char* version);
int aim_bionic_binary128_conversion_capability(const char* capability);

#ifdef __cplusplus
}
#endif

#endif
