#include "aim_bionic_math.h"

#include <cstring>
#include <math.h>

namespace {

void DarwinSincosf(float value, float *sine, float *cosine) {
  __sincosf(value, sine, cosine);
}

void DarwinSincos(double value, double *sine, double *cosine) {
  __sincos(value, sine, cosine);
}

template <typename T>
void *Address(T function) {
  return reinterpret_cast<void *>(function);
}

}  // namespace

extern "C" long double aim_bionic_ldexpl_compat(long double value,
                                                        int exponent) {
  (void)value;
  (void)exponent;
  __asm__ volatile("movi v0.2d, #0" ::: "v0");
  return 0.0L;
}

extern "C" int aim_bionic_feclearexcept(int exceptions) {
  (void)exceptions;
  return 0;
}

extern "C" int aim_bionic_feraiseexcept(int exceptions) {
  (void)exceptions;
  return 0;
}

extern "C" void* aim_bionic_math_resolve(const char* soname,
                                                 const char* symbol,
                                                 const char* version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      (std::strcmp(soname, "libm.so") != 0 &&
       !(std::strcmp(soname, "libc.so") == 0 &&
         (std::strcmp(symbol, "ldexp") == 0 ||
          std::strcmp(symbol, "feclearexcept") == 0 ||
          std::strcmp(symbol, "feraiseexcept") == 0))) ||
      std::strcmp(version, "LIBC") != 0)
    return nullptr;
#define D(name) \
  if (std::strcmp(symbol, #name) == 0) \
    return Address(static_cast<double (*)(double)>(&name))
#define F(name) \
  if (std::strcmp(symbol, #name) == 0) \
    return Address(static_cast<float (*)(float)>(&name))
#define DD(name) \
  if (std::strcmp(symbol, #name) == 0) \
    return Address(static_cast<double (*)(double, double)>(&name))
#define FF(name) \
  if (std::strcmp(symbol, #name) == 0) \
    return Address(static_cast<float (*)(float, float)>(&name))
  if (std::strcmp(symbol, "feclearexcept") == 0)
    return Address(&aim_bionic_feclearexcept);
  if (std::strcmp(symbol, "feraiseexcept") == 0)
    return Address(&aim_bionic_feraiseexcept);
  D(acos); F(acosf); D(acosh); F(acoshf); D(asin); F(asinf); D(asinh); F(asinhf);
  D(atan); DD(atan2); FF(atan2f); F(atanf); D(atanh); F(atanhf);
  D(cbrt); F(cbrtf); F(ceilf); D(cos); F(cosf); D(cosh); F(coshf);
  D(erf); F(erff); F(erfcf);
  D(exp); D(expm1); F(expm1f); D(exp2); F(exp2f); F(expf); D(fabs);
  F(floorf); DD(fmod); FF(fmodf);
  DD(hypot); FF(hypotf);
  if (std::strcmp(symbol, "frexp") == 0)
    return Address(static_cast<double (*)(double, int*)>(&frexp));
  if (std::strcmp(symbol, "frexpf") == 0)
    return Address(static_cast<float (*)(float, int*)>(&frexpf));
  if (std::strcmp(symbol, "ldexp") == 0)
    return Address(static_cast<double (*)(double, int)>(&ldexp));
  if (std::strcmp(symbol, "ldexpf") == 0)
    return Address(static_cast<float (*)(float, int)>(&ldexpf));
  if (std::strcmp(symbol, "ldexpl") == 0)
    return Address(&aim_bionic_ldexpl_compat);
  if (std::strcmp(symbol, "ilogbf") == 0)
    return Address(static_cast<int (*)(float)>(&ilogbf));
  D(log); D(log10); F(log10f); D(log1p); F(log1pf); D(log2); F(log2f); D(logb);
  F(logbf); F(logf);
  if (std::strcmp(symbol, "modf") == 0)
    return Address(static_cast<double (*)(double, double*)>(&modf));
  if (std::strcmp(symbol, "modff") == 0)
    return Address(static_cast<float (*)(float, float*)>(&modff));
  if (std::strcmp(symbol, "nan") == 0)
    return Address(static_cast<double (*)(const char*)>(&nan));
  if (std::strcmp(symbol, "nanf") == 0)
    return Address(static_cast<float (*)(const char*)>(&nanf));
  F(nearbyintf); DD(nextafter); FF(nextafterf);
  DD(pow); FF(powf); DD(remainder); FF(remainderf); D(sqrt); F(sqrtf); D(sin);
  if (std::strcmp(symbol, "scalbnf") == 0)
    return Address(static_cast<float (*)(float, int)>(&scalbnf));
  if (std::strcmp(symbol, "scalbn") == 0)
    return Address(static_cast<double (*)(double, int)>(&scalbn));
  if (std::strcmp(symbol, "sincos") == 0) return Address(&DarwinSincos);
  if (std::strcmp(symbol, "sincosf") == 0) return Address(&DarwinSincosf);
  F(roundf); F(sinf); D(sinh); F(sinhf); D(tan); F(tanf); D(tanh); F(tanhf);
  D(trunc);
  F(truncf);
#undef D
#undef F
#undef DD
#undef FF
  return nullptr;
}
