#include "stack.h"
#include "scoped_thread_state_change-inl.h"
#include "thread.h"

#include <dlfcn.h>
#include <string>

extern "C" bool aim_lookup_native_method(uintptr_t entrypoint,
                                                   void (*callback)(const char*, void*),
                                                   void* context);

extern "C" void aim_walk_managed_frames(void (*callback)(const char*, void*),
                                                 void* context) {
  if (callback == nullptr) return;
  art::Thread* self = art::Thread::Current();
  if (self == nullptr) return;
  // StackVisitor's AOSP contract requires a shared mutator-lock section. The
  // unwinder reaches this callback from a JNI/native frame, so establish the
  // ART runnable state explicitly before inspecting managed frames.
  art::ScopedObjectAccess soa(self);
  if (art::ArtMethod* current = self->GetCurrentMethod(
          nullptr, /*check_suspended=*/false, /*abort_on_error=*/false);
      current != nullptr) {
    std::string name;
    if (current->IsNative()) {
      using NativeLookup = bool (*)(uintptr_t, void (*)(const char*, void*), void*);
      auto lookup = reinterpret_cast<NativeLookup>(dlsym(RTLD_DEFAULT,
                                                          "aim_lookup_native_method"));
      if (lookup == nullptr) lookup = aim_lookup_native_method;
      lookup(
          reinterpret_cast<uintptr_t>(current->GetEntryPointFromJni()),
          [](const char* registered, void* target) {
            if (registered != nullptr) *static_cast<std::string*>(target) = registered;
          },
          &name);
      Dl_info info{};
      if (dladdr(current->GetEntryPointFromJni(), &info) != 0 && info.dli_sname != nullptr) {
        name = info.dli_sname[0] == '_' ? info.dli_sname + 1 : info.dli_sname;
      }
    }
    if (name.empty()) name = current->PrettyMethod(/*with_signature=*/false);
    callback(name.c_str(), context);
  }
  art::StackVisitor::WalkStack<art::StackVisitor::CountTransitions::kNo>(
      [callback, context](art::StackVisitor* visitor) {
        art::ArtMethod* method = visitor->GetMethod();
        if (method != nullptr) {
          std::string name = method->PrettyMethod(/*with_signature=*/false);
          callback(name.c_str(), context);
        }
        return true;
      },
      self,
      nullptr,
      art::StackVisitor::StackWalkKind::kIncludeInlinedFrames,
      /*check_suspended=*/true,
      /*include_transitions=*/true);
}
