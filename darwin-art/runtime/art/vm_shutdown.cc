#include "vm_shutdown.h"

#include <iostream>

namespace darwin_art::runtime_art {

bool DetachAndDestroyVm(JavaVM* java_vm) {
  if (java_vm == nullptr || java_vm->DetachCurrentThread() != JNI_OK) {
    return false;
  }
  std::cerr << "ART Darwin shutdown stage=detach complete\n";
  if (java_vm->DestroyJavaVM() != JNI_OK) {
    return false;
  }
  std::cerr << "ART Darwin shutdown stage=destroy-vm complete\n";
  return true;
}

}  // namespace darwin_art::runtime_art
