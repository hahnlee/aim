#include <float.h>
#include <stdint.h>
#include <wchar.h>

_Static_assert(sizeof(wchar_t) == 4, "Android wchar_t must be 32-bit");
_Static_assert((wchar_t)-1 > 0, "Android wchar_t must be unsigned");
_Static_assert(sizeof(double) == 8 && DBL_MANT_DIG == 53,
               "Android double ABI drift");
_Static_assert(sizeof(float) == 4 && FLT_MANT_DIG == 24,
               "Android float ABI drift");
_Static_assert(sizeof(long double) == 16 && LDBL_MANT_DIG == 113,
               "Android long double must remain IEEE binary128");

static double (*volatile wcstod_signature)(const wchar_t*, wchar_t**) = wcstod;
static float (*volatile wcstof_signature)(const wchar_t*, wchar_t**) = wcstof;
static long double (*volatile wcstold_signature)(const wchar_t*, wchar_t**) =
    wcstold;

int main(void) {
  return wcstod_signature == 0 || wcstof_signature == 0 ||
         wcstold_signature == 0;
}
