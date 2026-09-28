#include "aim_bionic_stdio.h"
#include <errno.h>
#include <pthread.h>
#include <stddef.h>

extern int32_t aim_bionic_errno_load(void);
extern void aim_bionic_errno_store(int32_t);
extern void* aim_bionic_realloc(void*, size_t);
extern int aim_bionic_atoi(const char*);

AimAndroidFile aim_bionic___sF[3];
AimAndroidFile* aim_bionic_stdin = &aim_bionic___sF[0];
AimAndroidFile* aim_bionic_stdout = &aim_bionic___sF[1];
AimAndroidFile* aim_bionic_stderr = &aim_bionic___sF[2];
_Static_assert(sizeof(AimAndroidFile) == 152, "Android FILE size drift");
_Static_assert(_Alignof(AimAndroidFile) == 8, "Android FILE align drift");

#define WRAP(saved, call) do { const int saved = errno; const __typeof__(call) result = (call); errno = saved; return result; } while (0)
AimAndroidFile* aim_bionic_fopen(const char* p, const char* m) { WRAP(e, aim_bionic_stdio_fopen_core(p,m)); }
AimAndroidFile* aim_bionic_fdopen(int fd, const char* mode) {
  if (mode == NULL) {
    aim_bionic_errno_store(22);
    return NULL;
  }
  if (fd >= 0 && fd <= 2) return &aim_bionic___sF[fd];
  aim_bionic_errno_store(38);
  return NULL;
}
int aim_bionic_fclose(AimAndroidFile* f) { WRAP(e, aim_bionic_stdio_fclose_core(f)); }
int aim_bionic_fflush(AimAndroidFile* f) { WRAP(e, aim_bionic_stdio_fflush_core(f)); }
int aim_bionic_fileno(AimAndroidFile* f) { WRAP(e, aim_bionic_stdio_fileno_core(f)); }
static pthread_once_t gStdioLockOnce = PTHREAD_ONCE_INIT;
static pthread_mutex_t gStdioLock;
static void InitializeStdioLock(void) {
  pthread_mutexattr_t attributes;
  (void)pthread_mutexattr_init(&attributes);
  (void)pthread_mutexattr_settype(&attributes, PTHREAD_MUTEX_RECURSIVE);
  (void)pthread_mutex_init(&gStdioLock, &attributes);
  (void)pthread_mutexattr_destroy(&attributes);
}
void aim_bionic_flockfile(AimAndroidFile* f) {
  (void)f;
  (void)pthread_once(&gStdioLockOnce, InitializeStdioLock);
  (void)pthread_mutex_lock(&gStdioLock);
}
int aim_bionic_ftrylockfile(AimAndroidFile* f) {
  (void)f;
  (void)pthread_once(&gStdioLockOnce, InitializeStdioLock);
  return pthread_mutex_trylock(&gStdioLock) == 0 ? 0 : -1;
}
void aim_bionic_funlockfile(AimAndroidFile* f) {
  (void)f;
  (void)pthread_once(&gStdioLockOnce, InitializeStdioLock);
  (void)pthread_mutex_unlock(&gStdioLock);
}
size_t aim_bionic_fread(void* b,size_t s,size_t n,AimAndroidFile* f) { WRAP(e, aim_bionic_stdio_fread_core(b,s,n,f)); }
size_t aim_bionic_fwrite(const void* b,size_t s,size_t n,AimAndroidFile* f) { WRAP(e, aim_bionic_stdio_fwrite_core(b,s,n,f)); }
int aim_bionic_fseek(AimAndroidFile* f,long o,int w) { WRAP(e, aim_bionic_stdio_fseek_core(f,o,w)); }
int aim_bionic_fseeko(AimAndroidFile* f,int64_t o,int w) { WRAP(e, aim_bionic_stdio_fseek_core(f,o,w)); }
int64_t aim_bionic_ftello(AimAndroidFile* f) { WRAP(e, aim_bionic_stdio_ftello_core(f)); }
int aim_bionic_fputc(int c,AimAndroidFile* f) { WRAP(e, aim_bionic_stdio_fputc_core(c,f)); }
int aim_bionic_fputs(const char* s,AimAndroidFile* f) {
  if (s == NULL) return -1;
  size_t n = 0;
  const volatile char* cursor = s;
  while (*cursor++ != '\0') ++n;
  const int saved = errno;
  const size_t written = aim_bionic_stdio_fwrite_core(s,1,n,f);
  errno = saved;
  return written == n ? 0 : -1;
}
int aim_bionic_getc(AimAndroidFile* f) { WRAP(e, aim_bionic_stdio_getc_core(f)); }
int aim_bionic_ungetc(int c,AimAndroidFile* f) { WRAP(e, aim_bionic_stdio_ungetc_core(c,f)); }
int aim_bionic_feof(AimAndroidFile* f) { WRAP(e, aim_bionic_stdio_feof_core(f)); }
int aim_bionic_ferror(AimAndroidFile* f) { WRAP(e, aim_bionic_stdio_ferror_core(f)); }
void aim_bionic_clearerr(AimAndroidFile* f) {
  const int saved = errno;
  aim_bionic_stdio_clearerr_core(f);
  errno = saved;
}

AimAndroidFile* aim_bionic_setmntent(const char* path,
                                                  const char* mode) {
  return aim_bionic_fopen(path, mode);
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

AimAndroidMntent* aim_bionic_getmntent_r(
    AimAndroidFile* stream, AimAndroidMntent* entry, char* buffer,
    int capacity) {
  if (stream == NULL || entry == NULL || buffer == NULL || capacity <= 0)
    return NULL;
  while (aim_bionic_fgets(buffer, capacity, stream) != NULL) {
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
    entry->frequency = frequency == NULL ? 0 : aim_bionic_atoi(frequency);
    entry->pass_number =
        pass_number == NULL ? 0 : aim_bionic_atoi(pass_number);
    return entry;
  }
  return NULL;
}

int aim_bionic_endmntent(AimAndroidFile* stream) {
  return aim_bionic_fclose(stream) == 0 ? 1 : 0;
}

int aim_bionic_getchar(void) {
  return aim_bionic_getc(aim_bionic_stdin);
}

int aim_bionic_fgetc(AimAndroidFile* stream) {
  return aim_bionic_getc(stream);
}

int aim_bionic_getc_unlocked(AimAndroidFile* stream) {
  return aim_bionic_getc(stream);
}

int aim_bionic_putc_unlocked(int value, AimAndroidFile* stream) {
  return aim_bionic_fputc(value, stream);
}

int aim_bionic_setvbuf(AimAndroidFile* stream, char* buffer,
                              int mode, size_t size) {
  (void)stream;
  (void)buffer;
  (void)mode;
  (void)size;
  return 0;
}

AimAndroidFile* aim_bionic_freopen_unsupported(
    const char* path, const char* mode, AimAndroidFile* stream) {
  (void)path;
  (void)mode;
  (void)stream;
  aim_bionic_errno_store(38);
  return NULL;
}

AimAndroidFile* aim_bionic_tmpfile_unsupported(void) {
  aim_bionic_errno_store(38);
  return NULL;
}

AimAndroidFile* aim_bionic_popen_unsupported(const char* command,
                                                          const char* mode) {
  (void)command;
  (void)mode;
  aim_bionic_errno_store(38);
  return NULL;
}

int aim_bionic_pclose_unsupported(AimAndroidFile* stream) {
  (void)stream;
  aim_bionic_errno_store(38);
  return -1;
}
long aim_bionic_ftell(AimAndroidFile* f) { WRAP(e, (long)aim_bionic_stdio_ftello_core(f)); }
char* aim_bionic_fgets(char* s,int n,AimAndroidFile* f) {
  if (s == NULL || n <= 0) return NULL;
  int i = 0;
  while (i + 1 < n) {
    int c = aim_bionic_stdio_getc_core(f);
    if (c == -1) break;
    s[i++] = (char)c;
    if (c == '\n') break;
  }
  if (i == 0) return NULL;
  s[i] = '\0';
  return s;
}
int aim_bionic_puts(const char* s) {
  if (s == NULL) return -1;
  if (aim_bionic_fputs(s,aim_bionic_stdout) < 0)
    return -1;
  return aim_bionic_fputc('\n',aim_bionic_stdout);
}
int aim_bionic_putchar(int c) {
  return aim_bionic_fputc(c, aim_bionic_stdout);
}
void aim_bionic_setbuf(AimAndroidFile* f, char* buffer) {
  /* The compatibility stream owns its backing bytes and makes writes visible
   * immediately. Bionic setbuf(stream, NULL) therefore already has its exact
   * unbuffered effect; a caller buffer must never become host-owned storage. */
  (void)f;
  (void)buffer;
}
void aim_bionic_rewind(AimAndroidFile* f) {
  const int saved = errno;
  (void)aim_bionic_stdio_fseek_core(f, 0, 0);
  errno = saved;
}
int64_t aim_bionic_getline(char** line, size_t* capacity,
                                  AimAndroidFile* f) {
  if (line == NULL || capacity == NULL || f == NULL) {
    aim_bionic_errno_store(22);
    return -1;
  }
  if (*line == NULL || *capacity == 0) {
    char* allocation = (char*)aim_bionic_realloc(*line, 128);
    if (allocation == NULL) {
      aim_bionic_errno_store(12);
      return -1;
    }
    *line = allocation;
    *capacity = 128;
  }
  size_t length = 0;
  for (;;) {
    const int value = aim_bionic_stdio_getc_core(f);
    if (value == -1) {
      if (length == 0) return -1;
      break;
    }
    if (length + 1 >= *capacity) {
      if (*capacity > SIZE_MAX / 2) {
        aim_bionic_errno_store(75);
        return -1;
      }
      const size_t grown = *capacity * 2;
      char* allocation = (char*)aim_bionic_realloc(*line, grown);
      if (allocation == NULL) {
        aim_bionic_errno_store(12);
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
__attribute__((no_stack_protector)) void aim_bionic_perror(const char* prefix) {
  const int saved = errno;
  char message[32];
  FormatErrno(aim_bionic_errno_load(),message);
  if (prefix != NULL && prefix[0] != '\0') {
    (void)aim_bionic_fputs(prefix,aim_bionic_stderr);
    (void)aim_bionic_fputs(": ",aim_bionic_stderr);
  }
  (void)aim_bionic_fputs(message,aim_bionic_stderr);
  (void)aim_bionic_fputc('\n',aim_bionic_stderr);
  errno = saved;
}
int aim_bionic_vprintf(const char* format, const void* ap) { (void)format; (void)ap; return 0; }

static int Compare(const char* a,const char* b) { while(*a==*b&&*a){a++;b++;} return (unsigned char)*a<(unsigned char)*b?-1:((unsigned char)*a!=(unsigned char)*b); }
typedef struct { const char* name; AimBionicStdioFunction address; } Binding;
static const Binding kBindings[] = {
 {"__fread_chk",(AimBionicStdioFunction)aim_bionic___fread_chk},
 {"__fwrite_chk",(AimBionicStdioFunction)aim_bionic___fwrite_chk},
 {"__sF",(AimBionicStdioFunction)aim_bionic___sF},
 {"clearerr",(AimBionicStdioFunction)aim_bionic_clearerr},
 {"endmntent",(AimBionicStdioFunction)aim_bionic_endmntent},
 {"fclose",(AimBionicStdioFunction)aim_bionic_fclose},
 {"fdopen",(AimBionicStdioFunction)aim_bionic_fdopen},
 {"feof",(AimBionicStdioFunction)aim_bionic_feof},
 {"ferror",(AimBionicStdioFunction)aim_bionic_ferror},
 {"fflush",(AimBionicStdioFunction)aim_bionic_fflush},
 {"fgetc",(AimBionicStdioFunction)aim_bionic_fgetc},
 {"fgets",(AimBionicStdioFunction)aim_bionic_fgets},
 {"fileno",(AimBionicStdioFunction)aim_bionic_fileno},
 {"flockfile",(AimBionicStdioFunction)aim_bionic_flockfile},
 {"fopen",(AimBionicStdioFunction)aim_bionic_fopen},
 {"fputc",(AimBionicStdioFunction)aim_bionic_fputc},
 {"fputs",(AimBionicStdioFunction)aim_bionic_fputs},
 {"fread",(AimBionicStdioFunction)aim_bionic_fread},
 {"freopen",(AimBionicStdioFunction)aim_bionic_freopen_unsupported},
 {"fseek",(AimBionicStdioFunction)aim_bionic_fseek},
 {"fseeko",(AimBionicStdioFunction)aim_bionic_fseeko},
 {"ftell",(AimBionicStdioFunction)aim_bionic_ftell},
 {"ftello",(AimBionicStdioFunction)aim_bionic_ftello},
 {"ftrylockfile",(AimBionicStdioFunction)aim_bionic_ftrylockfile},
 {"funlockfile",(AimBionicStdioFunction)aim_bionic_funlockfile},
 {"fwrite",(AimBionicStdioFunction)aim_bionic_fwrite},
 {"getc",(AimBionicStdioFunction)aim_bionic_getc},
 {"getc_unlocked",(AimBionicStdioFunction)aim_bionic_getc_unlocked},
 {"getchar",(AimBionicStdioFunction)aim_bionic_getchar},
 {"getline",(AimBionicStdioFunction)aim_bionic_getline},
 {"getmntent_r",(AimBionicStdioFunction)aim_bionic_getmntent_r},
 {"pclose",(AimBionicStdioFunction)aim_bionic_pclose_unsupported},
 {"perror",(AimBionicStdioFunction)aim_bionic_perror},
 {"popen",(AimBionicStdioFunction)aim_bionic_popen_unsupported},
 {"putc_unlocked",(AimBionicStdioFunction)aim_bionic_putc_unlocked},
 {"putchar",(AimBionicStdioFunction)aim_bionic_putchar},
 {"puts",(AimBionicStdioFunction)aim_bionic_puts},
 {"rewind",(AimBionicStdioFunction)aim_bionic_rewind},
 {"setbuf",(AimBionicStdioFunction)aim_bionic_setbuf},
 {"setmntent",(AimBionicStdioFunction)aim_bionic_setmntent},
 {"setvbuf",(AimBionicStdioFunction)aim_bionic_setvbuf},
 {"stderr",(AimBionicStdioFunction)&aim_bionic_stderr},
 {"stdin",(AimBionicStdioFunction)&aim_bionic_stdin},
 {"stdout",(AimBionicStdioFunction)&aim_bionic_stdout},
 {"tmpfile",(AimBionicStdioFunction)aim_bionic_tmpfile_unsupported},
 {"ungetc",(AimBionicStdioFunction)aim_bionic_ungetc},
 {"vprintf",(AimBionicStdioFunction)aim_bionic_vprintf},
};
AimBionicStdioFunction aim_bionic_stdio_resolve(const char* n) { if(!n)return NULL; size_t l=0,h=sizeof(kBindings)/sizeof(kBindings[0]); while(l<h){size_t m=l+(h-l)/2;int o=Compare(n,kBindings[m].name);if(!o)return kBindings[m].address;if(o<0)h=m;else l=m+1;}return NULL; }
