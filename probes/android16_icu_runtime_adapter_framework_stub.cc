#include <jni.h>

namespace aim {

// The ICU adapter smoke test links the libcore registrar in isolation. The
// production framework registrar is supplied by the graphics bootstrap; this
// seam keeps the standalone closure honest without pulling HWUI into the ICU
// source gate.
bool RegisterFrameworkSupportNatives(JNIEnv*) { return true; }

}  // namespace aim
