#pragma once

#include <jni.h>

namespace aim::framework::display {

bool RegisterHostDisplayFacts(JNIEnv* env, jclass facts_class);

}  // namespace aim::framework::display
