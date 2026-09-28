#ifndef AIM_BIONIC_STDIO_H_
#define AIM_BIONIC_STDIO_H_
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct __attribute__((aligned(8))) AimAndroidFile {
  unsigned char opaque[152];
} AimAndroidFile;
extern AimAndroidFile aim_bionic___sF[3];
/* Android API 23+ exports these as FILE* object symbols.  The ELF resolver
 * publishes the address of each pointer object, not the FILE token itself. */
extern AimAndroidFile* aim_bionic_stdin;
extern AimAndroidFile* aim_bionic_stdout;
extern AimAndroidFile* aim_bionic_stderr;
typedef void (*AimBionicStdioFunction)(void);
typedef int (*AimBionicStdioScanCallback)(
    const char* input, size_t length, void* context, size_t* consumed);
typedef struct AimAndroidMntent {
  char* fsname;
  char* directory;
  char* type;
  char* options;
  int frequency;
  int pass_number;
} AimAndroidMntent;

/* Process-scoped central owner used by the runtime. Calls are refcounted. */
int aim_bionic_stdio_process_install(void);
int aim_bionic_stdio_process_uninstall(void);

AimAndroidFile* aim_bionic_fopen(const char*, const char*);
AimAndroidFile* aim_bionic_fdopen(int, const char*);
int aim_bionic_fclose(AimAndroidFile*);
int aim_bionic_fflush(AimAndroidFile*);
int aim_bionic_fileno(AimAndroidFile*);
void aim_bionic_flockfile(AimAndroidFile*);
int aim_bionic_ftrylockfile(AimAndroidFile*);
void aim_bionic_funlockfile(AimAndroidFile*);
size_t aim_bionic_fread(void*, size_t, size_t, AimAndroidFile*);
size_t aim_bionic_fwrite(const void*, size_t, size_t, AimAndroidFile*);
size_t aim_bionic___fread_chk(void*, size_t, size_t, AimAndroidFile*, size_t);
size_t aim_bionic___fwrite_chk(const void*, size_t, size_t, AimAndroidFile*, size_t);
int aim_bionic_fseek(AimAndroidFile*, long, int);
int aim_bionic_fseeko(AimAndroidFile*, int64_t, int);
int64_t aim_bionic_ftello(AimAndroidFile*);
int aim_bionic_fputc(int, AimAndroidFile*);
int aim_bionic_fputs(const char*, AimAndroidFile*);
int aim_bionic_getc(AimAndroidFile*);
int aim_bionic_ungetc(int, AimAndroidFile*);
int aim_bionic_feof(AimAndroidFile*);
int aim_bionic_ferror(AimAndroidFile*);
long aim_bionic_ftell(AimAndroidFile*);
char* aim_bionic_fgets(char*, int, AimAndroidFile*);
void aim_bionic_perror(const char*);
int aim_bionic_puts(const char*);
int aim_bionic_putchar(int);
void aim_bionic_setbuf(AimAndroidFile*, char*);
void aim_bionic_rewind(AimAndroidFile*);
int64_t aim_bionic_getline(char** line, size_t* capacity,
                                  AimAndroidFile*);
void aim_bionic_clearerr(AimAndroidFile*);
AimAndroidFile* aim_bionic_setmntent(const char*, const char*);
AimAndroidMntent* aim_bionic_getmntent_r(
    AimAndroidFile*, AimAndroidMntent*, char*, int);
int aim_bionic_endmntent(AimAndroidFile*);
AimBionicStdioFunction aim_bionic_stdio_resolve(const char*);

AimAndroidFile* aim_bionic_stdio_fopen_core(const char*, const char*);
int aim_bionic_stdio_fclose_core(AimAndroidFile*);
int aim_bionic_stdio_fflush_core(AimAndroidFile*);
int aim_bionic_stdio_fileno_core(AimAndroidFile*);
size_t aim_bionic_stdio_fread_core(void*, size_t, size_t, AimAndroidFile*);
size_t aim_bionic_stdio_fwrite_core(const void*, size_t, size_t, AimAndroidFile*);
int aim_bionic_stdio_fseek_core(AimAndroidFile*, int64_t, int);
int64_t aim_bionic_stdio_ftello_core(AimAndroidFile*);
int aim_bionic_stdio_fputc_core(int, AimAndroidFile*);
int aim_bionic_stdio_getc_core(AimAndroidFile*);
int aim_bionic_stdio_ungetc_core(int, AimAndroidFile*);
int aim_bionic_stdio_feof_core(AimAndroidFile*);
int aim_bionic_stdio_ferror_core(AimAndroidFile*);
void aim_bionic_stdio_clearerr_core(AimAndroidFile*);
/* Runs one scanf parser against the unread bytes while retaining the FILE
 * owner's central lease, then advances the stream by exactly `consumed`.
 * The callback input is additionally NUL-terminated for Bionic's byte parser;
 * `length` excludes that terminator. */
int aim_bionic_stdio_scan_core(AimAndroidFile*,
                                      AimBionicStdioScanCallback, void*);

#ifdef __cplusplus
}
#endif

#endif
