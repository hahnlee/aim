#pragma once
#include <jni.h>

namespace darwin_art::content {
// android.util.jar.StrictJarFile natives (frameworks/base
// core/jni/android_util_jar_StrictJarFile.cpp) over this process's Android
// descriptors.
bool RegisterStrictJarFileNatives(JNIEnv* env);
}
