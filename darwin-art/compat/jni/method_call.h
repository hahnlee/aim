#pragma once

#include <cstdint>

#include <jni.h>

namespace darwin_art::jni {

// Invoke a previously resolved JNI method through ART's ...A entrypoints.
// invocation_kind is 0 for an instance call, 1 for a static call, and 2 for
// construction. For kind 0, receiver is a jobject; for kind 1 and kind 2 it
// is a jclass. A null args pointer is valid only for a method with no
// arguments. The caller must provide a live JNIEnv, receiver, and jmethodID,
// and must have established ART's normal JNI exception/thread preconditions.
// This function does not own or validate those objects and performs no
// policy, class-loader, logging, or exception-state handling.
//
// return_shorty is one of Z/B/C/S/I/J/F/D/L/V. Constructors require L and
// use NewObjectA. Null required pointers, invalid kinds or return shapes return
// zero without invoking ART.
uint64_t CallMethodA(JNIEnv* env,
                     jobject receiver,
                     jmethodID method,
                     const jvalue* args,
                     int32_t return_shorty,
                     int32_t invocation_kind);

}  // namespace darwin_art::jni
