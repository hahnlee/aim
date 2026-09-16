#include <inttypes.h>
#include <locale.h>
#include <stdlib.h>

_Static_assert(sizeof(long) == 8, "Android arm64 long drift");
_Static_assert(sizeof(long long) == 8, "Android arm64 long long drift");
_Static_assert(sizeof(intmax_t) == 8, "Android arm64 intmax_t drift");
_Static_assert(sizeof(locale_t) == 8, "Android arm64 locale_t drift");

static long (*strtol_signature)(const char*, char**, int) = strtol;
static intmax_t (*strtoimax_signature)(const char*, char**, int) = strtoimax;
static long long (*strtoll_l_signature)(const char*, char**, int, locale_t) =
    strtoll_l;
static unsigned long long (*strtoull_l_signature)(const char*, char**, int,
                                                  locale_t) = strtoull_l;

int main(void) {
  return strtol_signature == 0 || strtoimax_signature == 0 ||
         strtoll_l_signature == 0 ||
         strtoull_l_signature == 0;
}
