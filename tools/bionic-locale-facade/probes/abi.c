#include <locale.h>
#include <stddef.h>
#include <wchar.h>
#include <wctype.h>

/* Android exposes the locale-owned byte mappings in libc but does not
 * currently declare them in the NDK locale.h surface. */
extern int imported_tolower_l(int, locale_t) __asm__("tolower_l");
extern int imported_toupper_l(int, locale_t) __asm__("toupper_l");

_Static_assert(sizeof(locale_t) == 8, "Android arm64 locale_t drift");
_Static_assert(sizeof(mbstate_t) == 8, "Android arm64 mbstate_t drift");
_Static_assert(_Alignof(mbstate_t) == 1, "Android arm64 mbstate_t alignment drift");
_Static_assert(sizeof(wchar_t) == 4, "Android arm64 wchar_t drift");
_Static_assert(sizeof(struct lconv) == 96, "Android arm64 lconv drift");
_Static_assert(LC_ALL_MASK == 0x1fbf, "Android locale mask drift");

static size_t (*mbrtowc_signature)(wchar_t*, const char*, size_t, mbstate_t*) =
    mbrtowc;
static locale_t (*newlocale_signature)(int, const char*, locale_t) = newlocale;
static int (*iswalpha_l_signature)(wint_t, locale_t) = iswalpha_l;
static int (*iswblank_l_signature)(wint_t, locale_t) = iswblank_l;
static int (*iswcntrl_l_signature)(wint_t, locale_t) = iswcntrl_l;
static int (*iswdigit_l_signature)(wint_t, locale_t) = iswdigit_l;
static int (*iswlower_l_signature)(wint_t, locale_t) = iswlower_l;
static int (*iswprint_l_signature)(wint_t, locale_t) = iswprint_l;
static int (*iswpunct_l_signature)(wint_t, locale_t) = iswpunct_l;
static int (*iswspace_l_signature)(wint_t, locale_t) = iswspace_l;
static int (*iswupper_l_signature)(wint_t, locale_t) = iswupper_l;
static int (*iswxdigit_l_signature)(wint_t, locale_t) = iswxdigit_l;
static wint_t (*towlower_l_signature)(wint_t, locale_t) = towlower_l;
static wint_t (*towupper_l_signature)(wint_t, locale_t) = towupper_l;
static int (*tolower_l_signature)(int, locale_t) = imported_tolower_l;
static int (*toupper_l_signature)(int, locale_t) = imported_toupper_l;

int main(void) {
  return mbrtowc_signature == 0 || newlocale_signature == 0 ||
         iswalpha_l_signature == 0 || iswblank_l_signature == 0 ||
         iswcntrl_l_signature == 0 || iswdigit_l_signature == 0 ||
         iswlower_l_signature == 0 || iswprint_l_signature == 0 ||
         iswpunct_l_signature == 0 || iswspace_l_signature == 0 ||
         iswupper_l_signature == 0 || iswxdigit_l_signature == 0 ||
         towlower_l_signature == 0 || towupper_l_signature == 0 ||
         tolower_l_signature == 0 || toupper_l_signature == 0;
}
