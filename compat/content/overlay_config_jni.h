#pragma once
#include <jni.h>

namespace aim::content {
// com.android.internal.content.om.OverlayConfig.createIdmap over the idmaps
// image assembly created for the immutable framework overlays.
bool RegisterOverlayConfigNatives(JNIEnv* env);
}
