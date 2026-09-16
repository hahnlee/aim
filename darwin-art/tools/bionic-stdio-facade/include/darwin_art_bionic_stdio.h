#ifndef DARWIN_ART_BIONIC_STDIO_H_
#define DARWIN_ART_BIONIC_STDIO_H_
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct __attribute__((aligned(8))) DarwinArtAndroidFile {
  unsigned char opaque[152];
} DarwinArtAndroidFile;
extern DarwinArtAndroidFile darwin_art_bionic___sF[3];
/* Android API 23+ exports these as FILE* object symbols.  The ELF resolver
 * publishes the address of each pointer object, not the FILE token itself. */
extern DarwinArtAndroidFile* darwin_art_bionic_stdin;
extern DarwinArtAndroidFile* darwin_art_bionic_stdout;
extern DarwinArtAndroidFile* darwin_art_bionic_stderr;
typedef void (*DarwinArtBionicStdioFunction)(void);
typedef int (*DarwinArtBionicStdioScanCallback)(
    const char* input, size_t length, void* context, size_t* consumed);
typedef struct DarwinArtAndroidMntent {
  char* fsname;
  char* directory;
  char* type;
  char* options;
  int frequency;
  int pass_number;
} DarwinArtAndroidMntent;

/* Process-scoped central owner used by the runtime. Calls are refcounted. */
int darwin_art_bionic_stdio_process_install(void);
int darwin_art_bionic_stdio_process_uninstall(void);

DarwinArtAndroidFile* darwin_art_bionic_fopen(const char*, const char*);
DarwinArtAndroidFile* darwin_art_bionic_fdopen(int, const char*);
int darwin_art_bionic_fclose(DarwinArtAndroidFile*);
int darwin_art_bionic_fflush(DarwinArtAndroidFile*);
int darwin_art_bionic_fileno(DarwinArtAndroidFile*);
void darwin_art_bionic_flockfile(DarwinArtAndroidFile*);
int darwin_art_bionic_ftrylockfile(DarwinArtAndroidFile*);
void darwin_art_bionic_funlockfile(DarwinArtAndroidFile*);
size_t darwin_art_bionic_fread(void*, size_t, size_t, DarwinArtAndroidFile*);
size_t darwin_art_bionic_fwrite(const void*, size_t, size_t, DarwinArtAndroidFile*);
size_t darwin_art_bionic___fread_chk(void*, size_t, size_t, DarwinArtAndroidFile*, size_t);
size_t darwin_art_bionic___fwrite_chk(const void*, size_t, size_t, DarwinArtAndroidFile*, size_t);
int darwin_art_bionic_fseek(DarwinArtAndroidFile*, long, int);
int darwin_art_bionic_fseeko(DarwinArtAndroidFile*, int64_t, int);
int64_t darwin_art_bionic_ftello(DarwinArtAndroidFile*);
int darwin_art_bionic_fputc(int, DarwinArtAndroidFile*);
int darwin_art_bionic_fputs(const char*, DarwinArtAndroidFile*);
int darwin_art_bionic_getc(DarwinArtAndroidFile*);
int darwin_art_bionic_ungetc(int, DarwinArtAndroidFile*);
int darwin_art_bionic_feof(DarwinArtAndroidFile*);
int darwin_art_bionic_ferror(DarwinArtAndroidFile*);
long darwin_art_bionic_ftell(DarwinArtAndroidFile*);
char* darwin_art_bionic_fgets(char*, int, DarwinArtAndroidFile*);
void darwin_art_bionic_perror(const char*);
int darwin_art_bionic_puts(const char*);
int darwin_art_bionic_putchar(int);
void darwin_art_bionic_setbuf(DarwinArtAndroidFile*, char*);
void darwin_art_bionic_rewind(DarwinArtAndroidFile*);
int64_t darwin_art_bionic_getline(char** line, size_t* capacity,
                                  DarwinArtAndroidFile*);
void darwin_art_bionic_clearerr(DarwinArtAndroidFile*);
DarwinArtAndroidFile* darwin_art_bionic_setmntent(const char*, const char*);
DarwinArtAndroidMntent* darwin_art_bionic_getmntent_r(
    DarwinArtAndroidFile*, DarwinArtAndroidMntent*, char*, int);
int darwin_art_bionic_endmntent(DarwinArtAndroidFile*);
DarwinArtBionicStdioFunction darwin_art_bionic_stdio_resolve(const char*);

DarwinArtAndroidFile* darwin_art_bionic_stdio_fopen_core(const char*, const char*);
int darwin_art_bionic_stdio_fclose_core(DarwinArtAndroidFile*);
int darwin_art_bionic_stdio_fflush_core(DarwinArtAndroidFile*);
int darwin_art_bionic_stdio_fileno_core(DarwinArtAndroidFile*);
size_t darwin_art_bionic_stdio_fread_core(void*, size_t, size_t, DarwinArtAndroidFile*);
size_t darwin_art_bionic_stdio_fwrite_core(const void*, size_t, size_t, DarwinArtAndroidFile*);
int darwin_art_bionic_stdio_fseek_core(DarwinArtAndroidFile*, int64_t, int);
int64_t darwin_art_bionic_stdio_ftello_core(DarwinArtAndroidFile*);
int darwin_art_bionic_stdio_fputc_core(int, DarwinArtAndroidFile*);
int darwin_art_bionic_stdio_getc_core(DarwinArtAndroidFile*);
int darwin_art_bionic_stdio_ungetc_core(int, DarwinArtAndroidFile*);
int darwin_art_bionic_stdio_feof_core(DarwinArtAndroidFile*);
int darwin_art_bionic_stdio_ferror_core(DarwinArtAndroidFile*);
void darwin_art_bionic_stdio_clearerr_core(DarwinArtAndroidFile*);
/* Runs one scanf parser against the unread bytes while retaining the FILE
 * owner's central lease, then advances the stream by exactly `consumed`.
 * The callback input is additionally NUL-terminated for Bionic's byte parser;
 * `length` excludes that terminator. */
int darwin_art_bionic_stdio_scan_core(DarwinArtAndroidFile*,
                                      DarwinArtBionicStdioScanCallback, void*);

#ifdef __cplusplus
}
#endif

#endif
