#pragma once

#include <jni.h>

#include <cstdint>
#include <string>

#include "../embedding/process_config.h"

namespace art {
class Thread;
}

namespace darwin_art::runtime_art {

// The result of Android's app_process-equivalent VM creation.  Ownership of
// the VM remains with the embedding process state; this value only exposes the
// owner thread and JNI environment needed by the next Android lifecycle stage.
struct VmBootstrapResult final {
  art::Thread* self = nullptr;
  JNIEnv* env = nullptr;
  bool jit_enabled = false;
};

// Creates ART with the checked embedding configuration and the ordinary
// Android application/system class path.  Fixture switches (ELF probes,
// direct activities and upstream test harnesses) intentionally do not appear
// in this production API; those belong to probes and use their own launcher.
int CreateVm(const darwin_art_process_config_t* config,
             const embedding::ProcessConfigBounds& bounds,
             const std::string& application_class_path,
             VmBootstrapResult* result);

}  // namespace darwin_art::runtime_art
