#pragma once
#include <jni.h>
struct ASurfaceControl;
// Returns an acquired reference to the Java SurfaceControl's existing owner.
extern "C" ASurfaceControl* ASurfaceControl_fromJava(JNIEnv*, jobject);
