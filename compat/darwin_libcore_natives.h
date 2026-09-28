#ifndef AIM_COMPAT_DARWIN_LIBCORE_NATIVES_H_
#define AIM_COMPAT_DARWIN_LIBCORE_NATIVES_H_

#include <jni.h>

namespace aim {

bool RegisterLibcoreNatives(JNIEnv* env);
// Installs the bootstrap-only System.log entry before VMClassLoader can
// initialize java.nio and report diagnostics through that native method.
bool RegisterEarlySystemLog(JNIEnv* env);
// Complete Android java.lang.Runtime + sun.nio.fs.UnixNativeDispatcher tables.
// The real-graphics owner installs these after Math and before app code.
bool RegisterManagedLoadNatives(JNIEnv* env);
bool RegisterLibcoreCharacterNatives(JNIEnv* env);
bool RegisterLibcoreIcuNatives(JNIEnv* env);
bool ShutdownLibcoreNatives();

}  // namespace aim

#endif  // AIM_COMPAT_DARWIN_LIBCORE_NATIVES_H_
