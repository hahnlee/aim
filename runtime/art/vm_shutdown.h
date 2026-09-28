#pragma once

#include <jni.h>

namespace aim::runtime_art {

// Detach the owner thread and destroy the ART VM after framework/application
// state has been quiesced. Returns false when either VM operation fails.
bool DetachAndDestroyVm(JavaVM* java_vm);

}  // namespace aim::runtime_art
