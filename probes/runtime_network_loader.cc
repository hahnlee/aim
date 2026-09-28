#include <iostream>
#include <string>

#include "runtime_network_probe.h"
#include "jni/java_vm_ext.h"
#include "runtime.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"

extern "C" int aim_network_load_fixture(JNIEnv* env,
                                                  const char* fixture_path,
                                                  jobject app_loader,
                                                  jclass fixture_class) {
  art::Thread* self = art::Thread::Current();
  if (self == nullptr) {
    std::cerr << "ART Android network: no owner thread\n";
    return 47;
  }
  std::string load_error;
  bool loaded = false;
  {
    art::ScopedThreadSuspension suspended(self, art::ThreadState::kNative);
    loaded = art::Runtime::Current()->GetJavaVM()->LoadNativeLibrary(
        env, fixture_path, app_loader, fixture_class, &load_error);
  }
  if (!loaded || !load_error.empty() || env->ExceptionCheck()) {
    std::cerr << "ART Android network JavaVMExt load failed: " << load_error
              << "\n";
    return 47;
  }
  return 0;
}
