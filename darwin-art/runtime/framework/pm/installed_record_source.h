#pragma once
#include <jni.h>

namespace darwin_art::framework::pm {
// Bootstrap supplies its profile socket capability; this owner never reads env.
// Null means an explicitly missing package. Transport failure leaves an exception.
jstring QueryInstalledRecord(JNIEnv* env, const char* profile_socket, jstring package);
}
