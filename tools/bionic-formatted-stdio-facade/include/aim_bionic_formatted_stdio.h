#ifndef AIM_BIONIC_FORMATTED_STDIO_H_
#define AIM_BIONIC_FORMATTED_STDIO_H_

#ifdef __cplusplus
extern "C" {
#endif

#include "aim_bionic_stdio.h"

typedef void (*AimBionicFormattedStdioFunction)(void);

int aim_bionic_fprintf(AimAndroidFile* file, const char* format, ...);
int aim_bionic_vfprintf(AimAndroidFile* file, const char* format,
                               const void* android_va_list);
int aim_bionic_printf(const char* format, ...);
AimBionicFormattedStdioFunction aim_bionic_formatted_stdio_resolve(
    const char* soname, const char* symbol, const char* version);
const char* aim_bionic_formatted_stdio_capability(const char* name);

#ifdef __cplusplus
}
#endif
#endif
