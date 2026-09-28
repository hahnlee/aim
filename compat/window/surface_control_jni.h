#pragma once
#include <jni.h>
struct ASurfaceControl;

namespace aim::window {

// Registers only the framework SurfaceControl JNI contract.  The compositor
// objects and transactions remain owned by the Android surface provider; this
// module owns the Java marshalling and native method table.
bool RegisterSurfaceControlNatives(JNIEnv* env);

}  // namespace aim::window

// Returns an acquired reference to the Java SurfaceControl's existing owner.
extern "C" ASurfaceControl* ASurfaceControl_fromJava(JNIEnv*, jobject);
