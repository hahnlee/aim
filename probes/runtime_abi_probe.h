#pragma once

#include <jni.h>

namespace aim_abi_probe {

jlong packed_integer_stack(JNIEnv*, jclass, jint, jint, jint, jint, jint,
                            jint, jint, jlong, jint, jint);
jlong packed_floating_stack(JNIEnv*, jclass, jfloat, jfloat, jfloat, jfloat,
                            jfloat, jfloat, jfloat, jfloat, jfloat, jdouble);
jlong packed_reference_stack(JNIEnv*, jclass, jint, jint, jint, jint, jint,
                             jint, jint, jobject, jint);
jlong packed_narrow_stack(JNIEnv*, jclass, jint, jint, jint, jint, jint, jint,
                          jboolean, jbyte, jchar, jshort, jint, jlong);

}  // namespace aim_abi_probe
