#include "darwin_os_constants.h"

#include <errno.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

#include <nativehelper/ScopedUtfChars.h>

namespace {

jint CreateWithAndroidFlags(JNIEnv* env, jclass, jstring java_path,
                            jint android_flags) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr) {
    return -1;
  }
  int darwin_flags;
  if (!aim::os_constants::DarwinOpenFlagsFromAndroid(
          android_flags, &darwin_flags)) {
    return -1;
  }
  const int fd = open(path.c_str(), darwin_flags, 0600);
  if (fd == -1) {
    return -1;
  }
  return close(fd) == 0 ? 0 : -1;
}

jint StatMode(JNIEnv* env, jclass, jstring java_path) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr) {
    return -1;
  }
  struct stat status {};
  return stat(path.c_str(), &status) == 0 ? static_cast<jint>(status.st_mode)
                                         : -1;
}

jint MissingPathErrno(JNIEnv* env, jclass, jstring java_path) {
  ScopedUtfChars path(env, java_path);
  if (path.c_str() == nullptr) {
    return -1;
  }
  const int fd = open(path.c_str(), O_RDONLY);
  if (fd != -1) {
    close(fd);
    return 0;
  }
  int android_errno;
  return aim::os_constants::AndroidErrnoFromDarwin(errno,
                                                           &android_errno)
             ? android_errno
             : -1;
}

jint AndroidNotSupportedErrno(JNIEnv*, jclass) {
  int android_errno;
  return aim::os_constants::AndroidErrnoFromDarwin(ENOTSUP,
                                                           &android_errno)
             ? android_errno
             : -1;
}

jlong ProcessorCount(JNIEnv*, jclass, jint android_name) {
  int darwin_name;
  if (!aim::os_constants::DarwinSysconfNameFromAndroid(android_name,
                                                              &darwin_name)) {
    return -1;
  }
  return static_cast<jlong>(sysconf(darwin_name));
}

JNINativeMethod kProbeMethods[] = {
    {const_cast<char*>("createWithAndroidFlags"),
     const_cast<char*>("(Ljava/lang/String;I)I"),
     reinterpret_cast<void*>(&CreateWithAndroidFlags)},
    {const_cast<char*>("statMode"),
     const_cast<char*>("(Ljava/lang/String;)I"),
     reinterpret_cast<void*>(&StatMode)},
    {const_cast<char*>("missingPathErrno"),
     const_cast<char*>("(Ljava/lang/String;)I"),
     reinterpret_cast<void*>(&MissingPathErrno)},
    {const_cast<char*>("androidNotSupportedErrno"), const_cast<char*>("()I"),
     reinterpret_cast<void*>(&AndroidNotSupportedErrno)},
    {const_cast<char*>("processorCount"), const_cast<char*>("(I)J"),
     reinterpret_cast<void*>(&ProcessorCount)},
};

}  // namespace

extern "C" JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void*) {
  JNIEnv* env = nullptr;
  if (vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) != JNI_OK) {
    return JNI_ERR;
  }
  register_android_system_OsConstants(env);
  if (env->ExceptionCheck()) {
    return JNI_ERR;
  }
  jclass probe = env->FindClass("dev/aim/probe/OsConstantsProbe");
  if (probe == nullptr ||
      env->RegisterNatives(
          probe, kProbeMethods,
          static_cast<jint>(sizeof(kProbeMethods) /
                            sizeof(kProbeMethods[0]))) != JNI_OK) {
    env->DeleteLocalRef(probe);
    return JNI_ERR;
  }
  env->DeleteLocalRef(probe);
  return JNI_VERSION_1_6;
}
