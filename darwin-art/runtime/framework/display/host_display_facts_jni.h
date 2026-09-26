#pragma once

#include <jni.h>

namespace darwin_art::framework::display {

bool RegisterHostDisplayFacts(JNIEnv* env, jclass facts_class);

}  // namespace darwin_art::framework::display
