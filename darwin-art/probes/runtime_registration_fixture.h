#pragma once

#include <jni.h>

namespace darwin_art_graphics {
struct GraphicsState;
}

namespace art {
class Thread;
}

namespace darwin_art_registration_fixture {

struct Inputs {
  JNIEnv* env;
  art::Thread* self;
  jobject app_loader_ref;
  jclass probe_canvas_class;
  bool headless_fixture;
  darwin_art_graphics::GraphicsState* graphics_state;
};

// Runs opt-in fixture regressions after production native registration.
int run_checks(JNIEnv* env);

// Installs application-loader-owned state after the APK/support DEX classes
// have been loaded. Returns the process-probe status code (0 success).
int finish(const Inputs& inputs);

}  // namespace darwin_art_registration_fixture
