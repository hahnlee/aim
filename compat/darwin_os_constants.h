#ifndef AIM_DARWIN_OS_CONSTANTS_H_
#define AIM_DARWIN_OS_CONSTANTS_H_

#include <jni.h>

namespace aim::os_constants {

// Translate Java-visible Android/Linux ABI values at the Darwin syscall edge.
bool DarwinOpenFlagsFromAndroid(int android_flags, int* darwin_flags);
bool DarwinSysconfNameFromAndroid(int android_name, int* darwin_name);
bool AndroidErrnoFromDarwin(int darwin_errno, int* android_errno);

}  // namespace aim::os_constants

// Exact Android 16 android.system.OsConstants registrar owner.
void register_android_system_OsConstants(JNIEnv* env);

#endif  // AIM_DARWIN_OS_CONSTANTS_H_
