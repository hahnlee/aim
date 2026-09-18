#pragma once
#include <jni.h>
struct ASurfaceControl;

namespace darwin_art::window {

// Registers only the framework SurfaceControl JNI contract.  The compositor
// objects and transactions remain owned by the Android surface provider; this
// module owns the Java marshalling and native method table.
bool RegisterSurfaceControlNatives(JNIEnv* env);

}  // namespace darwin_art::window

// Returns an acquired reference to the Java SurfaceControl's existing owner.
extern "C" ASurfaceControl* ASurfaceControl_fromJava(JNIEnv*, jobject);
