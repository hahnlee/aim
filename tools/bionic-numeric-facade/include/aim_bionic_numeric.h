#ifndef AIM_BIONIC_NUMERIC_H_
#define AIM_BIONIC_NUMERIC_H_

#include <stdint.h>

typedef void* AimAndroidLocale;
typedef void (*AimBionicNumericFunction)(void);
typedef struct AimBionicDiv {
  int quot;
  int rem;
} AimBionicDiv;
typedef struct AimBionicLongLongDiv {
  long long quot;
  long long rem;
} AimBionicLongLongDiv;

#ifdef __cplusplus
extern "C" {
#endif

long aim_bionic_strtol(const char*, char**, int);
long long aim_bionic_atoll(const char*);
AimBionicDiv aim_bionic_div(int, int);
AimBionicLongLongDiv aim_bionic_lldiv(long long, long long);
long long aim_bionic_strtoll(const char*, char**, int);
intmax_t aim_bionic_strtoimax(const char*, char**, int);
unsigned long aim_bionic_strtoul(const char*, char**, int);
unsigned long long aim_bionic_strtoull(const char*, char**, int);
uint64_t aim_bionic_strtoumax(const char*, char**, int);
long long aim_bionic_strtoll_l(const char*, char**, int,
                                      AimAndroidLocale);
unsigned long long aim_bionic_strtoull_l(const char*, char**, int,
                                                AimAndroidLocale);

AimBionicNumericFunction aim_bionic_numeric_resolve(const char*);

#ifdef __cplusplus
}
#endif
#endif
