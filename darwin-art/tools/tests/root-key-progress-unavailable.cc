// Test-only linker port for RootKeyAuthority component tests.
//
// These tests intentionally do not install a ReusableLooperTask.  Keep the
// production authority's dependency explicit while making accidental progress
// use fail closed instead of silently reporting a successful wake.
#include "compat/looper/android_looper_owner.h"

#include <cstdlib>

namespace darwin_art::looper {

bool ReusableLooperTask::Request() {
  std::abort();
}

}  // namespace darwin_art::looper
