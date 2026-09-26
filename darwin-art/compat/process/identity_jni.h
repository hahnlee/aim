#pragma once
#include <jni.h>

namespace darwin_art::process {
// android.os.Process.getGidForName/getUidForName over bionic's Android
// user/group name space.
jint GetGidForName(JNIEnv* env, jclass, jstring name);
jint GetUidForName(JNIEnv* env, jclass, jstring name);
}
