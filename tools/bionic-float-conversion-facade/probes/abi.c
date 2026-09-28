#include <stddef.h>
#include <stdlib.h>

_Static_assert(sizeof(float) == 4, "Android arm64 float drift");
_Static_assert(sizeof(double) == 8, "Android arm64 double drift");
_Static_assert(sizeof(long double) == 16, "Android arm64 long double drift");
_Static_assert(_Alignof(long double) == 16, "Android arm64 long double alignment drift");

static double (*strtod_signature)(const char*, char**) = strtod;
static float (*strtof_signature)(const char*, char**) = strtof;
static double (*strtod_l_signature)(const char*, char**, locale_t) = strtod_l;
static float (*strtof_l_signature)(const char*, char**, locale_t) = strtof_l;
static long double (*strtold_signature)(const char*, char**) = strtold;

int main(void) {
  return strtod_signature == 0 || strtof_signature == 0 ||
         strtod_l_signature == 0 || strtof_l_signature == 0 ||
         strtold_signature == 0;
}
