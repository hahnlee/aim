#ifndef AIM_BIONIC_FLOAT_CONVERSION_H_
#define AIM_BIONIC_FLOAT_CONVERSION_H_

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

double aim_bionic_strtod(const char* input, char** end_pointer);
float aim_bionic_strtof(const char* input, char** end_pointer);
double aim_bionic_strtod_l(const char* input, char** end_pointer,
                                  void* locale);
float aim_bionic_strtof_l(const char* input, char** end_pointer,
                                 void* locale);
double aim_bionic_atof(const char* input);

void* aim_bionic_float_conversion_resolve(const char* soname,
                                                  const char* symbol,
                                                  const char* version);
int aim_bionic_float_conversion_capability(const char* capability);


#ifdef __cplusplus
}
#endif

#endif  // AIM_BIONIC_FLOAT_CONVERSION_H_
