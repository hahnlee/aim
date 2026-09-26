#pragma once
#include <jni.h>

namespace darwin_art::content {
// com.android.internal.content.om.OverlayConfig.createIdmap over the idmaps
// image assembly created for the immutable framework overlays.
bool RegisterOverlayConfigNatives(JNIEnv* env);
}
