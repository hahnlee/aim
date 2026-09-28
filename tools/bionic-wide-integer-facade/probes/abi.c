#include <stddef.h>
#include <stdint.h>
#include <wchar.h>

_Static_assert(sizeof(wchar_t) == 4, "Android arm64 wchar_t width drift");
_Static_assert((wchar_t)-1 > 0, "Android wchar_t must be unsigned");
_Static_assert(sizeof(long) == 8, "Android arm64 long width drift");

static long (*wcstol_signature)(const wchar_t*, wchar_t**, int) = wcstol;
static long long (*wcstoll_signature)(const wchar_t*, wchar_t**, int) = wcstoll;
static unsigned long (*wcstoul_signature)(const wchar_t*, wchar_t**, int) =
    wcstoul;
static unsigned long long (*wcstoull_signature)(const wchar_t*, wchar_t**,
                                                int) = wcstoull;

int main(void) {
  return wcstol_signature == 0 || wcstoll_signature == 0 ||
         wcstoul_signature == 0 || wcstoull_signature == 0;
}
