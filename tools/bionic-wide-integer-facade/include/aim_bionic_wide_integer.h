#ifndef AIM_BIONIC_WIDE_INTEGER_H_
#define AIM_BIONIC_WIDE_INTEGER_H_

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef uint32_t AimAndroidWchar;

long aim_bionic_wcstol(const AimAndroidWchar* input,
                              AimAndroidWchar** end_pointer,
                              int base);
long long aim_bionic_wcstoll(const AimAndroidWchar* input,
                                    AimAndroidWchar** end_pointer,
                                    int base);
unsigned long aim_bionic_wcstoul(const AimAndroidWchar* input,
                                        AimAndroidWchar** end_pointer,
                                        int base);
unsigned long long aim_bionic_wcstoull(
    const AimAndroidWchar* input,
    AimAndroidWchar** end_pointer,
    int base);

void* aim_bionic_wide_integer_resolve(const char* soname,
                                              const char* symbol,
                                              const char* version);
int aim_bionic_wide_integer_capability(const char* capability);
void aim_bionic_wide_integer_test_prepare_host_state(void);
int aim_bionic_wide_integer_test_host_state_is_preserved(void);

#ifdef __cplusplus
}
#endif

#endif  // AIM_BIONIC_WIDE_INTEGER_H_
