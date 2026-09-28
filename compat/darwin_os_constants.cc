#include "darwin_os_constants.h"

#include <errno.h>
#include <fcntl.h>
#include <unistd.h>

#include <cstddef>


namespace {

struct AndroidConstant {
  const char* name;
  int value;
};

constexpr AndroidConstant kAndroidConstants[] = {
#include "android16_os_constants_values.inc"
};

void OsConstantsInitConstants(JNIEnv* env, jclass constants_class) {
  for (const AndroidConstant& constant : kAndroidConstants) {
    jfieldID field =
        env->GetStaticFieldID(constants_class, constant.name, "I");
    if (field == nullptr) {
      return;
    }
    env->SetStaticIntField(constants_class, field, constant.value);
    if (env->ExceptionCheck()) {
      return;
    }
  }
}

JNINativeMethod kOsConstantsMethods[] = {
    {const_cast<char*>("initConstants"), const_cast<char*>("()V"),
     reinterpret_cast<void*>(&OsConstantsInitConstants)},
};

}  // namespace

namespace aim::os_constants {

bool DarwinOpenFlagsFromAndroid(int android_flags, int* darwin_flags) {
  if (darwin_flags == nullptr) {
    return false;
  }

  // Linux arm64 UAPI values. These intentionally differ from both Darwin and
  // asm-generic Linux for O_DIRECTORY/O_DIRECT/O_LARGEFILE.
  constexpr int kAndroidOAccmode = 3;
  constexpr int kAndroidOCreat = 64;
  constexpr int kAndroidOExcl = 128;
  constexpr int kAndroidONoctty = 256;
  constexpr int kAndroidOTrunc = 512;
  constexpr int kAndroidOAppend = 1024;
  constexpr int kAndroidONonblock = 2048;
  constexpr int kAndroidODsync = 4096;
  constexpr int kAndroidODirectory = 16384;
  constexpr int kAndroidONofollow = 32768;
  constexpr int kAndroidODirect = 65536;
  constexpr int kAndroidOLargefile = 131072;
  constexpr int kAndroidOCloexec = 524288;
  constexpr int kAndroidOSync = 1052672;
  constexpr int kAndroidOPath = 2097152;
  constexpr int kAndroidOTmpfile = 4210688;

  int remaining = android_flags;
  int translated = 0;
  switch (remaining & kAndroidOAccmode) {
    case 0:
      translated |= O_RDONLY;
      break;
    case 1:
      translated |= O_WRONLY;
      break;
    case 2:
      translated |= O_RDWR;
      break;
    default:
      return false;
  }
  remaining &= ~kAndroidOAccmode;

  // Reject Linux-only semantic modes before consuming overlapping bits.
  if ((remaining & kAndroidOTmpfile) == kAndroidOTmpfile ||
      (remaining & (kAndroidODirect | kAndroidOPath)) != 0) {
    return false;
  }
  if ((remaining & kAndroidOSync) == kAndroidOSync) {
    translated |= O_SYNC;
    remaining &= ~kAndroidOSync;
  }

#define TRANSLATE_FLAG(android_flag, darwin_flag) \
  if ((remaining & (android_flag)) != 0) {          \
    translated |= (darwin_flag);                    \
    remaining &= ~(android_flag);                   \
  }
  TRANSLATE_FLAG(kAndroidOCreat, O_CREAT)
  TRANSLATE_FLAG(kAndroidOExcl, O_EXCL)
  TRANSLATE_FLAG(kAndroidONoctty, O_NOCTTY)
  TRANSLATE_FLAG(kAndroidOTrunc, O_TRUNC)
  TRANSLATE_FLAG(kAndroidOAppend, O_APPEND)
  TRANSLATE_FLAG(kAndroidONonblock, O_NONBLOCK)
#ifdef O_DSYNC
  TRANSLATE_FLAG(kAndroidODsync, O_DSYNC)
#else
  TRANSLATE_FLAG(kAndroidODsync, O_SYNC)
#endif
#ifdef O_DIRECTORY
  TRANSLATE_FLAG(kAndroidODirectory, O_DIRECTORY)
#endif
#ifdef O_NOFOLLOW
  TRANSLATE_FLAG(kAndroidONofollow, O_NOFOLLOW)
#endif
#ifdef O_CLOEXEC
  TRANSLATE_FLAG(kAndroidOCloexec, O_CLOEXEC)
#endif
#undef TRANSLATE_FLAG

  // O_LARGEFILE is a no-op for a 64-bit Darwin process.
  remaining &= ~kAndroidOLargefile;
  if (remaining != 0) {
    return false;
  }
  *darwin_flags = translated;
  return true;
}

bool DarwinSysconfNameFromAndroid(int android_name, int* darwin_name) {
  if (darwin_name == nullptr) {
    return false;
  }
#include "android16_os_constants_sysconf.inc"
  return false;
}

bool AndroidErrnoFromDarwin(int darwin_errno, int* android_errno) {
  if (android_errno == nullptr) {
    return false;
  }
#include "android16_os_constants_errno.inc"
  return false;
}

}  // namespace aim::os_constants

void register_android_system_OsConstants(JNIEnv* env) {
  jclass klass = env->FindClass("android/system/OsConstants");
  if (klass == nullptr || env->ExceptionCheck()) return;
  env->RegisterNatives(
      klass, kOsConstantsMethods,
      static_cast<jint>(sizeof(kOsConstantsMethods) /
                        sizeof(kOsConstantsMethods[0])));
  env->DeleteLocalRef(klass);
}
