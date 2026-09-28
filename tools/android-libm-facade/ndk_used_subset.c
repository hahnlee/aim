#include <math.h>

__attribute__((visibility("default"))) double math_used_subset(
    double magnitude, double sign, float magnitude_f, float sign_f) {
  return fabs(magnitude) + copysign(1.0, sign) +
         (double)fabsf(magnitude_f) + (double)copysignf(1.0f, sign_f);
}

