#pragma once

#include <jni.h>

#include <cstddef>

// The native methods a class declares and whether each is bound: one
// "NAME\tSIGNATURE\tSTATE\n" line per method, STATE 'R' when its JNI entry
// point is bound (RegisterNatives or an earlier by-name lookup) and 'U' while
// it still points at ART's dlsym lookup stub. Returns the text length; the
// text and its NUL are written only when `capacity` exceeds it.
extern "C" size_t darwin_art_class_native_registrations(JNIEnv* env, jclass klass, char* out,
                                                        size_t capacity);
