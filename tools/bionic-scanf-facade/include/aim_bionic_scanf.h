#ifndef AIM_BIONIC_SCANF_H_
#define AIM_BIONIC_SCANF_H_

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*AimBionicScanfFunction)(void);

/* sscanf has Android AAPCS64 variadic entry semantics. */
int aim_bionic_sscanf(const char* input, const char* format, ...);
/* fscanf has Android AAPCS64 variadic entry semantics and accepts only a
 * provider-owned Android FILE token. */
int aim_bionic_fscanf(void* stream, const char* format, ...);
/* android_va_list points to the Android arm64 32-byte va_list object. */
int aim_bionic_vsscanf(const char* input, const char* format,
                              const void* android_va_list);
void* aim_bionic_scanf_resolve(const char* soname, const char* symbol,
                                      const char* version);
const char* aim_bionic_scanf_capability(const char* capability);

#ifdef __cplusplus
}
#endif
#endif
