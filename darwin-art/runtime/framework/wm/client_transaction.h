#pragma once

#include <jni.h>

namespace darwin_art::framework::wm {
// System-side transport boundary for original framework transaction items.
// The task/window owner supplies ordered items and their tokens/configuration.
// No lifecycle callbacks, context construction or state synthesis happen here.
// References are borrowed; temporary transaction locals end before returning.
// Success means scheduled, NOT executed or rendered by the application.
bool ScheduleClientTransaction(JNIEnv* env, jobject application_thread,
                               jobjectArray items);
}
