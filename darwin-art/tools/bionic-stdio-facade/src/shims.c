#include "darwin_art_bionic_stdio.h"
#include <errno.h>
#include <pthread.h>
#include <stddef.h>

extern int32_t darwin_art_bionic_errno_load(void);
extern void darwin_art_bionic_errno_store(int32_t);
extern void* darwin_art_bionic_realloc(void*, size_t);
extern int darwin_art_bionic_atoi(const char*);

DarwinArtAndroidFile darwin_art_bionic___sF[3];
DarwinArtAndroidFile* darwin_art_bionic_stdin = &darwin_art_bionic___sF[0];
DarwinArtAndroidFile* darwin_art_bionic_stdout = &darwin_art_bionic___sF[1];
DarwinArtAndroidFile* darwin_art_bionic_stderr = &darwin_art_bionic___sF[2];
_Static_assert(sizeof(DarwinArtAndroidFile) == 152, "Android FILE size drift");
_Static_assert(_Alignof(DarwinArtAndroidFile) == 8, "Android FILE align drift");

#define WRAP(saved, call) do { const int saved = errno; const __typeof__(call) result = (call); errno = saved; return result; } while (0)
DarwinArtAndroidFile* darwin_art_bionic_fopen(const char* p, const char* m) { WRAP(e, darwin_art_bionic_stdio_fopen_core(p,m)); }
DarwinArtAndroidFile* darwin_art_bionic_fdopen(int fd, const char* mode) {
  if (mode == NULL) {
    darwin_art_bionic_errno_store(22);
    return NULL;
  }
  if (fd >= 0 && fd <= 2) return &darwin_art_bionic___sF[fd];
  darwin_art_bionic_errno_store(38);
  return NULL;
}
int darwin_art_bionic_fclose(DarwinArtAndroidFile* f) { WRAP(e, darwin_art_bionic_stdio_fclose_core(f)); }
int darwin_art_bionic_fflush(DarwinArtAndroidFile* f) { WRAP(e, darwin_art_bionic_stdio_fflush_core(f)); }
int darwin_art_bionic_fileno(DarwinArtAndroidFile* f) { WRAP(e, darwin_art_bionic_stdio_fileno_core(f)); }
static pthread_once_t gStdioLockOnce = PTHREAD_ONCE_INIT;
static pthread_mutex_t gStdioLock;
static void InitializeStdioLock(void) {
  pthread_mutexattr_t attributes;
  (void)pthread_mutexattr_init(&attributes);
  (void)pthread_mutexattr_settype(&attributes, PTHREAD_MUTEX_RECURSIVE);
  (void)pthread_mutex_init(&gStdioLock, &attributes);
  (void)pthread_mutexattr_destroy(&attributes);
}
void darwin_art_bionic_flockfile(DarwinArtAndroidFile* f) {
  (void)f;
  (void)pthread_once(&gStdioLockOnce, InitializeStdioLock);
  (void)pthread_mutex_lock(&gStdioLock);
}
int darwin_art_bionic_ftrylockfile(DarwinArtAndroidFile* f) {
  (void)f;
  (void)pthread_once(&gStdioLockOnce, InitializeStdioLock);
  return pthread_mutex_trylock(&gStdioLock) == 0 ? 0 : -1;
}
void darwin_art_bionic_funlockfile(DarwinArtAndroidFile* f) {
  (void)f;
  (void)pthread_once(&gStdioLockOnce, InitializeStdioLock);
  (void)pthread_mutex_unlock(&gStdioLock);
}
size_t darwin_art_bionic_fread(void* b,size_t s,size_t n,DarwinArtAndroidFile* f) { WRAP(e, darwin_art_bionic_stdio_fread_core(b,s,n,f)); }
size_t darwin_art_bionic_fwrite(const void* b,size_t s,size_t n,DarwinArtAndroidFile* f) { WRAP(e, darwin_art_bionic_stdio_fwrite_core(b,s,n,f)); }
int darwin_art_bionic_fseek(DarwinArtAndroidFile* f,long o,int w) { WRAP(e, darwin_art_bionic_stdio_fseek_core(f,o,w)); }
int darwin_art_bionic_fseeko(DarwinArtAndroidFile* f,int64_t o,int w) { WRAP(e, darwin_art_bionic_stdio_fseek_core(f,o,w)); }
int64_t darwin_art_bionic_ftello(DarwinArtAndroidFile* f) { WRAP(e, darwin_art_bionic_stdio_ftello_core(f)); }
int darwin_art_bionic_fputc(int c,DarwinArtAndroidFile* f) { WRAP(e, darwin_art_bionic_stdio_fputc_core(c,f)); }
int darwin_art_bionic_fputs(const char* s,DarwinArtAndroidFile* f) {
  if (s == NULL) return -1;
  size_t n = 0;
  const volatile char* cursor = s;
  while (*cursor++ != '\0') ++n;
  const int saved = errno;
  const size_t written = darwin_art_bionic_stdio_fwrite_core(s,1,n,f);
  errno = saved;
  return written == n ? 0 : -1;
}
int darwin_art_bionic_getc(DarwinArtAndroidFile* f) { WRAP(e, darwin_art_bionic_stdio_getc_core(f)); }
int darwin_art_bionic_ungetc(int c,DarwinArtAndroidFile* f) { WRAP(e, darwin_art_bionic_stdio_ungetc_core(c,f)); }
int darwin_art_bionic_feof(DarwinArtAndroidFile* f) { WRAP(e, darwin_art_bionic_stdio_feof_core(f)); }
int darwin_art_bionic_ferror(DarwinArtAndroidFile* f) { WRAP(e, darwin_art_bionic_stdio_ferror_core(f)); }
void darwin_art_bionic_clearerr(DarwinArtAndroidFile* f) {
  const int saved = errno;
  darwin_art_bionic_stdio_clearerr_core(f);
  errno = saved;
}

DarwinArtAndroidFile* darwin_art_bionic_setmntent(const char* path,
                                                  const char* mode) {
  return darwin_art_bionic_fopen(path, mode);
}

static char* MountField(char** cursor) {
  while (**cursor == ' ' || **cursor == '\t') ++*cursor;
  if (**cursor == '\0' || **cursor == '\n' || **cursor == '#') return NULL;
  char* field = *cursor;
  while (**cursor != '\0' && **cursor != '\n' && **cursor != ' ' &&
         **cursor != '\t')
    ++*cursor;
  if (**cursor != '\0') *(*cursor)++ = '\0';
  return field;
}

DarwinArtAndroidMntent* darwin_art_bionic_getmntent_r(
    DarwinArtAndroidFile* stream, DarwinArtAndroidMntent* entry, char* buffer,
    int capacity) {
  if (stream == NULL || entry == NULL || buffer == NULL || capacity <= 0)
    return NULL;
  while (darwin_art_bionic_fgets(buffer, capacity, stream) != NULL) {
    char* cursor = buffer;
    char* fsname = MountField(&cursor);
    if (fsname == NULL) continue;
    char* directory = MountField(&cursor);
    char* type = MountField(&cursor);
    char* options = MountField(&cursor);
    char* frequency = MountField(&cursor);
    char* pass_number = MountField(&cursor);
    if (directory == NULL || type == NULL || options == NULL) continue;
    entry->fsname = fsname;
    entry->directory = directory;
    entry->type = type;
    entry->options = options;
    entry->frequency = frequency == NULL ? 0 : darwin_art_bionic_atoi(frequency);
    entry->pass_number =
        pass_number == NULL ? 0 : darwin_art_bionic_atoi(pass_number);
    return entry;
  }
  return NULL;
}

int darwin_art_bionic_endmntent(DarwinArtAndroidFile* stream) {
  return darwin_art_bionic_fclose(stream) == 0 ? 1 : 0;
}

int darwin_art_bionic_getchar(void) {
  return darwin_art_bionic_getc(darwin_art_bionic_stdin);
}

int darwin_art_bionic_fgetc(DarwinArtAndroidFile* stream) {
  return darwin_art_bionic_getc(stream);
}

int darwin_art_bionic_getc_unlocked(DarwinArtAndroidFile* stream) {
  return darwin_art_bionic_getc(stream);
}

int darwin_art_bionic_putc_unlocked(int value, DarwinArtAndroidFile* stream) {
  return darwin_art_bionic_fputc(value, stream);
}

int darwin_art_bionic_setvbuf(DarwinArtAndroidFile* stream, char* buffer,
                              int mode, size_t size) {
  (void)stream;
  (void)buffer;
  (void)mode;
  (void)size;
  return 0;
}

DarwinArtAndroidFile* darwin_art_bionic_freopen_unsupported(
    const char* path, const char* mode, DarwinArtAndroidFile* stream) {
  (void)path;
  (void)mode;
  (void)stream;
  darwin_art_bionic_errno_store(38);
  return NULL;
}

DarwinArtAndroidFile* darwin_art_bionic_tmpfile_unsupported(void) {
  darwin_art_bionic_errno_store(38);
  return NULL;
}

DarwinArtAndroidFile* darwin_art_bionic_popen_unsupported(const char* command,
                                                          const char* mode) {
  (void)command;
  (void)mode;
  darwin_art_bionic_errno_store(38);
  return NULL;
}

int darwin_art_bionic_pclose_unsupported(DarwinArtAndroidFile* stream) {
  (void)stream;
  darwin_art_bionic_errno_store(38);
  return -1;
}
long darwin_art_bionic_ftell(DarwinArtAndroidFile* f) { WRAP(e, (long)darwin_art_bionic_stdio_ftello_core(f)); }
char* darwin_art_bionic_fgets(char* s,int n,DarwinArtAndroidFile* f) {
  if (s == NULL || n <= 0) return NULL;
  int i = 0;
  while (i + 1 < n) {
    int c = darwin_art_bionic_stdio_getc_core(f);
    if (c == -1) break;
    s[i++] = (char)c;
    if (c == '\n') break;
  }
  if (i == 0) return NULL;
  s[i] = '\0';
  return s;
}
int darwin_art_bionic_puts(const char* s) {
  if (s == NULL) return -1;
  if (darwin_art_bionic_fputs(s,darwin_art_bionic_stdout) < 0)
    return -1;
  return darwin_art_bionic_fputc('\n',darwin_art_bionic_stdout);
}
int darwin_art_bionic_putchar(int c) {
  return darwin_art_bionic_fputc(c, darwin_art_bionic_stdout);
}
void darwin_art_bionic_setbuf(DarwinArtAndroidFile* f, char* buffer) {
  /* The compatibility stream owns its backing bytes and makes writes visible
   * immediately. Bionic setbuf(stream, NULL) therefore already has its exact
   * unbuffered effect; a caller buffer must never become host-owned storage. */
  (void)f;
  (void)buffer;
}
void darwin_art_bionic_rewind(DarwinArtAndroidFile* f) {
  const int saved = errno;
  (void)darwin_art_bionic_stdio_fseek_core(f, 0, 0);
  errno = saved;
}
int64_t darwin_art_bionic_getline(char** line, size_t* capacity,
                                  DarwinArtAndroidFile* f) {
  if (line == NULL || capacity == NULL || f == NULL) {
    darwin_art_bionic_errno_store(22);
    return -1;
  }
  if (*line == NULL || *capacity == 0) {
    char* allocation = (char*)darwin_art_bionic_realloc(*line, 128);
    if (allocation == NULL) {
      darwin_art_bionic_errno_store(12);
      return -1;
    }
    *line = allocation;
    *capacity = 128;
  }
  size_t length = 0;
  for (;;) {
    const int value = darwin_art_bionic_stdio_getc_core(f);
    if (value == -1) {
      if (length == 0) return -1;
      break;
    }
    if (length + 1 >= *capacity) {
      if (*capacity > SIZE_MAX / 2) {
        darwin_art_bionic_errno_store(75);
        return -1;
      }
      const size_t grown = *capacity * 2;
      char* allocation = (char*)darwin_art_bionic_realloc(*line, grown);
      if (allocation == NULL) {
        darwin_art_bionic_errno_store(12);
        return -1;
      }
      *line = allocation;
      *capacity = grown;
    }
    (*line)[length++] = (char)value;
    if (value == '\n') break;
  }
  (*line)[length] = '\0';
  return (int64_t)length;
}
__attribute__((no_stack_protector)) static void FormatErrno(int32_t number,char output[32]) {
  static const char kPrefix[] = "Error ";
  size_t length = sizeof(kPrefix) - 1;
  for (size_t index = 0; index < length; ++index) output[index] = kPrefix[index];
  uint32_t magnitude;
  if (number < 0) {
    output[length++] = '-';
    magnitude = (uint32_t)(-(int64_t)number);
  } else {
    magnitude = (uint32_t)number;
  }
  char reverse[10];
  size_t digits = 0;
  do {
    reverse[digits++] = (char)('0' + magnitude % 10);
    magnitude /= 10;
  } while (magnitude != 0);
  while (digits != 0) output[length++] = reverse[--digits];
  output[length] = '\0';
}
__attribute__((no_stack_protector)) void darwin_art_bionic_perror(const char* prefix) {
  const int saved = errno;
  char message[32];
  FormatErrno(darwin_art_bionic_errno_load(),message);
  if (prefix != NULL && prefix[0] != '\0') {
    (void)darwin_art_bionic_fputs(prefix,darwin_art_bionic_stderr);
    (void)darwin_art_bionic_fputs(": ",darwin_art_bionic_stderr);
  }
  (void)darwin_art_bionic_fputs(message,darwin_art_bionic_stderr);
  (void)darwin_art_bionic_fputc('\n',darwin_art_bionic_stderr);
  errno = saved;
}
int darwin_art_bionic_vprintf(const char* format, const void* ap) { (void)format; (void)ap; return 0; }

static int Compare(const char* a,const char* b) { while(*a==*b&&*a){a++;b++;} return (unsigned char)*a<(unsigned char)*b?-1:((unsigned char)*a!=(unsigned char)*b); }
typedef struct { const char* name; DarwinArtBionicStdioFunction address; } Binding;
static const Binding kBindings[] = {
 {"__fread_chk",(DarwinArtBionicStdioFunction)darwin_art_bionic___fread_chk},
 {"__fwrite_chk",(DarwinArtBionicStdioFunction)darwin_art_bionic___fwrite_chk},
 {"__sF",(DarwinArtBionicStdioFunction)darwin_art_bionic___sF},
 {"clearerr",(DarwinArtBionicStdioFunction)darwin_art_bionic_clearerr},
 {"endmntent",(DarwinArtBionicStdioFunction)darwin_art_bionic_endmntent},
 {"fclose",(DarwinArtBionicStdioFunction)darwin_art_bionic_fclose},
 {"fdopen",(DarwinArtBionicStdioFunction)darwin_art_bionic_fdopen},
 {"feof",(DarwinArtBionicStdioFunction)darwin_art_bionic_feof},
 {"ferror",(DarwinArtBionicStdioFunction)darwin_art_bionic_ferror},
 {"fflush",(DarwinArtBionicStdioFunction)darwin_art_bionic_fflush},
 {"fgetc",(DarwinArtBionicStdioFunction)darwin_art_bionic_fgetc},
 {"fgets",(DarwinArtBionicStdioFunction)darwin_art_bionic_fgets},
 {"fileno",(DarwinArtBionicStdioFunction)darwin_art_bionic_fileno},
 {"flockfile",(DarwinArtBionicStdioFunction)darwin_art_bionic_flockfile},
 {"fopen",(DarwinArtBionicStdioFunction)darwin_art_bionic_fopen},
 {"fputc",(DarwinArtBionicStdioFunction)darwin_art_bionic_fputc},
 {"fputs",(DarwinArtBionicStdioFunction)darwin_art_bionic_fputs},
 {"fread",(DarwinArtBionicStdioFunction)darwin_art_bionic_fread},
 {"freopen",(DarwinArtBionicStdioFunction)darwin_art_bionic_freopen_unsupported},
 {"fseek",(DarwinArtBionicStdioFunction)darwin_art_bionic_fseek},
 {"fseeko",(DarwinArtBionicStdioFunction)darwin_art_bionic_fseeko},
 {"ftell",(DarwinArtBionicStdioFunction)darwin_art_bionic_ftell},
 {"ftello",(DarwinArtBionicStdioFunction)darwin_art_bionic_ftello},
 {"ftrylockfile",(DarwinArtBionicStdioFunction)darwin_art_bionic_ftrylockfile},
 {"funlockfile",(DarwinArtBionicStdioFunction)darwin_art_bionic_funlockfile},
 {"fwrite",(DarwinArtBionicStdioFunction)darwin_art_bionic_fwrite},
 {"getc",(DarwinArtBionicStdioFunction)darwin_art_bionic_getc},
 {"getc_unlocked",(DarwinArtBionicStdioFunction)darwin_art_bionic_getc_unlocked},
 {"getchar",(DarwinArtBionicStdioFunction)darwin_art_bionic_getchar},
 {"getline",(DarwinArtBionicStdioFunction)darwin_art_bionic_getline},
 {"getmntent_r",(DarwinArtBionicStdioFunction)darwin_art_bionic_getmntent_r},
 {"pclose",(DarwinArtBionicStdioFunction)darwin_art_bionic_pclose_unsupported},
 {"perror",(DarwinArtBionicStdioFunction)darwin_art_bionic_perror},
 {"popen",(DarwinArtBionicStdioFunction)darwin_art_bionic_popen_unsupported},
 {"putc_unlocked",(DarwinArtBionicStdioFunction)darwin_art_bionic_putc_unlocked},
 {"putchar",(DarwinArtBionicStdioFunction)darwin_art_bionic_putchar},
 {"puts",(DarwinArtBionicStdioFunction)darwin_art_bionic_puts},
 {"rewind",(DarwinArtBionicStdioFunction)darwin_art_bionic_rewind},
 {"setbuf",(DarwinArtBionicStdioFunction)darwin_art_bionic_setbuf},
 {"setmntent",(DarwinArtBionicStdioFunction)darwin_art_bionic_setmntent},
 {"setvbuf",(DarwinArtBionicStdioFunction)darwin_art_bionic_setvbuf},
 {"stderr",(DarwinArtBionicStdioFunction)&darwin_art_bionic_stderr},
 {"stdin",(DarwinArtBionicStdioFunction)&darwin_art_bionic_stdin},
 {"stdout",(DarwinArtBionicStdioFunction)&darwin_art_bionic_stdout},
 {"tmpfile",(DarwinArtBionicStdioFunction)darwin_art_bionic_tmpfile_unsupported},
 {"ungetc",(DarwinArtBionicStdioFunction)darwin_art_bionic_ungetc},
 {"vprintf",(DarwinArtBionicStdioFunction)darwin_art_bionic_vprintf},
};
DarwinArtBionicStdioFunction darwin_art_bionic_stdio_resolve(const char* n) { if(!n)return NULL; size_t l=0,h=sizeof(kBindings)/sizeof(kBindings[0]); while(l<h){size_t m=l+(h-l)/2;int o=Compare(n,kBindings[m].name);if(!o)return kBindings[m].address;if(o<0)h=m;else l=m+1;}return NULL; }
