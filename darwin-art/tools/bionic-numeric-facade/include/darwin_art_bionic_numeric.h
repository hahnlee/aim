#ifndef DARWIN_ART_BIONIC_NUMERIC_H_
#define DARWIN_ART_BIONIC_NUMERIC_H_

#include <stdint.h>

typedef void* DarwinArtAndroidLocale;
typedef void (*DarwinArtBionicNumericFunction)(void);
typedef struct DarwinArtBionicDiv {
  int quot;
  int rem;
} DarwinArtBionicDiv;
typedef struct DarwinArtBionicLongLongDiv {
  long long quot;
  long long rem;
} DarwinArtBionicLongLongDiv;

#ifdef __cplusplus
extern "C" {
#endif

long darwin_art_bionic_strtol(const char*, char**, int);
long long darwin_art_bionic_atoll(const char*);
DarwinArtBionicDiv darwin_art_bionic_div(int, int);
DarwinArtBionicLongLongDiv darwin_art_bionic_lldiv(long long, long long);
long long darwin_art_bionic_strtoll(const char*, char**, int);
intmax_t darwin_art_bionic_strtoimax(const char*, char**, int);
unsigned long darwin_art_bionic_strtoul(const char*, char**, int);
unsigned long long darwin_art_bionic_strtoull(const char*, char**, int);
uint64_t darwin_art_bionic_strtoumax(const char*, char**, int);
long long darwin_art_bionic_strtoll_l(const char*, char**, int,
                                      DarwinArtAndroidLocale);
unsigned long long darwin_art_bionic_strtoull_l(const char*, char**, int,
                                                DarwinArtAndroidLocale);

DarwinArtBionicNumericFunction darwin_art_bionic_numeric_resolve(const char*);

#ifdef __cplusplus
}
#endif
#endif
