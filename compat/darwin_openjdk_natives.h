#pragma once

#include <jni.h>

// Defined by Android 16 libcore's unmodified ojluni/src/main/native/Math.c.
// This declaration is the only Darwin bridge: the implementation and full
// JNINativeMethod table remain owned by upstream libcore.
extern "C" void register_java_lang_Math(JNIEnv* env);
