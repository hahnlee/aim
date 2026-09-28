#include <asm/hwcap.h>
#include <stddef.h>
#include <stdlib.h>
#include <sys/auxv.h>
#include <sys/system_properties.h>

_Static_assert(PROP_VALUE_MAX == 92, "Android property value bound drift");
_Static_assert(AT_PAGESZ == 6 && AT_HWCAP == 16 && AT_SECURE == 23 &&
                   AT_RANDOM == 25 && AT_HWCAP2 == 26,
               "Android auxv identifiers drift");
_Static_assert(HWCAP_FP == 1 && HWCAP_ASIMD == 2,
               "Android arm64 baseline HWCAP drift");
typedef char* (*GetenvSignature)(const char*);
typedef int (*PropertyGetSignature)(const char*, char*);
typedef unsigned long (*GetauxvalSignature)(unsigned long);
_Static_assert(_Generic(&getenv, GetenvSignature: 1, default: 0),
               "getenv signature drift");
_Static_assert(_Generic(&__system_property_get, PropertyGetSignature: 1, default: 0),
               "property signature drift");
_Static_assert(_Generic(&getauxval, GetauxvalSignature: 1, default: 0),
               "getauxval signature drift");
