#include "libcore_darwin_linux.h"

#include "darwin_os_constants.h"

#include <pwd.h>
#include <sys/utsname.h>
#include <unistd.h>

#include <array>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

#include <nativehelper/JNIHelp.h>
#include <nativehelper/ScopedUtfChars.h>

namespace darwin_art::libcore_darwin {
namespace {

jobject MakeStructPasswd(JNIEnv* env, const struct passwd& password) {
  jclass klass = env->FindClass("android/system/StructPasswd");
  jmethodID constructor =
      klass == nullptr
          ? nullptr
          : env->GetMethodID(
                klass, "<init>",
                "(Ljava/lang/String;IILjava/lang/String;Ljava/lang/String;)V");
  if (constructor == nullptr) {
    env->DeleteLocalRef(klass);
    return nullptr;
  }
  jstring name = env->NewStringUTF(password.pw_name);
  jstring directory = env->NewStringUTF(password.pw_dir);
  jstring shell = env->NewStringUTF(password.pw_shell);
  jobject result =
      name == nullptr || directory == nullptr || shell == nullptr
          ? nullptr
          : env->NewObject(klass, constructor, name,
                           static_cast<jint>(password.pw_uid),
                           static_cast<jint>(password.pw_gid), directory, shell);
  env->DeleteLocalRef(name);
  env->DeleteLocalRef(directory);
  env->DeleteLocalRef(shell);
  env->DeleteLocalRef(klass);
  return result;
}

jobject MakeStructUtsname(JNIEnv* env, const struct utsname& host) {
  jclass klass = env->FindClass("android/system/StructUtsname");
  jmethodID constructor =
      klass == nullptr
          ? nullptr
          : env->GetMethodID(
                klass, "<init>",
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;"
                "Ljava/lang/String;Ljava/lang/String;)V");
  if (constructor == nullptr) {
    env->DeleteLocalRef(klass);
    return nullptr;
  }
  // The compatibility environment exposes Android's kernel/ABI identity.
  // Host release/version remain available for diagnostics without making
  // framework ABI selection believe it is running on Darwin/x86_64.
  jstring sysname = env->NewStringUTF("Linux");
  jstring nodename = env->NewStringUTF(host.nodename);
  jstring release = env->NewStringUTF(host.release);
  jstring version = env->NewStringUTF(host.version);
  jstring machine = env->NewStringUTF("aarch64");
  jobject result =
      sysname == nullptr || nodename == nullptr || release == nullptr ||
              version == nullptr || machine == nullptr
          ? nullptr
          : env->NewObject(klass, constructor, sysname, nodename, release,
                           version, machine);
  env->DeleteLocalRef(sysname);
  env->DeleteLocalRef(nodename);
  env->DeleteLocalRef(release);
  env->DeleteLocalRef(version);
  env->DeleteLocalRef(klass);
  return result;
}

}  // namespace

jlong DarwinLinuxSysconf(JNIEnv* env, jobject, jint name) {
  // POSIX permits -1 with errno left at zero for an indeterminate value.
  errno = 0;
  const long result = Sysconf(name);
  if (result == -1 && errno == EINVAL) {
    ThrowErrno(env, "sysconf", errno);
  }
  return static_cast<jlong>(result);
}

jstring DarwinLinuxGetenv(JNIEnv* env, jobject, jstring java_name) {
  ScopedUtfChars name(env, java_name);
  if (name.c_str() == nullptr) {
    return nullptr;
  }
  const char* value = std::getenv(name.c_str());
  return value == nullptr ? nullptr : env->NewStringUTF(value);
}

void DarwinLinuxSetenv(JNIEnv* env, jobject, jstring java_name,
                       jstring java_value, jboolean overwrite) {
  ScopedUtfChars name(env, java_name);
  if (name.c_str() == nullptr) {
    return;
  }
  ScopedUtfChars value(env, java_value);
  if (value.c_str() == nullptr) {
    return;
  }
  if (::setenv(name.c_str(), value.c_str(), overwrite) == -1) {
    ThrowErrno(env, "setenv", errno);
  }
}

void DarwinLinuxUnsetenv(JNIEnv* env, jobject, jstring java_name) {
  ScopedUtfChars name(env, java_name);
  if (name.c_str() == nullptr) {
    return;
  }
  if (::unsetenv(name.c_str()) == -1) {
    ThrowErrno(env, "unsetenv", errno);
  }
}

jobject DarwinLinuxGetpwuid(JNIEnv* env, jobject, jint uid) {
  const long configured_size = sysconf(_SC_GETPW_R_SIZE_MAX);
  const size_t buffer_size = configured_size > 0
                                 ? static_cast<size_t>(configured_size)
                                 : 1024u;
  std::vector<char> buffer(buffer_size);
  struct passwd password {};
  struct passwd* result = nullptr;
  const int error = getpwuid_r(static_cast<uid_t>(uid), &password,
                               buffer.data(), buffer.size(), &result);
  if (result == nullptr) {
    ThrowErrno(env, "getpwuid_r", error);
    return nullptr;
  }
  return MakeStructPasswd(env, password);
}

jobject DarwinLinuxUname(JNIEnv* env, jobject) {
  struct utsname host {};
  int result;
  do {
    result = uname(&host);
  } while (result == -1 && errno == EINTR);
  if (result == -1) {
    // Matches upstream: uname failure is treated as impossible and returns
    // null without manufacturing an ErrnoException.
    return nullptr;
  }
  return MakeStructUtsname(env, host);
}

jstring DarwinLinuxStrerror(JNIEnv* env, jobject, jint error_number) {
  // Darwin exposes the XSI/POSIX int-returning strerror_r, unlike the
  // pointer-returning GNU/Bionic interface used by the upstream default path.
  // A private buffer preserves strerror's thread-safety across NewStringUTF.
  std::array<char, BUFSIZ> buffer {};
  const int result = strerror_r(error_number, buffer.data(), buffer.size());
  if (result != 0 && buffer[0] == '\0') {
    // Match upstream's POSIX strerror_r fallback: strerror never converts an
    // unknown errno into an exception or a null Java result.
    std::snprintf(buffer.data(), buffer.size(), "Unknown error %d",
                  error_number);
  }
  return env->NewStringUTF(buffer.data());
}

jstring DarwinLinuxStrsignal(JNIEnv* env, jobject, jint signal_number) {
  std::array<char, BUFSIZ> buffer {};
  if (strsignal_r(signal_number, buffer.data(), buffer.size()) == 0) {
    return env->NewStringUTF(buffer.data());
  }
  // On Darwin strsignal storage is unique to the calling thread. This keeps
  // the upstream libc result for invalid signals without inventing a message.
  const char* message = strsignal(signal_number);
  return message == nullptr ? nullptr : env->NewStringUTF(message);
}

}  // namespace darwin_art::libcore_darwin
