#include "boot_native_libraries.h"

#include <dlfcn.h>
#include <limits.h>
#include <cstdlib>
#include <iostream>
#include <string>

#include "class_root-inl.h"
#include "jni/java_vm_ext.h"
#include "runtime.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"

namespace darwin_art::platform::art {
bool LoadOpenJdkBootLibrary(JNIEnv* env) {
  auto* runtime = ::art::Runtime::Current();
  auto* self = ::art::Thread::Current();
  if (env == nullptr || runtime == nullptr || self == nullptr ||
      self->GetState() != ::art::ThreadState::kNative) return false;
  Dl_info image{};
  const void* entry = reinterpret_cast<const void*>(
      reinterpret_cast<uintptr_t>(::art::Runtime::Current));
  char path[PATH_MAX];
  if (dladdr(entry, &image) == 0 || image.dli_fname == nullptr ||
      realpath(image.dli_fname, path) == nullptr) return false;
  const std::string image_path(path);
  const auto separator = image_path.find_last_of('/');
  if (separator == std::string::npos) return false;
  const std::string library = image_path.substr(0, separator) +
      "/libopenjdk-named-jni-owner.dylib";
  jclass calling_class;
  {
    ::art::ScopedObjectAccess soa(self);
    calling_class = reinterpret_cast<jclass>(runtime->GetJavaVM()->AddGlobalRef(
        self, ::art::GetClassRoot<::art::mirror::Object>(runtime->GetClassLinker())));
  }
  std::string error;
  const bool loaded = runtime->GetJavaVM()->LoadNativeLibrary(
      env, library, nullptr, calling_class, &error);
  env->DeleteGlobalRef(calling_class);
  if (!loaded) std::cerr << "ART OpenJDK boot library: " << error << "\n";
  return loaded && !env->ExceptionCheck();
}
}  // namespace darwin_art::platform::art
