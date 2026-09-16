#pragma once

#include <jni.h>

namespace darwin_art::framework::app {

// Writes the pending Java exception and its managed stack to stderr while
// preserving that exception for the process boundary.
void ReportPendingJavaException(JNIEnv* env);

}
