#pragma once

#include <jni.h>

namespace darwin_art_headless_fixture {

// Installs only the fake Paint/RenderNode JNI tables used by the headless
// fixture. Product runtime registration deliberately does not call this.
bool RegisterGraphicsNatives(JNIEnv* env);

}  // namespace darwin_art_headless_fixture
