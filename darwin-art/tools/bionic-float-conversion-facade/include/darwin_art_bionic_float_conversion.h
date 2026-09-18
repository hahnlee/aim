#ifndef DARWIN_ART_BIONIC_FLOAT_CONVERSION_H_
#define DARWIN_ART_BIONIC_FLOAT_CONVERSION_H_

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

double darwin_art_bionic_strtod(const char* input, char** end_pointer);
float darwin_art_bionic_strtof(const char* input, char** end_pointer);
double darwin_art_bionic_strtod_l(const char* input, char** end_pointer,
                                  void* locale);
float darwin_art_bionic_strtof_l(const char* input, char** end_pointer,
                                 void* locale);
double darwin_art_bionic_atof(const char* input);

void* darwin_art_bionic_float_conversion_resolve(const char* soname,
                                                  const char* symbol,
                                                  const char* version);
int darwin_art_bionic_float_conversion_capability(const char* capability);


#ifdef __cplusplus
}
#endif

#endif  // DARWIN_ART_BIONIC_FLOAT_CONVERSION_H_
