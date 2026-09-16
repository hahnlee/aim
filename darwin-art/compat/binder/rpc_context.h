#pragma once

#include <binder/IBinder.h>

namespace darwin_art::binder {

// AOSP Binder RPC owns Parcel/object/FD transport. The path and JVM thread
// attachment are the only Darwin boundaries in this API.
android::sp<android::IBinder> ConnectRpcContext(const char* path);

}  // namespace darwin_art::binder
