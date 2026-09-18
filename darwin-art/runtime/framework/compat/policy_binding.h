#pragma once
#include <jni.h>

namespace darwin_art::framework::compat {
bool InitializeSystemPolicy(JNIEnv* env, jobject system_context);
struct ApplicationChanges {
  jlongArray disabled = nullptr;
  jlongArray loggable = nullptr;
};
// Returned arrays are local references owned by the caller's JNI frame. The
// application must come from AMS's authenticated installed-package resolution.
bool EvaluateApplicationChanges(JNIEnv* env, jobject application,
                                ApplicationChanges* result);
}
