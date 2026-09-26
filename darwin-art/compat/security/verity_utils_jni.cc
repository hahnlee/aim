#include "verity_utils_jni.h"

#include <cerrno>
#include <sys/stat.h>

#include "../darwin_libcore_filesystem_bridge.h"

// com_android_internal_security_VerityUtils.cpp semantics on a filesystem
// without fs-verity: Linux answers FS_IOC_ENABLE_VERITY and
// FS_IOC_MEASURE_VERITY with EOPNOTSUPP there, and a file never reports the
// verity attribute. Paths are guest paths resolved by the filesystem facade.
namespace darwin_art::security {
namespace {
// Android (Linux) errno values returned to Java.
constexpr jint kAndroidEinval = 22;
constexpr jint kAndroidEopnotsupp = 95;

jint AndroidErrno(int darwin_errno) {
  // ENOENT, EACCES, EBADF, ENOTDIR, EISDIR and the other low values match.
  return darwin_errno >= 1 && darwin_errno <= 34 ? darwin_errno : kAndroidEinval;
}

// Returns 0 when the guest path names an existing file, else an Android errno.
jint Exists(JNIEnv* env, jstring path) {
  if (path == nullptr) return kAndroidEinval;
  const char* text = env->GetStringUTFChars(path, nullptr);
  if (text == nullptr) return kAndroidEinval;
  struct stat status {};
  const int result = darwin_art_libcore_stat(text, &status);
  const int error = errno;
  env->ReleaseStringUTFChars(path, text);
  return result == 0 ? 0 : AndroidErrno(error);
}

jint EnableForFd(JNIEnv*, jclass, jint fd) {
  return fd < 0 ? 9 /* EBADF */ : kAndroidEopnotsupp;
}

jint Enable(JNIEnv* env, jclass, jstring path) {
  const jint missing = Exists(env, path);
  return missing != 0 ? missing : kAndroidEopnotsupp;
}

// 0 when the file lacks fs-verity, -errno on error.
jint Statx(JNIEnv* env, jclass, jstring path) {
  const jint missing = Exists(env, path);
  return missing != 0 ? -missing : 0;
}

jint Measure(JNIEnv* env, jclass, jstring path, jbyteArray) {
  const jint missing = Exists(env, path);
  return -(missing != 0 ? missing : kAndroidEopnotsupp);
}
}  // namespace

bool RegisterVerityUtilsNatives(JNIEnv* env) {
  jclass verity = env->FindClass("com/android/internal/security/VerityUtils");
  if (verity == nullptr) return false;
  const JNINativeMethod methods[] = {
      {const_cast<char*>("enableFsverityNative"), const_cast<char*>("(Ljava/lang/String;)I"),
       reinterpret_cast<void*>(Enable)},
      {const_cast<char*>("enableFsverityForFdNative"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(EnableForFd)},
      {const_cast<char*>("statxForFsverityNative"), const_cast<char*>("(Ljava/lang/String;)I"),
       reinterpret_cast<void*>(Statx)},
      {const_cast<char*>("measureFsverityNative"), const_cast<char*>("(Ljava/lang/String;[B)I"),
       reinterpret_cast<void*>(Measure)},
  };
  const bool registered = env->RegisterNatives(verity, methods, 4) == JNI_OK;
  env->DeleteLocalRef(verity);
  return registered;
}
}  // namespace darwin_art::security
