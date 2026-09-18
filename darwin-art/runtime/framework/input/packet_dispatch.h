#pragma once

#include <jni.h>

#include "../../../compat/darwin_framework_input_hint.h"
#include "framework_dispatch.h"
#include "view_root_input_jni.h"

namespace darwin_art::input {

// Constructs an Android event and hands it to the admitted original
// InputReceiver. It never looks up ViewRootImpl.mInputEventReceiver or a
// mutable receiver ID.
FrameworkInputEventDispatchResult DispatchInputPacketToReceiver(
    JNIEnv* env, const OriginalInputReceiverDispatchContext& context,
    const darwin_art::DarwinArtInputPacket& packet);

}  // namespace darwin_art::input
