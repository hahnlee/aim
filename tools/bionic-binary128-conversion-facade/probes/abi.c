#include <locale.h>
#include <stdlib.h>
#include <wchar.h>

static long double (*volatile p_strtold)(const char*, char**) = strtold;
static long double (*volatile p_strtold_l)(const char*, char**, locale_t) =
    strtold_l;
static long double (*volatile p_wcstold)(const wchar_t*, wchar_t**) = wcstold;

int main(void) {
  return p_strtold == 0 || p_strtold_l == 0 || p_wcstold == 0;
}
