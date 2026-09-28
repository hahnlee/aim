#pragma once

#include <jni.h>

namespace aim::framework::os {

// Installs the process-independent client side used to reach Android's service
// directory. Application and system processes own this registration.
bool RegisterSystemServiceClientTransport(JNIEnv* env);

// Registers the service-process spawn/release methods on the caller's endpoint
// in addition to the shared client transport.
bool RegisterServiceProcessTransport(JNIEnv* env, jclass endpoint);

}  // namespace aim::framework::os
