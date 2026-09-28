#pragma once

#include <jni.h>

namespace aim::framework::wm {
// WMS owns publication; InputChannel supplies a server-only resource lease.
// This registrar installs only the WMS publication boundary, not channel JNI.
bool RegisterWindowInputPublisherNatives(JNIEnv* env);
}
