#include "libcore_darwin_linux.h"
#include "darwin_dns_hints.h"

#include "AsynchronousCloseMonitor.h"
#include "darwin_art_bionic_process_state.h"
#include "darwin_art_bionic_socket_broker.h"
#include "darwin_os_constants.h"

#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <pthread.h>
#include <sys/mman.h>
#include <sys/statvfs.h>
#include <sys/time.h>
#include <unistd.h>

#include <arpa/inet.h>
#include <atomic>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <string>
#include <string_view>
#include <vector>

#include "process/guest_environment.h"

#include <nativehelper/JNIHelp.h>
#include <nativehelper/JNIPlatformHelp.h>
#include <nativehelper/ScopedUtfChars.h>

extern "C" int darwin_art_bionic_fs_chmod_core(const char *, uint32_t)
    __attribute__((weak_import));
extern "C" int darwin_art_bionic_fs_fchmod_core(int, uint32_t)
    __attribute__((weak_import));
extern "C" int darwin_art_bionic_fs_mkdir_core(const char *, uint32_t)
    __attribute__((weak_import));
extern "C" ssize_t darwin_art_bionic_fs_getxattr_core(const char *, const char *,
                                                     void *, size_t, int)
    __attribute__((weak_import));
extern "C" int darwin_art_bionic_fs_setxattr_core(const char *, const char *,
                                                 const void *, size_t, int, int)
    __attribute__((weak_import));
extern "C" int darwin_art_bionic_fs_removexattr_core(const char *, const char *, int)
    __attribute__((weak_import));
extern "C" ssize_t darwin_art_bionic_fs_listxattr_core(const char *, char *, size_t, int)
    __attribute__((weak_import));
struct DarwinArtAndroidStatvfs {
  uint64_t f_bsize;
  uint64_t f_frsize;
  uint64_t f_blocks;
  uint64_t f_bfree;
  uint64_t f_bavail;
  uint64_t f_files;
  uint64_t f_ffree;
  uint64_t f_favail;
  uint64_t f_fsid;
  uint64_t f_flag;
  uint64_t f_namemax;
  uint64_t reserved[6];
};
extern "C" int darwin_art_bionic_fs_statvfs_core(const char *,
                                                 DarwinArtAndroidStatvfs *)
    __attribute__((weak_import));
extern "C" int32_t darwin_art_bionic_errno_load(void)
    __attribute__((weak_import));

namespace darwin_art::libcore_darwin {

namespace {

constexpr uint32_t kAndroidGetaddrinfoDebugLogLimit = 256;
std::atomic<uint32_t> g_android_getaddrinfo_debug_logs{0};

bool ShouldLogAndroidGetaddrinfo() {
  if (std::getenv("DARWIN_ART_DEBUG_DNS") == nullptr)
    return false;
  const uint32_t prior =
      g_android_getaddrinfo_debug_logs.fetch_add(1, std::memory_order_relaxed);
  return prior < kAndroidGetaddrinfoDebugLogLimit;
}

} // namespace

void ThrowErrno(JNIEnv *env, const char *operation, int error) {
  int android_error = error;
  os_constants::AndroidErrnoFromDarwin(error, &android_error);
  jniThrowErrnoException(env, operation, android_error);
}

namespace {

jobject MakeTimespec(JNIEnv *env, const struct timespec &value) {
  jclass klass = env->FindClass("android/system/StructTimespec");
  if (klass == nullptr) {
    return nullptr;
  }
  jmethodID constructor = env->GetMethodID(klass, "<init>", "(JJ)V");
  jobject result =
      constructor == nullptr
          ? nullptr
          : env->NewObject(klass, constructor, static_cast<jlong>(value.tv_sec),
                           static_cast<jlong>(value.tv_nsec));
  env->DeleteLocalRef(klass);
  return result;
}

jobject MakeStructStat(JNIEnv *env, const struct stat &status) {
  jobject access_time = MakeTimespec(env, status.st_atimespec);
  jobject modification_time = MakeTimespec(env, status.st_mtimespec);
  jobject change_time = MakeTimespec(env, status.st_ctimespec);
  if (access_time == nullptr || modification_time == nullptr ||
      change_time == nullptr) {
    env->DeleteLocalRef(access_time);
    env->DeleteLocalRef(modification_time);
    env->DeleteLocalRef(change_time);
    return nullptr;
  }
  jclass klass = env->FindClass("android/system/StructStat");
  jmethodID constructor =
      klass == nullptr
          ? nullptr
          : env->GetMethodID(klass, "<init>",
                             "(JJIJIIJJLandroid/system/StructTimespec;"
                             "Landroid/system/StructTimespec;Landroid/system/"
                             "StructTimespec;JJ)V");
  jobject result =
      constructor == nullptr
          ? nullptr
          : env->NewObject(klass, constructor,
                           static_cast<jlong>(status.st_dev),
                           static_cast<jlong>(status.st_ino),
                           static_cast<jint>(status.st_mode),
                           static_cast<jlong>(status.st_nlink),
                           static_cast<jint>(status.st_uid),
                           static_cast<jint>(status.st_gid),
                           static_cast<jlong>(status.st_rdev),
                           static_cast<jlong>(status.st_size), access_time,
                           modification_time, change_time,
                           static_cast<jlong>(status.st_blksize),
                           static_cast<jlong>(status.st_blocks));
  env->DeleteLocalRef(klass);
  env->DeleteLocalRef(access_time);
  env->DeleteLocalRef(modification_time);
  env->DeleteLocalRef(change_time);
  return result;
}

jobject MakeStructStatVfs(JNIEnv *env, const struct statvfs &status) {
  jclass klass = env->FindClass("android/system/StructStatVfs");
  jmethodID constructor =
      klass == nullptr ? nullptr
                       : env->GetMethodID(klass, "<init>", "(JJJJJJJJJJJ)V");
  jobject result = constructor == nullptr
                       ? nullptr
                       : env->NewObject(klass, constructor,
                                        static_cast<jlong>(status.f_bsize),
                                        static_cast<jlong>(status.f_frsize),
                                        static_cast<jlong>(status.f_blocks),
                                        static_cast<jlong>(status.f_bfree),
                                        static_cast<jlong>(status.f_bavail),
                                        static_cast<jlong>(status.f_files),
                                        static_cast<jlong>(status.f_ffree),
                                        static_cast<jlong>(status.f_favail),
                                        static_cast<jlong>(status.f_fsid),
                                        static_cast<jlong>(status.f_flag),
                                        static_cast<jlong>(status.f_namemax));
  env->DeleteLocalRef(klass);
  return result;
}

jobject MakeStructStatVfs(JNIEnv *env, const DarwinArtAndroidStatvfs &status) {
  jclass klass = env->FindClass("android/system/StructStatVfs");
  jmethodID constructor =
      klass == nullptr ? nullptr
                       : env->GetMethodID(klass, "<init>", "(JJJJJJJJJJJ)V");
  jobject result = constructor == nullptr
                       ? nullptr
                       : env->NewObject(klass, constructor,
                                        static_cast<jlong>(status.f_bsize),
                                        static_cast<jlong>(status.f_frsize),
                                        static_cast<jlong>(status.f_blocks),
                                        static_cast<jlong>(status.f_bfree),
                                        static_cast<jlong>(status.f_bavail),
                                        static_cast<jlong>(status.f_files),
                                        static_cast<jlong>(status.f_ffree),
                                        static_cast<jlong>(status.f_favail),
                                        static_cast<jlong>(status.f_fsid),
                                        static_cast<jlong>(status.f_flag),
                                        static_cast<jlong>(status.f_namemax));
  env->DeleteLocalRef(klass);
  return result;
}

void DarwinUnsupported(JNIEnv *env, const char *operation) {
  ThrowErrno(env, operation, ENOTSUP);
}

template <typename Operation>
ssize_t RunInterruptibleIo(int fd, Operation operation, bool *was_signaled) {
  for (;;) {
    AsynchronousCloseMonitor monitor(fd);
    const ssize_t result = operation();
    const int saved_errno = errno;
    if (monitor.wasSignaled()) {
      *was_signaled = true;
      errno = EINTR;
      return -1;
    }
    if (result != -1 || saved_errno != EINTR) {
      *was_signaled = false;
      errno = saved_errno;
      return result;
    }
  }
}

jobject DarwinLinuxStat(JNIEnv *env, jobject, jstring java_path) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr) {
    return nullptr;
  }
  struct stat status{};
  if (Stat(path.c_str(), &status) == -1) {
    ThrowErrno(env, "stat", errno);
    return nullptr;
  }
  return MakeStructStat(env, status);
}

jboolean DarwinLinuxAccess(JNIEnv *env, jobject, jstring java_path, jint mode) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr) {
    return JNI_FALSE;
  }
  if ((mode & ~7) != 0) {
    jniThrowErrnoException(env, "access", 22);
    return JNI_FALSE;
  }
  if (Access(path.c_str(), mode) == -1) {
    ThrowErrno(env, "access", errno);
    return JNI_FALSE;
  }
  return JNI_TRUE;
}

void DarwinLinuxRemove(JNIEnv *env, jobject, jstring java_path) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() != nullptr && Remove(path.c_str()) == -1) {
    ThrowErrno(env, "remove", errno);
  }
}

jobject DarwinLinuxLstat(JNIEnv *env, jobject, jstring java_path) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr) {
    return nullptr;
  }
  struct stat status{};
  if (Lstat(path.c_str(), &status) == -1) {
    ThrowErrno(env, "lstat", errno);
    return nullptr;
  }
  return MakeStructStat(env, status);
}

void DarwinLinuxUnlink(JNIEnv *env, jobject, jstring java_path) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() != nullptr && Unlink(path.c_str()) == -1) {
    ThrowErrno(env, "unlink", errno);
  }
}

jstring DarwinLinuxReadlink(JNIEnv *env, jobject, jstring java_path) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr) {
    return nullptr;
  }
  std::vector<char> buffer(4096);
  const ssize_t length = Readlink(path.c_str(), buffer.data(), buffer.size());
  if (length == -1) {
    ThrowErrno(env, "readlink", errno);
    return nullptr;
  }
  if (static_cast<size_t>(length) == buffer.size()) {
    jniThrowErrnoException(env, "readlink", 36);  // ENAMETOOLONG
    return nullptr;
  }
  return env->NewStringUTF(std::string(buffer.data(), length).c_str());
}

void DarwinLinuxSymlink(JNIEnv *env, jobject, jstring java_old_path,
                        jstring java_new_path) {
  ScopedUtfChars old_path(env, java_old_path);
  ScopedUtfChars new_path(env, java_new_path);
  if (old_path.c_str() != nullptr && new_path.c_str() != nullptr &&
      Symlink(old_path.c_str(), new_path.c_str()) == -1) {
    ThrowErrno(env, "symlink", errno);
  }
}

void DarwinLinuxLink(JNIEnv *env, jobject, jstring java_old_path,
                     jstring java_new_path) {
  ScopedUtfChars old_path(env, java_old_path);
  ScopedUtfChars new_path(env, java_new_path);
  if (old_path.c_str() != nullptr && new_path.c_str() != nullptr &&
      Link(old_path.c_str(), new_path.c_str()) == -1) {
    ThrowErrno(env, "link", errno);
  }
}

void DarwinLinuxRename(JNIEnv *env, jobject, jstring java_old_path,
                       jstring java_new_path) {
  ScopedUtfChars old_path(env, java_old_path);
  ScopedUtfChars new_path(env, java_new_path);
  if (old_path.c_str() != nullptr && new_path.c_str() != nullptr &&
      Rename(old_path.c_str(), new_path.c_str()) == -1) {
    ThrowErrno(env, "rename", errno);
  }
}

jobject DarwinLinuxStatvfs(JNIEnv *env, jobject, jstring java_path) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr)
    return nullptr;
  if (darwin_art_bionic_fs_statvfs_core != nullptr && path.c_str()[0] == '/') {
    DarwinArtAndroidStatvfs status{};
    if (darwin_art_bionic_fs_statvfs_core(path.c_str(), &status) == -1) {
      const int android_error = darwin_art_bionic_errno_load == nullptr
                                    ? EIO
                                    : darwin_art_bionic_errno_load();
      jniThrowErrnoException(env, "statvfs", android_error);
      return nullptr;
    }
    return MakeStructStatVfs(env, status);
  }
  struct statvfs status{};
  if (statvfs(path.c_str(), &status) == -1) {
    ThrowErrno(env, "statvfs", errno);
    return nullptr;
  }
  return MakeStructStatVfs(env, status);
}

jobject DarwinLinuxFstatvfs(JNIEnv *env, jobject, jobject java_fd) {
  struct statvfs status{};
  if (fstatvfs(jniGetFDFromFileDescriptor(env, java_fd), &status) == -1) {
    ThrowErrno(env, "fstatvfs", errno);
    return nullptr;
  }
  return MakeStructStatVfs(env, status);
}

void DarwinLinuxChmod(JNIEnv *env, jobject, jstring java_path, jint mode) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr)
    return;
  if (darwin_art_bionic_fs_chmod_core != nullptr) {
    if (darwin_art_bionic_fs_chmod_core(path.c_str(),
                                        static_cast<uint32_t>(mode)) == -1) {
      const int android_error = darwin_art_bionic_errno_load == nullptr
                                    ? EIO
                                    : darwin_art_bionic_errno_load();
      jniThrowErrnoException(env, "chmod", android_error);
    }
  } else if (chmod(path.c_str(), static_cast<mode_t>(mode)) == -1) {
    ThrowErrno(env, "chmod", errno);
  }
}

void DarwinLinuxMkdir(JNIEnv *env, jobject, jstring java_path, jint mode) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr)
    return;
  if (darwin_art_bionic_fs_mkdir_core != nullptr) {
    if (darwin_art_bionic_fs_mkdir_core(path.c_str(),
                                        static_cast<uint32_t>(mode)) == -1) {
      const int android_error = darwin_art_bionic_errno_load == nullptr
                                    ? EIO
                                    : darwin_art_bionic_errno_load();
      jniThrowErrnoException(env, "mkdir", android_error);
    }
  } else if (mkdir(path.c_str(), static_cast<mode_t>(mode)) == -1) {
    ThrowErrno(env, "mkdir", errno);
  }
}

// Linux extended attributes through the guest filesystem (user namespace).
// Same shape as libcore_io_Linux.cpp: size query, read, retry on ERANGE.
namespace {
bool XattrProviderAvailable(JNIEnv *env, const char *operation) {
  if (darwin_art_bionic_fs_getxattr_core != nullptr &&
      darwin_art_bionic_fs_setxattr_core != nullptr &&
      darwin_art_bionic_fs_removexattr_core != nullptr &&
      darwin_art_bionic_fs_listxattr_core != nullptr &&
      darwin_art_bionic_errno_load != nullptr) {
    return true;
  }
  DarwinUnsupported(env, operation);
  return false;
}

void ThrowGuestErrno(JNIEnv *env, const char *operation) {
  jniThrowErrnoException(env, operation, darwin_art_bionic_errno_load());
}
}  // namespace

jbyteArray DarwinLinuxGetxattr(JNIEnv *env, jobject, jstring java_path,
                               jstring java_name) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr) return nullptr;
  ScopedUtfChars name(env, java_name);
  if (name.c_str() == nullptr) return nullptr;
  if (!XattrProviderAvailable(env, "getxattr")) return nullptr;
  constexpr int kAndroidErange = 34;
  while (true) {
    const ssize_t size =
        darwin_art_bionic_fs_getxattr_core(path.c_str(), name.c_str(), nullptr, 0, 0);
    if (size < 0) {
      ThrowGuestErrno(env, "getxattr");
      return nullptr;
    }
    std::vector<char> value(static_cast<size_t>(size));
    const ssize_t length = darwin_art_bionic_fs_getxattr_core(
        path.c_str(), name.c_str(), value.data(), value.size(), 0);
    if (length < 0) {
      // The value grew since the size query; ask again.
      if (darwin_art_bionic_errno_load() == kAndroidErange) continue;
      ThrowGuestErrno(env, "getxattr");
      return nullptr;
    }
    jbyteArray array = env->NewByteArray(static_cast<jsize>(length));
    if (array == nullptr) return nullptr;
    env->SetByteArrayRegion(array, 0, static_cast<jsize>(length),
                            reinterpret_cast<const jbyte *>(value.data()));
    return array;
  }
}

void DarwinLinuxSetxattr(JNIEnv *env, jobject, jstring java_path, jstring java_name,
                         jbyteArray java_value, jint flags) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr) return;
  ScopedUtfChars name(env, java_name);
  if (name.c_str() == nullptr) return;
  if (java_value == nullptr) {
    jniThrowNullPointerException(env, "value");
    return;
  }
  if (!XattrProviderAvailable(env, "setxattr")) return;
  const jsize length = env->GetArrayLength(java_value);
  std::vector<jbyte> value(static_cast<size_t>(length));
  env->GetByteArrayRegion(java_value, 0, length, value.data());
  if (darwin_art_bionic_fs_setxattr_core(path.c_str(), name.c_str(), value.data(),
                                         value.size(), flags, 0) != 0) {
    ThrowGuestErrno(env, "setxattr");
  }
}

void DarwinLinuxRemovexattr(JNIEnv *env, jobject, jstring java_path, jstring java_name) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr) return;
  ScopedUtfChars name(env, java_name);
  if (name.c_str() == nullptr) return;
  if (!XattrProviderAvailable(env, "removexattr")) return;
  if (darwin_art_bionic_fs_removexattr_core(path.c_str(), name.c_str(), 0) != 0) {
    ThrowGuestErrno(env, "removexattr");
  }
}

jobjectArray DarwinLinuxListxattr(JNIEnv *env, jobject, jstring java_path) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr) return nullptr;
  if (!XattrProviderAvailable(env, "listxattr")) return nullptr;
  constexpr int kAndroidErange = 34;
  while (true) {
    const ssize_t size = darwin_art_bionic_fs_listxattr_core(path.c_str(), nullptr, 0, 0);
    if (size < 0) {
      ThrowGuestErrno(env, "listxattr");
      return nullptr;
    }
    std::vector<char> names(static_cast<size_t>(size));
    const ssize_t length =
        darwin_art_bionic_fs_listxattr_core(path.c_str(), names.data(), names.size(), 0);
    if (length < 0) {
      if (darwin_art_bionic_errno_load() == kAndroidErange) continue;
      ThrowGuestErrno(env, "listxattr");
      return nullptr;
    }
    std::vector<std::string> entries;
    for (size_t start = 0; start < static_cast<size_t>(length);) {
      const size_t end = std::find(names.begin() + start, names.begin() + length, '\0') -
                         names.begin();
      entries.emplace_back(names.data() + start, end - start);
      start = end + 1;
    }
    jclass string_class = env->FindClass("java/lang/String");
    if (string_class == nullptr) return nullptr;
    jobjectArray result =
        env->NewObjectArray(static_cast<jsize>(entries.size()), string_class, nullptr);
    env->DeleteLocalRef(string_class);
    if (result == nullptr) return nullptr;
    for (size_t index = 0; index < entries.size(); ++index) {
      jstring entry = env->NewStringUTF(entries[index].c_str());
      if (entry == nullptr) return nullptr;
      env->SetObjectArrayElement(result, static_cast<jsize>(index), entry);
      env->DeleteLocalRef(entry);
    }
    return result;
  }
}

void DarwinLinuxFchmod(JNIEnv *env, jobject, jobject java_fd, jint mode) {
  const int fd = jniGetFDFromFileDescriptor(env, java_fd);
  if (darwin_art_bionic_fs_fchmod_core != nullptr) {
    if (darwin_art_bionic_fs_fchmod_core(fd, static_cast<uint32_t>(mode)) ==
        -1) {
      const int android_error = darwin_art_bionic_errno_load == nullptr
                                    ? EIO
                                    : darwin_art_bionic_errno_load();
      jniThrowErrnoException(env, "fchmod", android_error);
    }
  } else if (fchmod(fd, static_cast<mode_t>(mode)) == -1) {
    ThrowErrno(env, "fchmod", errno);
  }
}

jobjectArray DarwinLinuxAndroidGetaddrinfo(JNIEnv *env, jobject,
                                           jstring java_node,
                                           jobject java_hints, jint netid) {
  ScopedUtfChars node(env, java_node);
  if (node.c_str() == nullptr)
    return nullptr;
  addrinfo hints{};
  if (java_hints != nullptr) {
    jclass hints_class = env->GetObjectClass(java_hints);
    const jfieldID flags = env->GetFieldID(hints_class, "ai_flags", "I");
    const jfieldID family = env->GetFieldID(hints_class, "ai_family", "I");
    const jfieldID socktype = env->GetFieldID(hints_class, "ai_socktype", "I");
    const jfieldID protocol = env->GetFieldID(hints_class, "ai_protocol", "I");
    if (flags == nullptr || family == nullptr || socktype == nullptr ||
        protocol == nullptr || env->ExceptionCheck()) {
      env->DeleteLocalRef(hints_class);
      return nullptr;
    }
    hints.ai_flags = env->GetIntField(java_hints, flags);
    hints.ai_family = env->GetIntField(java_hints, family);
    hints.ai_socktype = env->GetIntField(java_hints, socktype);
    hints.ai_protocol = env->GetIntField(java_hints, protocol);
    env->DeleteLocalRef(hints_class);
  }
  const bool trace = ShouldLogAndroidGetaddrinfo();
  if (trace) {
    std::fprintf(stderr,
                 "DARWIN DNS libcore android_getaddrinfo node=%s netid=%d "
                 "flags=0x%x family=%d socktype=%d protocol=%d\n",
                 node.c_str(), netid, hints.ai_flags, hints.ai_family,
                 hints.ai_socktype, hints.ai_protocol);
  }
  addrinfo *addresses = nullptr;
  const int hints_status = DarwinArtTranslateDnsHints(&hints);
  const int status = hints_status != 0 ? hints_status
                                       : ::getaddrinfo(node.c_str(), nullptr,
                                                       &hints, &addresses);
  if (status != 0) {
    if (trace) {
      std::fprintf(stderr,
                   "DARWIN DNS libcore android_getaddrinfo failure node=%s "
                   "status=%d message=%s\n",
                   node.c_str(), status, gai_strerror(status));
    }
    // InetAddressUtils first attempts AI_NUMERICHOST and catches GaiException
    // to continue with DNS. Throwing UnknownHostException here prematurely
    // rejects every non-numeric hostname before any DNS query is made.
    jclass exception_class = env->FindClass("android/system/GaiException");
    jmethodID constructor = exception_class == nullptr
                                ? nullptr
                                : env->GetMethodID(exception_class, "<init>",
                                                   "(Ljava/lang/String;I)V");
    jstring operation = env->NewStringUTF("android_getaddrinfo");
    if (constructor != nullptr && operation != nullptr) {
      jobject exception =
          env->NewObject(exception_class, constructor, operation,
                         DarwinArtAndroidGaiError(status));
      if (exception != nullptr) {
        env->Throw(static_cast<jthrowable>(exception));
        env->DeleteLocalRef(exception);
      }
    }
    env->DeleteLocalRef(operation);
    env->DeleteLocalRef(exception_class);
    return nullptr;
  }
  std::vector<const addrinfo *> entries;
  for (addrinfo *current = addresses; current != nullptr;
       current = current->ai_next) {
    if (current->ai_family == AF_INET || current->ai_family == AF_INET6) {
      entries.push_back(current);
    }
  }
  if (trace) {
    std::fprintf(stderr,
                 "DARWIN DNS libcore android_getaddrinfo success node=%s "
                 "results=%zu\n",
                 node.c_str(), entries.size());
    for (const addrinfo *current : entries) {
      char address_text[INET6_ADDRSTRLEN] = {};
      const void *address = nullptr;
      if (current->ai_family == AF_INET) {
        address =
            &reinterpret_cast<const sockaddr_in *>(current->ai_addr)->sin_addr;
      } else if (current->ai_family == AF_INET6) {
        address = &reinterpret_cast<const sockaddr_in6 *>(current->ai_addr)
                       ->sin6_addr;
      }
      if (address != nullptr) {
        (void)inet_ntop(current->ai_family, address, address_text,
                        sizeof(address_text));
      }
      std::fprintf(stderr, "DARWIN DNS libcore address family=%d value=%s\n",
                   current->ai_family,
                   address_text[0] == '\0' ? "(unprintable)" : address_text);
    }
  }
  jclass inet_class = env->FindClass("java/net/InetAddress");
  jmethodID get_by_address =
      inet_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(
                inet_class, "getByAddress",
                "(Ljava/lang/String;[B)Ljava/net/InetAddress;");
  jobjectArray result =
      inet_class == nullptr || get_by_address == nullptr
          ? nullptr
          : env->NewObjectArray(static_cast<jsize>(entries.size()), inet_class,
                                nullptr);
  for (jsize index = 0;
       result != nullptr && index < static_cast<jsize>(entries.size());
       ++index) {
    const addrinfo *current = entries[static_cast<size_t>(index)];
    const size_t length = current->ai_family == AF_INET ? 4u : 16u;
    const uint8_t *address =
        current->ai_family == AF_INET
            ? reinterpret_cast<const uint8_t *>(
                  &reinterpret_cast<const sockaddr_in *>(current->ai_addr)
                       ->sin_addr)
            : reinterpret_cast<const uint8_t *>(
                  &reinterpret_cast<const sockaddr_in6 *>(current->ai_addr)
                       ->sin6_addr);
    jbyteArray bytes = env->NewByteArray(static_cast<jsize>(length));
    if (bytes == nullptr)
      break;
    env->SetByteArrayRegion(bytes, 0, static_cast<jsize>(length),
                            reinterpret_cast<const jbyte *>(address));
    jobject inet = env->CallStaticObjectMethod(inet_class, get_by_address,
                                               java_node, bytes);
    env->SetObjectArrayElement(result, index, inet);
    env->DeleteLocalRef(inet);
    env->DeleteLocalRef(bytes);
  }
  env->DeleteLocalRef(inet_class);
  freeaddrinfo(addresses);
  return result;
}

namespace {

constexpr uint16_t kAndroidAfInet = 2;
constexpr uint16_t kAndroidAfInet6 = 10;

struct AndroidSockaddrIn {
  uint16_t family;
  uint16_t port;
  uint32_t address;
  uint8_t zero[8];
};

struct AndroidSockaddrIn6 {
  uint16_t family;
  uint16_t port;
  uint32_t flowinfo;
  uint8_t address[16];
  uint32_t scope_id;
};

struct AndroidTimeval {
  int64_t seconds;
  int64_t microseconds;
};

static_assert(sizeof(AndroidSockaddrIn) == 16);
static_assert(sizeof(AndroidSockaddrIn6) == 28);

int BrokerErrno() {
  return darwin_art_bionic_errno_load == nullptr
             ? (errno == 0 ? EIO : errno)
             : darwin_art_bionic_errno_load();
}

void ThrowBrokerErrno(JNIEnv *env, const char *operation) {
  int error = BrokerErrno();
  if (error <= 0)
    error = EIO;
  if (std::getenv("DARWIN_ART_VERIFY_SOCKET_CLOSE") != nullptr)
    std::fprintf(stderr, "ART socket gate: %s Android errno=%d\n", operation, error);
  // The broker publishes Android errno values. Do not pass these through
  // ThrowErrno(), which intentionally translates Darwin errno values.
  jniThrowErrnoException(env, operation, error);
}

void DarwinLinuxKill(JNIEnv *env, jobject, jint pid, jint signal_number) {
  if (darwin_art_bionic_kill(pid, signal_number) == -1) {
    ThrowBrokerErrno(env, "kill");
  }
}

bool GetJavaFd(JNIEnv *env, jobject java_fd, const char *operation, int *fd) {
  if (java_fd == nullptr) {
    jniThrowNullPointerException(env, "null fd");
    return false;
  }
  *fd = jniGetFDFromFileDescriptor(env, java_fd);
  if (*fd < 0) {
    jniThrowErrnoException(env, operation, EBADF);
    return false;
  }
  return true;
}

bool InetAddressToAndroidSockaddr(JNIEnv *env, jobject java_address, jint port,
                                  bool force_ipv6, AndroidSockaddrIn6 *storage,
                                  uint32_t *length) {
  if (java_address == nullptr) {
    jniThrowNullPointerException(env, "null address");
    return false;
  }
  if (port < 0 || port > 65535) {
    jniThrowException(env, "java/lang/IllegalArgumentException",
                      "port out of range");
    return false;
  }
  jclass address_class = env->GetObjectClass(java_address);
  if (address_class == nullptr)
    return false;
  jmethodID get_address = env->GetMethodID(address_class, "getAddress", "()[B");
  if (get_address == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(address_class);
    return false;
  }
  jbyteArray bytes =
      static_cast<jbyteArray>(env->CallObjectMethod(java_address, get_address));
  if (bytes == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(address_class);
    return false;
  }
  const jsize byte_count = env->GetArrayLength(bytes);
  std::memset(storage, 0, sizeof(*storage));
  if (byte_count == 4 && !force_ipv6) {
    auto *ipv4 = reinterpret_cast<AndroidSockaddrIn *>(storage);
    ipv4->family = kAndroidAfInet;
    ipv4->port = htons(static_cast<uint16_t>(port));
    env->GetByteArrayRegion(bytes, 0, byte_count,
                            reinterpret_cast<jbyte *>(&ipv4->address));
    *length = sizeof(*ipv4);
  } else if (byte_count == 4 && force_ipv6) {
    storage->family = kAndroidAfInet6;
    storage->port = htons(static_cast<uint16_t>(port));
    uint8_t ipv4[4] = {};
    env->GetByteArrayRegion(bytes, 0, byte_count,
                            reinterpret_cast<jbyte *>(ipv4));
    const bool wildcard =
        ipv4[0] == 0 && ipv4[1] == 0 && ipv4[2] == 0 && ipv4[3] == 0;
    if (!wildcard) {
      storage->address[10] = 0xff;
      storage->address[11] = 0xff;
    }
    std::memcpy(storage->address + 12, ipv4, sizeof(ipv4));
    *length = sizeof(*storage);
  } else if (byte_count == 16) {
    storage->family = kAndroidAfInet6;
    storage->port = htons(static_cast<uint16_t>(port));
    env->GetByteArrayRegion(bytes, 0, byte_count,
                            reinterpret_cast<jbyte *>(storage->address));
    jmethodID get_scope = env->GetMethodID(address_class, "getScopeId", "()I");
    if (get_scope != nullptr && !env->ExceptionCheck()) {
      storage->scope_id =
          static_cast<uint32_t>(env->CallIntMethod(java_address, get_scope));
    }
    *length = sizeof(*storage);
  } else {
    env->DeleteLocalRef(bytes);
    env->DeleteLocalRef(address_class);
    jniThrowException(env, "java/lang/IllegalArgumentException",
                      "InetAddress has an invalid address length");
    return false;
  }
  env->DeleteLocalRef(address_class);
  env->DeleteLocalRef(bytes);
  return !env->ExceptionCheck();
}

bool SocketAddressToAndroidSockaddr(JNIEnv *env, jobject java_address,
                                    bool force_ipv6,
                                    AndroidSockaddrIn6 *storage,
                                    uint32_t *length) {
  if (java_address == nullptr) {
    jniThrowNullPointerException(env, "null socket address");
    return false;
  }
  jclass isa_class = env->FindClass("java/net/InetSocketAddress");
  if (isa_class == nullptr || !env->IsInstanceOf(java_address, isa_class)) {
    if (isa_class != nullptr)
      env->DeleteLocalRef(isa_class);
    jniThrowException(env, "java/lang/UnsupportedOperationException",
                      "only InetSocketAddress is supported");
    return false;
  }
  jmethodID get_address =
      env->GetMethodID(isa_class, "getAddress", "()Ljava/net/InetAddress;");
  jmethodID get_port = env->GetMethodID(isa_class, "getPort", "()I");
  jobject inet_address = get_address == nullptr
                             ? nullptr
                             : env->CallObjectMethod(java_address, get_address);
  const jint port =
      get_port == nullptr ? -1 : env->CallIntMethod(java_address, get_port);
  env->DeleteLocalRef(isa_class);
  if (inet_address == nullptr || env->ExceptionCheck()) {
    if (inet_address != nullptr)
      env->DeleteLocalRef(inet_address);
    if (!env->ExceptionCheck()) {
      jniThrowException(env, "java/net/UnknownHostException",
                        "unresolved socket address");
    }
    return false;
  }
  const bool result = InetAddressToAndroidSockaddr(env, inet_address, port,
                                                   force_ipv6, storage, length);
  env->DeleteLocalRef(inet_address);
  return result;
}

void CloseBrokerFdOnJniFailure(int fd) {
  if (fd >= 0)
    (void)darwin_art_bionic_socket_broker_close(fd);
}

uint16_t BrokerSocketFamily(int fd) {
  AndroidSockaddrIn6 local{};
  uint32_t length = sizeof(local);
  if (darwin_art_bionic_socket_broker_getsockname(fd, &local, &length) < 0)
    return 0;
  uint16_t family = 0;
  std::memcpy(&family, &local, sizeof(family));
  return family;
}

} // namespace

jobject DarwinLinuxSocket(JNIEnv *env, jobject, jint domain, jint type,
                          jint protocol) {
  const int fd = darwin_art_bionic_socket_broker_socket(domain, type, protocol);
  if (fd < 0) {
    ThrowBrokerErrno(env, "socket");
    return nullptr;
  }
  jobject result = jniCreateFileDescriptor(env, fd);
  if (result == nullptr)
    CloseBrokerFdOnJniFailure(fd);
  return result;
}

void DarwinLinuxConnect(JNIEnv *env, jobject, jobject java_fd,
                        jobject java_address, jint port) {
  int fd = -1;
  AndroidSockaddrIn6 storage{};
  uint32_t length = 0;
  if (!GetJavaFd(env, java_fd, "connect", &fd) ||
      !InetAddressToAndroidSockaddr(env, java_address, port,
                                    BrokerSocketFamily(fd) == kAndroidAfInet6,
                                    &storage, &length))
    return;
  if (darwin_art_bionic_socket_broker_connect(fd, &storage, length) < 0)
    ThrowBrokerErrno(env, "connect");
}

void DarwinLinuxConnectSocketAddress(JNIEnv *env, jobject, jobject java_fd,
                                     jobject java_address) {
  int fd = -1;
  AndroidSockaddrIn6 storage{};
  uint32_t length = 0;
  if (!GetJavaFd(env, java_fd, "connect", &fd) ||
      !SocketAddressToAndroidSockaddr(env, java_address,
                                      BrokerSocketFamily(fd) == kAndroidAfInet6,
                                      &storage, &length))
    return;
  if (darwin_art_bionic_socket_broker_connect(fd, &storage, length) < 0)
    ThrowBrokerErrno(env, "connect");
}

void DarwinLinuxBind(JNIEnv *env, jobject, jobject java_fd,
                     jobject java_address, jint port) {
  int fd = -1;
  AndroidSockaddrIn6 storage{};
  uint32_t length = 0;
  if (!GetJavaFd(env, java_fd, "bind", &fd) ||
      !InetAddressToAndroidSockaddr(env, java_address, port,
                                    BrokerSocketFamily(fd) == kAndroidAfInet6,
                                    &storage, &length))
    return;
  if (darwin_art_bionic_socket_broker_bind(fd, &storage, length) < 0)
    ThrowBrokerErrno(env, "bind");
}

void DarwinLinuxBindSocketAddress(JNIEnv *env, jobject, jobject java_fd,
                                  jobject java_address) {
  int fd = -1;
  AndroidSockaddrIn6 storage{};
  uint32_t length = 0;
  if (!GetJavaFd(env, java_fd, "bind", &fd) ||
      !SocketAddressToAndroidSockaddr(env, java_address,
                                      BrokerSocketFamily(fd) == kAndroidAfInet6,
                                      &storage, &length))
    return;
  if (darwin_art_bionic_socket_broker_bind(fd, &storage, length) < 0)
    ThrowBrokerErrno(env, "bind");
}

jint DarwinLinuxGetsockoptInt(JNIEnv *env, jobject, jobject java_fd, jint level,
                              jint option) {
  int fd = -1;
  if (!GetJavaFd(env, java_fd, "getsockopt", &fd))
    return -1;
  jint value = 0;
  uint32_t length = sizeof(value);
  if (darwin_art_bionic_socket_broker_getsockopt(fd, level, option, &value,
                                                 &length) < 0) {
    ThrowBrokerErrno(env, "getsockopt");
    return -1;
  }
  if (length != sizeof(value)) {
    jniThrowException(env, "java/lang/IllegalStateException",
                      "getsockopt returned an invalid integer size");
    return -1;
  }
  return value;
}

void DarwinLinuxSetsockoptInt(JNIEnv *env, jobject, jobject java_fd, jint level,
                              jint option, jint value) {
  int fd = -1;
  if (!GetJavaFd(env, java_fd, "setsockopt", &fd))
    return;
  if (darwin_art_bionic_socket_broker_setsockopt(fd, level, option, &value,
                                                 sizeof(value)) < 0)
    ThrowBrokerErrno(env, "setsockopt");
}

jobject DarwinLinuxGetsockoptTimeval(JNIEnv *env, jobject, jobject java_fd,
                                     jint level, jint option) {
  int fd = -1;
  if (!GetJavaFd(env, java_fd, "getsockopt", &fd))
    return nullptr;
  AndroidTimeval value{};
  uint32_t length = sizeof(value);
  if (darwin_art_bionic_socket_broker_getsockopt(fd, level, option, &value,
                                                 &length) < 0) {
    ThrowBrokerErrno(env, "getsockopt");
    return nullptr;
  }
  if (length != sizeof(value) || value.microseconds < 0 ||
      value.microseconds >= 1000000) {
    jniThrowException(env, "java/lang/IllegalStateException",
                      "getsockopt returned an invalid timeval");
    return nullptr;
  }
  jclass timeval_class = env->FindClass("android/system/StructTimeval");
  if (timeval_class == nullptr)
    return nullptr;
  jmethodID constructor = env->GetMethodID(timeval_class, "<init>", "(JJ)V");
  jobject result = constructor == nullptr
                       ? nullptr
                       : env->NewObject(timeval_class, constructor,
                                        static_cast<jlong>(value.seconds),
                                        static_cast<jlong>(value.microseconds));
  env->DeleteLocalRef(timeval_class);
  return result;
}

void DarwinLinuxSetsockoptTimeval(JNIEnv *env, jobject, jobject java_fd,
                                  jint level, jint option,
                                  jobject java_timeval) {
  int fd = -1;
  if (!GetJavaFd(env, java_fd, "setsockopt", &fd))
    return;
  if (java_timeval == nullptr) {
    jniThrowNullPointerException(env, "null timeval");
    return;
  }
  jclass timeval_class = env->FindClass("android/system/StructTimeval");
  if (timeval_class == nullptr)
    return;
  jfieldID seconds = env->GetFieldID(timeval_class, "tv_sec", "J");
  jfieldID microseconds = env->GetFieldID(timeval_class, "tv_usec", "J");
  if (seconds == nullptr || microseconds == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(timeval_class);
    return;
  }
  const jlong seconds_value = env->GetLongField(java_timeval, seconds);
  const jlong microseconds_value =
      env->GetLongField(java_timeval, microseconds);
  env->DeleteLocalRef(timeval_class);
  if (seconds_value < 0 || microseconds_value < 0 ||
      microseconds_value >= 1000000) {
    jniThrowException(env, "java/lang/IllegalArgumentException",
                      "invalid timeval");
    return;
  }
  AndroidTimeval value{seconds_value, microseconds_value};
  if (darwin_art_bionic_socket_broker_setsockopt(fd, level, option, &value,
                                                 sizeof(value)) < 0)
    ThrowBrokerErrno(env, "setsockopt");
}

void DarwinLinuxShutdown(JNIEnv *env, jobject, jobject java_fd, jint how) {
  int fd = -1;
  if (!GetJavaFd(env, java_fd, "shutdown", &fd))
    return;
  if (darwin_art_bionic_socket_broker_shutdown(fd, how) < 0)
    ThrowBrokerErrno(env, "shutdown");
}

jobject DarwinLinuxGetsockname(JNIEnv *env, jobject, jobject java_fd) {
  int fd = -1;
  if (!GetJavaFd(env, java_fd, "getsockname", &fd))
    return nullptr;
  AndroidSockaddrIn6 storage{};
  uint32_t length = sizeof(storage);
  if (darwin_art_bionic_socket_broker_getsockname(fd, &storage, &length) < 0) {
    ThrowBrokerErrno(env, "getsockname");
    return nullptr;
  }
  uint16_t family = 0;
  std::memcpy(&family, &storage, sizeof(family));
  size_t address_length = 0;
  const uint8_t *address = nullptr;
  uint16_t port = 0;
  if (family == kAndroidAfInet && length >= sizeof(AndroidSockaddrIn)) {
    const auto *ipv4 = reinterpret_cast<const AndroidSockaddrIn *>(&storage);
    address = reinterpret_cast<const uint8_t *>(&ipv4->address);
    address_length = 4;
    port = ntohs(ipv4->port);
  } else if (family == kAndroidAfInet6 &&
             length >= sizeof(AndroidSockaddrIn6)) {
    address = storage.address;
    address_length = sizeof(storage.address);
    port = ntohs(storage.port);
  } else {
    jniThrowException(env, "java/lang/UnsupportedOperationException",
                      "getsockname returned an unsupported address family");
    return nullptr;
  }
  jclass inet_class = env->FindClass("java/net/InetAddress");
  jclass socket_class = env->FindClass("java/net/InetSocketAddress");
  if (inet_class == nullptr || socket_class == nullptr) {
    if (inet_class != nullptr)
      env->DeleteLocalRef(inet_class);
    if (socket_class != nullptr)
      env->DeleteLocalRef(socket_class);
    return nullptr;
  }
  jmethodID from_bytes = env->GetStaticMethodID(inet_class, "getByAddress",
                                                "([B)Ljava/net/InetAddress;");
  jmethodID constructor =
      env->GetMethodID(socket_class, "<init>", "(Ljava/net/InetAddress;I)V");
  jbyteArray bytes = env->NewByteArray(static_cast<jsize>(address_length));
  if (from_bytes == nullptr || constructor == nullptr || bytes == nullptr) {
    env->DeleteLocalRef(inet_class);
    env->DeleteLocalRef(socket_class);
    if (bytes != nullptr)
      env->DeleteLocalRef(bytes);
    return nullptr;
  }
  env->SetByteArrayRegion(bytes, 0, static_cast<jsize>(address_length),
                          reinterpret_cast<const jbyte *>(address));
  jobject inet = env->CallStaticObjectMethod(inet_class, from_bytes, bytes);
  jobject result = inet == nullptr
                       ? nullptr
                       : env->NewObject(socket_class, constructor, inet,
                                        static_cast<jint>(port));
  if (inet != nullptr)
    env->DeleteLocalRef(inet);
  env->DeleteLocalRef(bytes);
  env->DeleteLocalRef(inet_class);
  env->DeleteLocalRef(socket_class);
  return result;
}

jint DarwinLinuxPoll(JNIEnv *env, jobject, jobjectArray java_structs,
                     jint timeout_ms) {
  if (java_structs == nullptr) {
    jniThrowNullPointerException(env, "null pollfd array");
    return -1;
  }
  jclass poll_class = env->FindClass("android/system/StructPollfd");
  if (poll_class == nullptr)
    return -1;
  jfieldID fd_field =
      env->GetFieldID(poll_class, "fd", "Ljava/io/FileDescriptor;");
  jfieldID events_field = env->GetFieldID(poll_class, "events", "S");
  jfieldID revents_field = env->GetFieldID(poll_class, "revents", "S");
  if (fd_field == nullptr || events_field == nullptr ||
      revents_field == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(poll_class);
    return -1;
  }
  const jsize array_length = env->GetArrayLength(java_structs);
  std::vector<DarwinArtBionicPollFd> descriptors;
  descriptors.reserve(static_cast<size_t>(array_length));
  std::vector<jobject> entries;
  entries.reserve(static_cast<size_t>(array_length));
  std::vector<size_t> descriptor_entries;
  descriptor_entries.reserve(static_cast<size_t>(array_length));
  for (jsize index = 0; index < array_length; ++index) {
    jobject pollfd = env->GetObjectArrayElement(java_structs, index);
    if (pollfd == nullptr)
      break;
    jobject java_fd = env->GetObjectField(pollfd, fd_field);
    if (java_fd == nullptr) {
      env->DeleteLocalRef(pollfd);
      break;
    }
    const int fd = jniGetFDFromFileDescriptor(env, java_fd);
    env->DeleteLocalRef(java_fd);
    if (fd < 0) {
      // POSIX poll ignores negative descriptors and leaves revents zero.
      // Keep the Java entry so fields are updated consistently below.
      entries.push_back(pollfd);
      continue;
    }
    descriptors.push_back(
        DarwinArtBionicPollFd{fd, env->GetShortField(pollfd, events_field), 0});
    descriptor_entries.push_back(entries.size());
    entries.push_back(pollfd);
  }
  const int result = darwin_art_bionic_socket_broker_poll(
      descriptors.data(), descriptors.size(), timeout_ms);
  if (result < 0) {
    ThrowBrokerErrno(env, "poll");
  } else {
    for (jobject entry : entries)
      env->SetShortField(entry, revents_field, 0);
    for (size_t index = 0; index < descriptors.size(); ++index) {
      env->SetShortField(entries[descriptor_entries[index]], revents_field,
                         descriptors[index].revents);
    }
  }
  for (jobject entry : entries)
    env->DeleteLocalRef(entry);
  env->DeleteLocalRef(poll_class);
  return result;
}

int DarwinEaiFromAndroid(jint error) {
  switch (error) {
  case 0:
    return 0;
  case 1:
    return EAI_ADDRFAMILY;
  case 2:
    return EAI_AGAIN;
  case 3:
    return EAI_BADFLAGS;
  case 4:
    return EAI_FAIL;
  case 5:
    return EAI_FAMILY;
  case 6:
    return EAI_MEMORY;
  case 7:
    return EAI_NODATA;
  case 8:
    return EAI_NONAME;
  case 9:
    return EAI_SERVICE;
  case 10:
    return EAI_SOCKTYPE;
  case 11:
    return EAI_SYSTEM;
#ifdef EAI_BADHINTS
  case 12:
    return EAI_BADHINTS;
#endif
#ifdef EAI_PROTOCOL
  case 13:
    return EAI_PROTOCOL;
#endif
  case 14:
    return EAI_OVERFLOW;
  default:
    return EAI_FAIL;
  }
}

jstring DarwinLinuxGaiStrerror(JNIEnv *env, jobject, jint error) {
  return env->NewStringUTF(gai_strerror(DarwinEaiFromAndroid(error)));
}

void DarwinLinuxFdsanExchangeOwnerTag(JNIEnv *, jclass, jobject, jlong, jlong) {
  // Match upstream's complete non-Bionic branch. Java still updates
  // FileDescriptor.ownerId; Darwin libc has no fdsan kernel/libc side table.
}

jlong DarwinLinuxFdsanGetOwnerTag(JNIEnv *, jclass, jobject) { return 0; }

jstring DarwinLinuxFdsanGetTagType(JNIEnv *env, jclass, jlong) {
  return env->NewStringUTF("unknown");
}

jlong DarwinLinuxFdsanGetTagValue(JNIEnv *, jclass, jlong) { return 0; }

jint DarwinLinuxWriteBytes(JNIEnv *env, jobject, jobject java_fd,
                           jobject java_bytes, jint byte_offset,
                           jint byte_count) {
  if (java_bytes == nullptr) {
    jniThrowNullPointerException(env, "null byte storage");
    return -1;
  }
  if (byte_offset < 0 || byte_count < 0) {
    jniThrowException(env, "java/lang/ArrayIndexOutOfBoundsException",
                      "negative offset or byte count");
    return -1;
  }
  const void *bytes = nullptr;
  jbyteArray array = nullptr;
  jbyte *array_elements = nullptr;
  jclass byte_array_class = env->FindClass("[B");
  if (byte_array_class != nullptr &&
      env->IsInstanceOf(java_bytes, byte_array_class)) {
    array = static_cast<jbyteArray>(java_bytes);
    const jsize length = env->GetArrayLength(array);
    if (byte_offset > length || byte_count > length - byte_offset) {
      env->DeleteLocalRef(byte_array_class);
      jniThrowException(env, "java/lang/ArrayIndexOutOfBoundsException",
                        "byte range exceeds array");
      return -1;
    }
    array_elements = env->GetByteArrayElements(array, nullptr);
    bytes = array_elements == nullptr
                ? nullptr
                : static_cast<const void *>(array_elements + byte_offset);
  } else {
    const jlong capacity = env->GetDirectBufferCapacity(java_bytes);
    const void *direct = env->GetDirectBufferAddress(java_bytes);
    if (direct == nullptr || byte_offset > capacity ||
        byte_count > capacity - byte_offset) {
      env->DeleteLocalRef(byte_array_class);
      jniThrowException(
          env, "java/lang/IllegalArgumentException",
          "storage is neither byte[] nor a valid direct buffer range");
      return -1;
    }
    bytes = static_cast<const char *>(direct) + byte_offset;
  }
  env->DeleteLocalRef(byte_array_class);
  if (bytes == nullptr) {
    return -1;
  }
  const int fd = jniGetFDFromFileDescriptor(env, java_fd);
  bool was_signaled = false;
  const ssize_t result = RunInterruptibleIo(
      fd, [&] { return Write(fd, bytes, static_cast<size_t>(byte_count)); },
      &was_signaled);
  const int saved_errno = errno;
  if (array_elements != nullptr) {
    env->ReleaseByteArrayElements(array, array_elements, JNI_ABORT);
  }
  if (was_signaled) {
    jniThrowException(env, "java/io/InterruptedIOException",
                      "write interrupted by close() on another thread");
    return -1;
  }
  if (result == -1) {
    ThrowErrno(env, "write", saved_errno);
    return -1;
  }
  return static_cast<jint>(result);
}

jint DarwinLinuxPwriteBytes(JNIEnv *env, jobject, jobject java_fd,
                            jobject java_bytes, jint byte_offset,
                            jint byte_count, jlong offset) {
  if (java_bytes == nullptr) {
    jniThrowNullPointerException(env, "null byte storage");
    return -1;
  }
  if (byte_offset < 0 || byte_count < 0 || offset < 0) {
    jniThrowException(env, "java/lang/ArrayIndexOutOfBoundsException",
                      "negative offset or byte count");
    return -1;
  }
  const void *bytes = nullptr;
  jbyteArray array = nullptr;
  jbyte *array_elements = nullptr;
  jclass byte_array_class = env->FindClass("[B");
  if (byte_array_class != nullptr &&
      env->IsInstanceOf(java_bytes, byte_array_class)) {
    array = static_cast<jbyteArray>(java_bytes);
    const jsize length = env->GetArrayLength(array);
    if (byte_offset > length || byte_count > length - byte_offset) {
      env->DeleteLocalRef(byte_array_class);
      jniThrowException(env, "java/lang/ArrayIndexOutOfBoundsException",
                        "byte range exceeds array");
      return -1;
    }
    array_elements = env->GetByteArrayElements(array, nullptr);
    bytes = array_elements == nullptr
                ? nullptr
                : static_cast<const void *>(array_elements + byte_offset);
  } else {
    const jlong capacity = env->GetDirectBufferCapacity(java_bytes);
    const void *direct = env->GetDirectBufferAddress(java_bytes);
    if (direct == nullptr || byte_offset > capacity ||
        byte_count > capacity - byte_offset) {
      env->DeleteLocalRef(byte_array_class);
      jniThrowException(
          env, "java/lang/IllegalArgumentException",
          "storage is neither byte[] nor a valid direct buffer range");
      return -1;
    }
    bytes = static_cast<const char *>(direct) + byte_offset;
  }
  env->DeleteLocalRef(byte_array_class);
  if (bytes == nullptr)
    return -1;
  const ssize_t result = Pwrite(jniGetFDFromFileDescriptor(env, java_fd), bytes,
                                static_cast<size_t>(byte_count), offset);
  const int saved_errno = errno;
  if (array_elements != nullptr) {
    env->ReleaseByteArrayElements(array, array_elements, JNI_ABORT);
  }
  if (result == -1) {
    ThrowErrno(env, "pwrite", saved_errno);
    return -1;
  }
  return static_cast<jint>(result);
}

#if defined(DARWIN_LIBCORE_LINUX_MANAGED_ABI_SMOKE)
void AbiSmokeVoid(JNIEnv *env, jobject, jstring, jint) {
  DarwinUnsupported(env, "abi.void");
}
jint AbiSmokeInt(JNIEnv *env, jobject, jobject, jint, jint) {
  DarwinUnsupported(env, "abi.int");
  return 0;
}
jlong AbiSmokeLong(JNIEnv *env, jobject, jobject) {
  DarwinUnsupported(env, "abi.long");
  return 0;
}
jobject AbiSmokeObject(JNIEnv *env, jobject, jstring) {
  DarwinUnsupported(env, "abi.object");
  return nullptr;
}
jboolean AbiSmokeBoolean(JNIEnv *env, jobject, jstring, jint) {
  DarwinUnsupported(env, "abi.boolean");
  return JNI_FALSE;
}
jlong AbiSmokeAvailableProcessors(JNIEnv *env, jobject receiver) {
  constexpr jint kAndroidScNprocessorsConf = 96;
  return DarwinLinuxSysconf(env, receiver, kAndroidScNprocessorsConf);
}
jstring AbiSmokeStrerror(JNIEnv *env, jobject receiver, jint error_number) {
  return DarwinLinuxStrerror(env, receiver, error_number);
}
jstring AbiSmokeStrsignal(JNIEnv *env, jobject receiver, jint signal_number) {
  return DarwinLinuxStrsignal(env, receiver, signal_number);
}
jint AbiSmokeWriteFile(JNIEnv *env, jobject, jstring java_path,
                       jbyteArray java_bytes) {
  constexpr int kAndroidOCreat = 64;
  constexpr int kAndroidOTrunc = 512;
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr || java_bytes == nullptr) {
    return -1;
  }
  jbyte *bytes = env->GetByteArrayElements(java_bytes, nullptr);
  if (bytes == nullptr) {
    return -1;
  }
  const jsize size = env->GetArrayLength(java_bytes);
  const int fd = Open(path.c_str(), kAndroidOTrunc | kAndroidOCreat | 1, 0600);
  const ssize_t written =
      fd == -1 ? -1 : Write(fd, bytes, static_cast<size_t>(size));
  struct stat status{};
  const int stat_result = fd == -1 ? -1 : Fstat(fd, &status);
  const int saved_errno = errno;
  if (fd != -1) {
    Close(fd);
  }
  env->ReleaseByteArrayElements(java_bytes, bytes, JNI_ABORT);
  if (written == -1 || stat_result == -1 || status.st_size != written) {
    ThrowErrno(env, "write", saved_errno == 0 ? EIO : saved_errno);
    return -1;
  }
  return static_cast<jint>(written);
}
#endif

jobject DarwinLinuxOpen(JNIEnv *env, jobject, jstring java_path, jint flags,
                        jint mode) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr) {
    return nullptr;
  }
  const int fd = Open(path.c_str(), flags, static_cast<mode_t>(mode));
  if (fd == -1) {
    ThrowErrno(env, "open", errno);
    return nullptr;
  }
  jobject result = jniCreateFileDescriptor(env, fd);
  if (result == nullptr) {
    Close(fd);
  }
  return result;
}

jobject DarwinLinuxDup(JNIEnv *env, jobject, jobject java_fd) {
  if (java_fd == nullptr) {
    jniThrowNullPointerException(env, "null fd");
    return nullptr;
  }
  const int duplicate = Dup(jniGetFDFromFileDescriptor(env, java_fd));
  if (duplicate == -1) {
    ThrowErrno(env, "dup", errno);
    return nullptr;
  }
  jobject result = jniCreateFileDescriptor(env, duplicate);
  if (result == nullptr)
    Close(duplicate);
  return result;
}

jint DarwinLinuxFcntlInt(JNIEnv *env, jobject, jobject java_fd, jint command,
                         jint argument) {
  if (java_fd == nullptr) {
    jniThrowNullPointerException(env, "null fd");
    return -1;
  }
  const int result =
      Fcntl(jniGetFDFromFileDescriptor(env, java_fd), command, argument);
  if (result == -1)
    ThrowErrno(env, "fcntl", errno);
  return result;
}

jint DarwinLinuxFcntlVoid(JNIEnv *env, jobject, jobject java_fd, jint command) {
  return DarwinLinuxFcntlInt(env, nullptr, java_fd, command, 0);
}

void DarwinLinuxClose(JNIEnv *env, jobject, jobject java_fd) {
  if (java_fd == nullptr) {
    jniThrowNullPointerException(env, "null fd");
    return;
  }
  const int fd = jniGetFDFromFileDescriptor(env, java_fd);
  jniSetFileDescriptorOfFD(env, java_fd, -1);
  if (Close(fd) == -1) {
    ThrowErrno(env, "close", errno);
  }
}

jobject DarwinLinuxDup2(JNIEnv *env, jobject, jobject java_fd, jint new_fd) {
  int old_fd = -1;
  if (!GetJavaFd(env, java_fd, "dup2", &old_fd))
    return nullptr;
  if (new_fd < 0) {
    ThrowErrno(env, "dup2", EBADF);
    return nullptr;
  }
  const int result_fd = darwin_art_bionic_socket_broker_dup2(old_fd, new_fd);
  if (result_fd == -1) {
    ThrowBrokerErrno(env, "dup2");
    return nullptr;
  }
  jobject result = jniCreateFileDescriptor(env, result_fd);
  return result;
}

void DarwinLinuxSocketpair(JNIEnv *env, jobject, jint domain, jint type,
                           jint protocol, jobject java_fd1, jobject java_fd2) {
  const bool trace = std::getenv("DARWIN_ART_VERIFY_SOCKET_CLOSE") != nullptr;
  if (trace) std::fprintf(stderr, "ART socket gate: socketpair domain=%d type=%d protocol=%d\n", domain, type, protocol);
  if (java_fd1 == nullptr) {
    jniThrowNullPointerException(env, "null fd1");
    return;
  }
  if (java_fd2 == nullptr) {
    jniThrowNullPointerException(env, "null fd2");
    return;
  }
  int32_t descriptors[2] = {-1, -1};
  if (darwin_art_bionic_socket_broker_socketpair(domain, type, protocol,
                                                 descriptors) == -1) {
    if (trace) std::fprintf(stderr, "ART socket gate: socketpair failed\n");
    ThrowBrokerErrno(env, "socketpair");
    return;
  }
  jniSetFileDescriptorOfFD(env, java_fd1, descriptors[0]);
  jniSetFileDescriptorOfFD(env, java_fd2, descriptors[1]);
  if (trace) std::fprintf(stderr, "ART socket gate: socketpair descriptors=%d,%d\n", descriptors[0], descriptors[1]);
}

// Linux.pipe2: both ends published through the central broker, like
// bionic's pipe2, so Os.read/Os.write reach them from any thread.
jobjectArray DarwinLinuxPipe2(JNIEnv *env, jobject, jint flags) {
  int32_t descriptors[2] = {-1, -1};
  if (darwin_art_bionic_socket_broker_pipe2(descriptors, flags) == -1) {
    ThrowBrokerErrno(env, "pipe2");
    return nullptr;
  }
  jclass descriptor_class = env->FindClass("java/io/FileDescriptor");
  jobjectArray result =
      descriptor_class == nullptr
          ? nullptr
          : env->NewObjectArray(2, descriptor_class, nullptr);
  if (descriptor_class != nullptr) env->DeleteLocalRef(descriptor_class);
  for (int index = 0; result != nullptr && index < 2; ++index) {
    jobject descriptor = jniCreateFileDescriptor(env, descriptors[index]);
    if (descriptor == nullptr) {
      result = nullptr;
      break;
    }
    env->SetObjectArrayElement(result, index, descriptor);
    env->DeleteLocalRef(descriptor);
  }
  if (result == nullptr) {
    // The Java owner never received them: close both ends here.
    (void)darwin_art_bionic_socket_broker_close(descriptors[0]);
    (void)darwin_art_bionic_socket_broker_close(descriptors[1]);
  }
  return result;
}

jobject DarwinLinuxFstat(JNIEnv *env, jobject, jobject java_fd) {
  struct stat status{};
  if (Fstat(jniGetFDFromFileDescriptor(env, java_fd), &status) == -1) {
    ThrowErrno(env, "fstat", errno);
    return nullptr;
  }
  return MakeStructStat(env, status);
}

void DarwinLinuxFsync(JNIEnv *env, jobject, jobject java_fd) {
  int fd = -1;
  if (!GetJavaFd(env, java_fd, "fsync", &fd)) return;
  if (Fsync(fd) == -1) {
    ThrowErrno(env, "fsync", errno);
  }
}

// The facade's descriptors have no separate metadata flush.
void DarwinLinuxFdatasync(JNIEnv *env, jobject, jobject java_fd) {
  int fd = -1;
  if (!GetJavaFd(env, java_fd, "fdatasync", &fd)) return;
  if (Fsync(fd) == -1) {
    ThrowErrno(env, "fdatasync", errno);
  }
}

void DarwinLinuxPosixFallocate(JNIEnv *env, jobject, jobject java_fd,
                               jlong offset, jlong length) {
  int fd = -1;
  if (!GetJavaFd(env, java_fd, "posix_fallocate", &fd)) return;
  const int error = PosixFallocate(fd, offset, length);
  if (error != 0) {
    jniThrowErrnoException(env, "posix_fallocate", error);
  }
}

void DarwinLinuxFtruncate(JNIEnv *env, jobject, jobject java_fd,
                          jlong length) {
  int fd = -1;
  if (!GetJavaFd(env, java_fd, "ftruncate", &fd)) return;
  if (Ftruncate(fd, length) == -1) {
    ThrowErrno(env, "ftruncate", errno);
  }
}

jint DarwinLinuxReadBytes(JNIEnv *env, jobject, jobject java_fd,
                          jobject java_bytes, jint byte_offset,
                          jint byte_count) {
  if (java_bytes == nullptr) {
    jniThrowNullPointerException(env, "null byte storage");
    return -1;
  }
  if (byte_offset < 0 || byte_count < 0) {
    jniThrowException(env, "java/lang/ArrayIndexOutOfBoundsException",
                      "negative offset or byte count");
    return -1;
  }

  void *bytes = nullptr;
  jbyteArray array = nullptr;
  jclass byte_array_class = env->FindClass("[B");
  if (byte_array_class != nullptr &&
      env->IsInstanceOf(java_bytes, byte_array_class)) {
    array = static_cast<jbyteArray>(java_bytes);
    const jsize length = env->GetArrayLength(array);
    if (byte_offset > length || byte_count > length - byte_offset) {
      env->DeleteLocalRef(byte_array_class);
      jniThrowException(env, "java/lang/ArrayIndexOutOfBoundsException",
                        "byte range exceeds array");
      return -1;
    }
    bytes = env->GetByteArrayElements(array, nullptr);
  } else {
    const jlong capacity = env->GetDirectBufferCapacity(java_bytes);
    bytes = env->GetDirectBufferAddress(java_bytes);
    if (bytes == nullptr || byte_offset > capacity ||
        byte_count > capacity - byte_offset) {
      env->DeleteLocalRef(byte_array_class);
      jniThrowException(
          env, "java/lang/IllegalArgumentException",
          "storage is neither byte[] nor a valid direct buffer range");
      return -1;
    }
  }
  env->DeleteLocalRef(byte_array_class);
  if (bytes == nullptr) {
    return -1;
  }

  const int fd = jniGetFDFromFileDescriptor(env, java_fd);
  bool was_signaled = false;
  const ssize_t result = RunInterruptibleIo(
      fd,
      [&] {
        return Read(fd, static_cast<char *>(bytes) + byte_offset,
                    static_cast<size_t>(byte_count));
      },
      &was_signaled);
  const int saved_errno = errno;
  if (array != nullptr) {
    env->ReleaseByteArrayElements(array, static_cast<jbyte *>(bytes), 0);
  }
  if (was_signaled) {
    jniThrowException(env, "java/io/InterruptedIOException",
                      "read interrupted by close() on another thread");
    return -1;
  }
  if (result == -1) {
    ThrowErrno(env, "read", saved_errno);
  }
  if (result > std::numeric_limits<jint>::max()) {
    ThrowErrno(env, "read", EOVERFLOW);
    return -1;
  }
  return static_cast<jint>(result);
}

jint DarwinLinuxPreadBytes(JNIEnv *env, jobject, jobject java_fd,
                           jobject java_bytes, jint byte_offset,
                           jint byte_count, jlong offset) {
  if (java_bytes == nullptr) {
    jniThrowNullPointerException(env, "null byte storage");
    return -1;
  }
  if (byte_offset < 0 || byte_count < 0 || offset < 0) {
    jniThrowException(env, "java/lang/ArrayIndexOutOfBoundsException",
                      "negative offset or byte count");
    return -1;
  }
  void *bytes = nullptr;
  jbyteArray array = nullptr;
  jclass byte_array_class = env->FindClass("[B");
  if (byte_array_class != nullptr &&
      env->IsInstanceOf(java_bytes, byte_array_class)) {
    array = static_cast<jbyteArray>(java_bytes);
    const jsize length = env->GetArrayLength(array);
    if (byte_offset > length || byte_count > length - byte_offset) {
      env->DeleteLocalRef(byte_array_class);
      jniThrowException(env, "java/lang/ArrayIndexOutOfBoundsException",
                        "byte range exceeds array");
      return -1;
    }
    bytes = env->GetByteArrayElements(array, nullptr);
  } else {
    const jlong capacity = env->GetDirectBufferCapacity(java_bytes);
    bytes = env->GetDirectBufferAddress(java_bytes);
    if (bytes == nullptr || byte_offset > capacity ||
        byte_count > capacity - byte_offset) {
      env->DeleteLocalRef(byte_array_class);
      jniThrowException(
          env, "java/lang/IllegalArgumentException",
          "storage is neither byte[] nor a valid direct buffer range");
      return -1;
    }
  }
  env->DeleteLocalRef(byte_array_class);
  if (bytes == nullptr)
    return -1;
  const ssize_t result = Pread(jniGetFDFromFileDescriptor(env, java_fd),
                               static_cast<char *>(bytes) + byte_offset,
                               static_cast<size_t>(byte_count), offset);
  const int saved_errno = errno;
  if (array != nullptr) {
    env->ReleaseByteArrayElements(array, static_cast<jbyte *>(bytes), 0);
  }
  if (result == -1) {
    ThrowErrno(env, "pread", saved_errno);
    return -1;
  }
  return static_cast<jint>(result);
}

jlong DarwinLinuxLseek(JNIEnv *env, jobject, jobject java_fd, jlong offset,
                       jint whence) {
  const int64_t result = Lseek(jniGetFDFromFileDescriptor(env, java_fd),
                               static_cast<int64_t>(offset), whence);
  if (result == -1) {
    ThrowErrno(env, "lseek", errno);
    return -1;
  }
  return static_cast<jlong>(result);
}

jlong DarwinLinuxSendfile(JNIEnv *env, jobject, jobject java_output_fd,
                          jobject java_input_fd, jobject java_offset,
                          jlong byte_count) {
  if (java_output_fd == nullptr || java_input_fd == nullptr) {
    jniThrowNullPointerException(env, "null fd");
    return -1;
  }
  if (byte_count < 0 || static_cast<unsigned long long>(byte_count) >
                            std::numeric_limits<size_t>::max()) {
    ThrowErrno(env, "sendfile", EINVAL);
    return -1;
  }

  int64_t offset = 0;
  int64_t *offset_pointer = nullptr;
  jclass offset_class = nullptr;
  jfieldID value_field = nullptr;
  if (java_offset != nullptr) {
    offset_class = env->GetObjectClass(java_offset);
    value_field = offset_class == nullptr
                      ? nullptr
                      : env->GetFieldID(offset_class, "value", "J");
    if (value_field == nullptr) {
      env->DeleteLocalRef(offset_class);
      return -1;
    }
    offset = env->GetLongField(java_offset, value_field);
    offset_pointer = &offset;
  }

  const intptr_t result =
      Sendfile(jniGetFDFromFileDescriptor(env, java_output_fd),
               jniGetFDFromFileDescriptor(env, java_input_fd), offset_pointer,
               static_cast<size_t>(byte_count));
  if (result == -1) {
    env->DeleteLocalRef(offset_class);
    ThrowErrno(env, "sendfile", errno);
    return -1;
  }
  if (java_offset != nullptr) {
    env->SetLongField(java_offset, value_field, offset);
  }
  env->DeleteLocalRef(offset_class);
  return static_cast<jlong>(result);
}

jlong DarwinLinuxMmap(JNIEnv *env, jobject, jlong address, jlong byte_count,
                      jint prot, jint flags, jobject java_fd, jlong offset) {
  if (byte_count < 0 || offset < 0 ||
      static_cast<unsigned long long>(byte_count) >
          std::numeric_limits<size_t>::max()) {
    ThrowErrno(env, "mmap", EINVAL);
    return 0;
  }
  void *result = Mmap(reinterpret_cast<void *>(static_cast<uintptr_t>(address)),
                      static_cast<size_t>(byte_count), prot, flags,
                      jniGetFDFromFileDescriptor(env, java_fd),
                      static_cast<off_t>(offset));
  if (result == MAP_FAILED) {
    ThrowErrno(env, "mmap", errno);
    return 0;
  }
  return static_cast<jlong>(reinterpret_cast<uintptr_t>(result));
}

void DarwinLinuxMunmap(JNIEnv *env, jobject, jlong address, jlong byte_count) {
  if (byte_count < 0 ||
      Munmap(reinterpret_cast<void *>(static_cast<uintptr_t>(address)),
             static_cast<size_t>(byte_count)) == -1) {
    ThrowErrno(env, "munmap", byte_count < 0 ? EINVAL : errno);
  }
}

static bool ReadAndroidCredentialIds(
    DarwinArtProcessCredentialsOutput* output) {
  return darwin_art_bionic_process_state_read_credential_ids_core(output) == 0;
}

jint DarwinNativeGetegid() {
  DarwinArtProcessCredentialsOutput credentials{};
  return ReadAndroidCredentialIds(&credentials) ? static_cast<jint>(credentials.egid)
                                                : static_cast<jint>(getegid());
}
jint DarwinNativeGeteuid() {
  DarwinArtProcessCredentialsOutput credentials{};
  return ReadAndroidCredentialIds(&credentials) ? static_cast<jint>(credentials.euid)
                                                : static_cast<jint>(geteuid());
}
jint DarwinNativeGetgid() {
  DarwinArtProcessCredentialsOutput credentials{};
  return ReadAndroidCredentialIds(&credentials) ? static_cast<jint>(credentials.gid)
                                                : static_cast<jint>(getgid());
}
jint DarwinNativeGetpid() { return static_cast<jint>(getpid()); }
jint DarwinNativeGetppid() { return static_cast<jint>(getppid()); }
jint DarwinNativeGetuid() {
  DarwinArtProcessCredentialsOutput credentials{};
  return ReadAndroidCredentialIds(&credentials) ? static_cast<jint>(credentials.uid)
                                                : static_cast<jint>(getuid());
}
jint DarwinNativeGettid() {
  uint64_t thread_id = 0;
  if (pthread_threadid_np(nullptr, &thread_id) != 0 ||
      thread_id > static_cast<uint64_t>(std::numeric_limits<jint>::max())) {
    return -1;
  }
  return static_cast<jint>(thread_id);
}

// java.lang.ProcessEnvironment asks the native layer for the process
// environment during static initialization.  Android exposes this as a
// byte[][] (alternating key/value strings), not as a Java String map.  Keep
// the conversion at the JNI boundary so the guest sees the exact OpenJDK
// contract.  Variables the guest process snapshot defines (ANDROID_ROOT,
// ANDROID_DATA, ANDROID_STORAGE, EXTERNAL_STORAGE) carry their Android
// values, as Android's init sets them; the host's values for the same names
// locate ART's own host runtime files and stay visible only to native code.
// Other entries are the profile-scoped host environment.
using darwin_art::process::EnvironmentName;
using darwin_art::process::GuestEnvironmentEntry;

extern "C" jobjectArray Java_java_lang_ProcessEnvironment_environ(JNIEnv *env,
                                                                  jclass) {
  extern char **environ;
  std::vector<const char*> entries;
  for (char **entry = environ; entry != nullptr && *entry != nullptr; ++entry) {
    const char* guest = GuestEnvironmentEntry(EnvironmentName(*entry));
    entries.push_back(guest != nullptr ? guest : *entry);
  }
  char** guest_environ =
      &darwin_art_bionic_environ == nullptr ? nullptr : darwin_art_bionic_environ;
  for (char** guest = guest_environ; guest != nullptr && *guest != nullptr; ++guest) {
    const std::string_view name = EnvironmentName(*guest);
    bool present = false;
    for (char **entry = environ; entry != nullptr && *entry != nullptr; ++entry) {
      if (EnvironmentName(*entry) == name) {
        present = true;
        break;
      }
    }
    if (!present) entries.push_back(*guest);
  }
  jclass byte_array_class = env->FindClass("[B");
  if (byte_array_class == nullptr)
    return nullptr;
  if (entries.size() > static_cast<size_t>(std::numeric_limits<jsize>::max())) {
    env->DeleteLocalRef(byte_array_class);
    jniThrowException(env, "java/lang/OutOfMemoryError",
                      "environment too large");
    return nullptr;
  }
  jobjectArray result = env->NewObjectArray(static_cast<jsize>(entries.size()),
                                            byte_array_class, nullptr);
  env->DeleteLocalRef(byte_array_class);
  if (result == nullptr)
    return nullptr;
  jsize index = 0;
  for (const char* entry : entries) {
    const size_t length = std::strlen(entry);
    if (length > static_cast<size_t>(std::numeric_limits<jsize>::max())) {
      jniThrowException(env, "java/lang/OutOfMemoryError",
                        "environment entry too large");
      return nullptr;
    }
    jbyteArray bytes = env->NewByteArray(static_cast<jsize>(length));
    if (bytes == nullptr)
      return nullptr;
    env->SetByteArrayRegion(bytes, 0, static_cast<jsize>(length),
                            reinterpret_cast<const jbyte *>(entry));
    env->SetObjectArrayElement(result, index++, bytes);
    env->DeleteLocalRef(bytes);
    if (env->ExceptionCheck())
      return nullptr;
  }
  return result;
}

// Generated from the checksum-locked Android 16 upstream gMethods table.
#include "darwin_linux_method_table.inc"

#if defined(DARWIN_LIBCORE_LINUX_MANAGED_ABI_SMOKE)
JNINativeMethod kAbiSmokeMethods[] = {
    {const_cast<char *>("unsupportedVoid"),
     const_cast<char *>("(Ljava/lang/String;I)V"),
     reinterpret_cast<void *>(&AbiSmokeVoid)},
    {const_cast<char *>("unsupportedInt"),
     const_cast<char *>("(Ljava/io/FileDescriptor;II)I"),
     reinterpret_cast<void *>(&AbiSmokeInt)},
    {const_cast<char *>("unsupportedLong"),
     const_cast<char *>("(Ljava/io/FileDescriptor;)J"),
     reinterpret_cast<void *>(&AbiSmokeLong)},
    {const_cast<char *>("unsupportedObject"),
     const_cast<char *>("(Ljava/lang/String;)Ljava/lang/String;"),
     reinterpret_cast<void *>(&AbiSmokeObject)},
    {const_cast<char *>("unsupportedBoolean"),
     const_cast<char *>("(Ljava/lang/String;I)Z"),
     reinterpret_cast<void *>(&AbiSmokeBoolean)},
    {const_cast<char *>("availableProcessors"), const_cast<char *>("()J"),
     reinterpret_cast<void *>(&AbiSmokeAvailableProcessors)},
    {const_cast<char *>("environment"),
     const_cast<char *>("(Ljava/lang/String;)Ljava/lang/String;"),
     reinterpret_cast<void *>(&DarwinLinuxGetenv)},
    {const_cast<char *>("setEnvironment"),
     const_cast<char *>("(Ljava/lang/String;Ljava/lang/String;Z)V"),
     reinterpret_cast<void *>(&DarwinLinuxSetenv)},
    {const_cast<char *>("unsetEnvironment"),
     const_cast<char *>("(Ljava/lang/String;)V"),
     reinterpret_cast<void *>(&DarwinLinuxUnsetenv)},
    {const_cast<char *>("statPath"),
     const_cast<char *>("(Ljava/lang/String;)Landroid/system/StructStat;"),
     reinterpret_cast<void *>(&DarwinLinuxStat)},
    {const_cast<char *>("accessPath"),
     const_cast<char *>("(Ljava/lang/String;I)Z"),
     reinterpret_cast<void *>(&DarwinLinuxAccess)},
    {const_cast<char *>("writeFile"),
     const_cast<char *>("(Ljava/lang/String;[B)I"),
     reinterpret_cast<void *>(&AbiSmokeWriteFile)},
    {const_cast<char *>("unameView"),
     const_cast<char *>("()Landroid/system/StructUtsname;"),
     reinterpret_cast<void *>(&DarwinLinuxUname)},
    {const_cast<char *>("errorMessage"),
     const_cast<char *>("(I)Ljava/lang/String;"),
     reinterpret_cast<void *>(&AbiSmokeStrerror)},
    {const_cast<char *>("signalMessage"),
     const_cast<char *>("(I)Ljava/lang/String;"),
     reinterpret_cast<void *>(&AbiSmokeStrsignal)},
    {const_cast<char *>("openFile"),
     const_cast<char *>("(Ljava/lang/String;II)Ljava/io/FileDescriptor;"),
     reinterpret_cast<void *>(&DarwinLinuxOpen)},
    {const_cast<char *>("seekFile"),
     const_cast<char *>("(Ljava/io/FileDescriptor;JI)J"),
     reinterpret_cast<void *>(&DarwinLinuxLseek)},
    {const_cast<char *>("readFile"),
     const_cast<char *>("(Ljava/io/FileDescriptor;Ljava/lang/Object;II)I"),
     reinterpret_cast<void *>(&DarwinLinuxReadBytes)},
    {const_cast<char *>("closeFile"),
     const_cast<char *>("(Ljava/io/FileDescriptor;)V"),
     reinterpret_cast<void *>(&DarwinLinuxClose)},
    {const_cast<char *>("exchangeOwnerTag"),
     const_cast<char *>("(Ljava/io/FileDescriptor;JJ)V"),
     reinterpret_cast<void *>(&DarwinLinuxFdsanExchangeOwnerTag)},
    {const_cast<char *>("getOwnerTag"),
     const_cast<char *>("(Ljava/io/FileDescriptor;)J"),
     reinterpret_cast<void *>(&DarwinLinuxFdsanGetOwnerTag)},
    {const_cast<char *>("getTagType"),
     const_cast<char *>("(J)Ljava/lang/String;"),
     reinterpret_cast<void *>(&DarwinLinuxFdsanGetTagType)},
    {const_cast<char *>("getTagValue"), const_cast<char *>("(J)J"),
     reinterpret_cast<void *>(&DarwinLinuxFdsanGetTagValue)},
};
#endif

} // namespace

#if defined(DARWIN_LIBCORE_LINUX_MANAGED_ABI_SMOKE)
extern "C" JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM *vm, void *) {
  JNIEnv *env = nullptr;
  if (vm->GetEnv(reinterpret_cast<void **>(&env), JNI_VERSION_1_6) != JNI_OK) {
    return JNI_ERR;
  }
  jclass klass = env->FindClass("dev/darwinart/probe/LibcoreDarwinAbiSmoke");
  if (klass == nullptr ||
      env->RegisterNatives(klass, kAbiSmokeMethods,
                           static_cast<jint>(sizeof(kAbiSmokeMethods) /
                                             sizeof(kAbiSmokeMethods[0]))) !=
          JNI_OK) {
    env->DeleteLocalRef(klass);
    return JNI_ERR;
  }
  env->DeleteLocalRef(klass);
  return JNI_VERSION_1_6;
}
#endif

bool RegisterLinuxNatives(JNIEnv *env) {
  jclass klass = env->FindClass("libcore/io/Linux");
  if (klass == nullptr) {
    return false;
  }
  const jint method_count = static_cast<jint>(sizeof(kDarwinLinuxMethods) /
                                              sizeof(kDarwinLinuxMethods[0]));
  const bool registered =
      method_count == 135 &&
      env->RegisterNatives(klass, kDarwinLinuxMethods, method_count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return registered;
}

} // namespace darwin_art::libcore_darwin
