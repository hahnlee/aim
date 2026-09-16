// The framework's original android_app_Activity.cpp is a Mach-O translation
// unit, so it cannot bind directly to the public symbol in the guest ELF
// libdl_android.so image. Keep the Darwin ABI crossing at the loader boundary:
// both sides consume the same original Bionic process-wide warning queue.

#include "linker_dlwarning.h"

extern "C" void android_dlwarning(
    void* context, void (*callback)(void*, const char*)) {
  get_dlwarning(context, callback);
}
