#include "library_warning.h"
#include "namespace_handles.h"
#include "namespace_operation.h"
#include "process_namespaces.h"
#include "linker_dlwarning.h"
#include <cstdlib>

namespace darwin_art::loader {
void NamespaceHandles::AppendWarning(const char* path, const char* message, const char* value) {
  NamespaceOperation operation(configured_->registry());
  if (!operation || !path || !message) std::abort();
  add_dlwarning(path, message, value);
}
void NamespaceHandles::ConsumeWarnings(void* context, void (*callback)(void*, const char*)) {
  NamespaceOperation operation(configured_->registry());
  if (!operation || !callback) std::abort();
  get_dlwarning(context, callback);
}
void AppendLinkerWarning(const char* path, const char* message, const char* value) {
  auto owner = AcquireProcessNamespaces();
  if (!owner || !path || !message) std::abort();
  owner->AppendWarning(path, message, value);
}
}

extern "C" void __loader_android_dlwarning(void* context, void (*callback)(void*, const char*)) {
  auto owner = darwin_art::loader::AcquireProcessNamespaces();
  if (!owner || !callback) std::abort();
  // Original AOSP process-wide queue drains before invoking the callback.
  owner->ConsumeWarnings(context, callback);
}
