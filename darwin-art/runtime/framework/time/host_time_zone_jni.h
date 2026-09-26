#pragma once

#include <jni.h>

namespace darwin_art::framework::time {

bool RegisterHostTimeZoneProvider(JNIEnv* env, jclass provider_class);

}  // namespace darwin_art::framework::time
