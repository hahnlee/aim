#include "native_registration_audit.h"

#include <cstring>
#include <string>

#include "art_method-inl.h"
#include "entrypoints/runtime_asm_entrypoints.h"
#include "mirror/class-inl.h"
#include "scoped_thread_state_change-inl.h"

// ART's own record of JNI binding, for crates/aim-runtime
// native_coverage.rs: RegisterNatives replaces a native method's JNI entry
// point, which otherwise stays the dlsym lookup stub until its first call.
extern "C" size_t aim_class_native_registrations(JNIEnv* env, jclass klass, char* out,
                                                        size_t capacity) {
  std::string text;
  {
    ::art::ScopedObjectAccess soa(env);
    ::art::ObjPtr<::art::mirror::Class> declaring = soa.Decode<::art::mirror::Class>(klass);
    if (declaring == nullptr) return 0;
    for (::art::ArtMethod& method : declaring->GetDeclaredMethods(::art::kRuntimePointerSize)) {
      if (!method.IsNative()) continue;
      const void* entry = method.GetEntryPointFromJni();
      const bool bound = entry != ::art::GetJniDlsymLookupStub() &&
                         entry != ::art::GetJniDlsymLookupCriticalStub();
      text += method.GetName();
      text += '\t';
      text += method.GetSignature().ToString();
      text += '\t';
      text += bound ? 'R' : 'U';
      text += '\n';
    }
  }
  if (out != nullptr && capacity > text.size()) {
    std::memcpy(out, text.data(), text.size());
    out[text.size()] = '\0';
  }
  return text.size();
}
