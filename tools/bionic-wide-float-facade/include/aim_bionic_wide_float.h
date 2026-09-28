#ifndef AIM_BIONIC_WIDE_FLOAT_H_
#define AIM_BIONIC_WIDE_FLOAT_H_

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef uint32_t AimAndroidWchar;

double aim_bionic_wcstod(const AimAndroidWchar* input,
                                AimAndroidWchar** end_pointer);
float aim_bionic_wcstof(const AimAndroidWchar* input,
                               AimAndroidWchar** end_pointer);

void* aim_bionic_wide_float_resolve(const char* soname,
                                            const char* symbol,
                                            const char* version);
int aim_bionic_wide_float_capability(const char* capability);

void aim_bionic_wide_float_test_prepare_host_state(void);
int aim_bionic_wide_float_test_host_state_is_preserved(void);

#ifdef __cplusplus
}
#endif

#endif  // AIM_BIONIC_WIDE_FLOAT_H_
