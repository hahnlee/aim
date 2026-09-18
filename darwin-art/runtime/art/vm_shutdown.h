#pragma once

#include <jni.h>

namespace darwin_art::runtime_art {

// Detach the owner thread and destroy the ART VM after framework/application
// state has been quiesced. Returns false when either VM operation fails.
bool DetachAndDestroyVm(JavaVM* java_vm);

}  // namespace darwin_art::runtime_art
