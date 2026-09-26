#ifndef DARWIN_ART_LIBCORE_DARWIN_LINUX_H_
#define DARWIN_ART_LIBCORE_DARWIN_LINUX_H_

#include <jni.h>
#include <sys/stat.h>
#include <sys/types.h>

#include <cstddef>
#include <cstdint>

typedef struct DarwinArtAndroidStat DarwinArtAndroidStat;

namespace darwin_art::libcore_darwin {

// These functions are the syscall boundary shared by JNI and the standalone
// acceptance probe. Inputs use Android/Linux flag values where documented.
int Open(const char *path, int linux_flags, mode_t mode);
int Dup(int fd);
int Fcntl(int fd, int android_command, intptr_t argument);
int Close(int fd);
ssize_t Read(int fd, void *bytes, size_t byte_count);
ssize_t Write(int fd, const void *bytes, size_t byte_count);
ssize_t Pread(int fd, void *bytes, size_t byte_count, int64_t offset);
ssize_t Pwrite(int fd, const void *bytes, size_t byte_count, int64_t offset);
int Fstat(int fd, struct stat *status);
int Ftruncate(int fd, int64_t length);
int Stat(const char *path, struct stat *status);
int Access(const char *path, int mode);
int Remove(const char *path);
int Rename(const char *old_path, const char *new_path);
int Lstat(const char *path, struct stat *status);
int Unlink(const char *path);
ssize_t Readlink(const char *path, char *buffer, size_t size);
int Symlink(const char *target, const char *link_path);
int Link(const char *old_path, const char *new_path);
int Fsync(int fd);
// Returns 0 or an Android errno value.
int PosixFallocate(int fd, int64_t offset, int64_t length);
bool IsAuthorizedHostRuntimePath(const char *path);
int64_t Lseek(int fd, int64_t offset, int android_whence);
intptr_t Sendfile(int output_fd, int input_fd, int64_t *offset,
                  size_t byte_count);
void *Mmap(void *address, size_t byte_count, int linux_prot, int linux_flags,
           int fd, off_t offset);
int Munmap(void *address, size_t byte_count);
long Sysconf(int name);

struct LinuxSyscallProviders {
  int (*open)(const char *, int, uint32_t);
  int (*dup)(int);
  int (*fcntl)(int, int, intptr_t);
  int (*close)(int);
  intptr_t (*read)(int, void *, size_t);
  intptr_t (*write)(int, const void *, size_t);
  intptr_t (*pread)(int, void *, size_t, int64_t);
  intptr_t (*pwrite)(int, const void *, size_t, int64_t);
  int (*fstat)(int, DarwinArtAndroidStat *);
  int (*ftruncate)(int, int64_t);
  int (*stat)(const char *, DarwinArtAndroidStat *);
  int64_t (*lseek)(int, int64_t, int);
  intptr_t (*sendfile)(int, int, int64_t *, size_t);
  int (*access)(const char *, int);
  int (*remove)(const char *);
  int (*rename)(const char *, const char *);
  int (*adopt_host_fd)(int);
  int32_t (*load_errno)();
  void *(*mmap)(void *, size_t, int, int, int, int64_t);
  int (*munmap)(void *, size_t);
};

inline constexpr uint32_t kLinuxSyscallProviderAbiVersion = 4;
void InstallLinuxSyscallProviders(uint32_t abi_version, size_t provider_size,
                                  const LinuxSyscallProviders &providers);

// System/metadata JNI phase. Keeping these definitions out of the generated
// registrar TU lets edits to environment, identity, and diagnostic policy
// rebuild one small object instead of the complete 135-entry table.
void ThrowErrno(JNIEnv *env, const char *operation, int error);
jobject DarwinLinuxDup2(JNIEnv *env, jobject receiver, jobject java_fd,
                        jint new_fd);
void DarwinLinuxSocketpair(JNIEnv *env, jobject receiver, jint domain,
                           jint type, jint protocol, jobject java_fd1,
                           jobject java_fd2);
jobjectArray DarwinLinuxPipe2(JNIEnv *env, jobject receiver, jint flags);
jlong DarwinLinuxSysconf(JNIEnv *env, jobject receiver, jint name);
jstring DarwinLinuxGetenv(JNIEnv *env, jobject receiver, jstring java_name);
void DarwinLinuxSetenv(JNIEnv *env, jobject receiver, jstring java_name,
                       jstring java_value, jboolean overwrite);
void DarwinLinuxUnsetenv(JNIEnv *env, jobject receiver, jstring java_name);
jobject DarwinLinuxGetpwuid(JNIEnv *env, jobject receiver, jint uid);
jobject DarwinLinuxUname(JNIEnv *env, jobject receiver);
jstring DarwinLinuxStrerror(JNIEnv *env, jobject receiver, jint error_number);
jstring DarwinLinuxStrsignal(JNIEnv *env, jobject receiver, jint signal_number);
void DarwinLinuxKill(JNIEnv *env, jobject receiver, jint pid,
                     jint signal_number);

// Owns the complete Android 16 libcore.io.Linux 135-entry registration table.
// No other registrar may register a subset of this class after this succeeds.
bool RegisterLinuxNatives(JNIEnv *env);

} // namespace darwin_art::libcore_darwin

#endif // DARWIN_ART_LIBCORE_DARWIN_LINUX_H_
