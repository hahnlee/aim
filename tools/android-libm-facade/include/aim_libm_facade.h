#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum AimLibmCapability {
  AIM_LIBM_UNKNOWN = 0,
  AIM_LIBM_BIT_EXACT = 1,
  AIM_LIBM_RESULT_ONLY_FENV_UNPROVEN = 2,
  AIM_LIBM_ERRNO_OR_FENV_SENSITIVE = 3,
  AIM_LIBM_UNSUPPORTED_ABI = 4,
};

double aim_bionic_fabs(double value);
float aim_bionic_fabsf(float value);
double aim_bionic_copysign(double magnitude, double sign);
float aim_bionic_copysignf(float magnitude, float sign);

uintptr_t aim_libm_resolve(const char* symbol, const char* version);
enum AimLibmCapability aim_libm_capability(const char* symbol);

#ifdef __cplusplus
}
#endif

