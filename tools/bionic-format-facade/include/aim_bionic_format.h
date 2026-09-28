#ifndef AIM_BIONIC_FORMAT_H_
#define AIM_BIONIC_FORMAT_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*AimBionicFormatFunction)(void);

int aim_bionic_snprintf(char* dst, size_t size, const char* format, ...);
int aim_bionic_sprintf(char* dst, const char* format, ...);
int aim_bionic_asprintf(char** output, const char* format, ...);
int aim_bionic_vsnprintf(char* dst, size_t size, const char* format,
                                const void* android_va_list);
int aim_bionic_vasprintf(char** output, const char* format,
                                const void* android_va_list);
AimBionicFormatFunction aim_bionic_format_resolve(const char* symbol);
const char* aim_bionic_format_capability(const char* symbol);

#ifdef __cplusplus
}
#endif
#endif
