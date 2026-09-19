#pragma once

#include <binder/IBinder.h>
#include <utils/RefBase.h>

#include "darwin_angle_egl.h"

namespace darwin_art::window {

// Creates the Browser-side capability. The endpoint retains the owner window
// and is the only object allowed to consume its remote-owner dequeue API.
android::sp<android::IBinder> CreateRemoteSurfaceProducerEndpoint(
    void* owner_native_window);

// Creates the GPU-side hooks consumed by
// darwin_art_android_ANativeWindow_create_remote(). The returned hook context
// owns each imported AHardwareBuffer until its matching queue/cancel call.
bool CreateRemoteSurfaceProducerClient(
    const android::sp<android::IBinder>& endpoint,
    DarwinArtRemoteNativeWindowHooks* out_hooks);

}  // namespace darwin_art::window
