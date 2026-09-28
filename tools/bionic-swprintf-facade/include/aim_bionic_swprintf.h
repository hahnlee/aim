#ifndef AIM_BIONIC_SWPRINTF_H_
#define AIM_BIONIC_SWPRINTF_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef uint32_t AimAndroidWchar;
typedef void (*AimBionicSwprintfFunction)(void);

int aim_bionic_swprintf(AimAndroidWchar* output, size_t capacity,
                               const AimAndroidWchar* format, ...);
int aim_bionic_swprintf_captured(
    AimAndroidWchar* output, size_t capacity,
    const AimAndroidWchar* format, const uint8_t* fp_registers,
    uint8_t* stack);
AimBionicSwprintfFunction aim_bionic_swprintf_resolve(
    const char* soname, const char* symbol, const char* version);

#ifdef __cplusplus
}
#endif

#endif
