#ifndef AIM_BIONIC_WIDE_STDIO_H_
#define AIM_BIONIC_WIDE_STDIO_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#define AIM_WIDE_STDIO_NOEXCEPT noexcept
#else
#define AIM_WIDE_STDIO_NOEXCEPT
#endif

#define AIM_BIONIC_WIDE_STDIO_BACKEND_ABI 1u
#define AIM_BIONIC_WEOF UINT32_C(0xffffffff)

typedef struct AimAndroidFile AimAndroidFile;
typedef struct AimBionicWideStdioActivation
    AimBionicWideStdioActivation;
typedef void (*AimBionicWideStdioFunction)(void);

/*
 * The central stdio owner returns a lease that keeps one Android FILE token
 * live and locked through release(). All callbacks other than acquire() take
 * that lease, never a Darwin FILE*. Callback failures must store Android errno.
 * Byte-oriented stdio and this callback must share the orientation field.
 */
typedef struct AimBionicWideStdioBackendV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  void* context;
  int (*acquire)(void* context, AimAndroidFile* file, void** lease);
  void (*release)(void* context, void* lease);
  int (*orient_wide)(void* context, void* lease);
  int (*read_byte)(void* context, void* lease, uint8_t* output);
  int (*write_bytes)(void* context,
                     void* lease,
                     const uint8_t* bytes,
                     size_t length);
  void (*set_error)(void* context, void* lease);
  void (*clear_error_and_eof)(void* context, void* lease);
} AimBionicWideStdioBackendV1;

AimBionicWideStdioActivation* aim_bionic_wide_stdio_install(
    const AimBionicWideStdioBackendV1* backend)
    AIM_WIDE_STDIO_NOEXCEPT;
int aim_bionic_wide_stdio_uninstall(
    AimBionicWideStdioActivation** activation)
    AIM_WIDE_STDIO_NOEXCEPT;

/* Called by the central owner while its exclusive stream lease is held. */
int aim_bionic_wide_stdio_reset(AimAndroidFile* file)
    AIM_WIDE_STDIO_NOEXCEPT;
int aim_bionic_wide_stdio_forget(AimAndroidFile* file)
    AIM_WIDE_STDIO_NOEXCEPT;

uint32_t aim_bionic_fputwc(uint32_t wc, AimAndroidFile* file);
uint32_t aim_bionic_getwc(AimAndroidFile* file);
uint32_t aim_bionic_ungetwc(uint32_t wc, AimAndroidFile* file);

uint32_t aim_bionic_wide_stdio_fputwc_core(
    uint32_t wc,
    AimAndroidFile* file) AIM_WIDE_STDIO_NOEXCEPT;
uint32_t aim_bionic_wide_stdio_getwc_core(AimAndroidFile* file)
    AIM_WIDE_STDIO_NOEXCEPT;
uint32_t aim_bionic_wide_stdio_ungetwc_core(
    uint32_t wc,
    AimAndroidFile* file) AIM_WIDE_STDIO_NOEXCEPT;

AimBionicWideStdioFunction aim_bionic_wide_stdio_resolve(
    const char* soname,
    const char* symbol,
    const char* version) AIM_WIDE_STDIO_NOEXCEPT;

#ifdef __cplusplus
}
#endif

#undef AIM_WIDE_STDIO_NOEXCEPT

#endif
