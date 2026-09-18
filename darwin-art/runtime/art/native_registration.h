#pragma once

#include <jni.h>

namespace art {
class Thread;
}

namespace darwin_art::runtime_art {

// Establishes Android's boot/libcore/framework JNI registration order for an
// embedded ART process. Test-only regressions are outside this owner.
int StartNativeRegistration(JNIEnv* env, art::Thread* self);

}  // namespace darwin_art::runtime_art
