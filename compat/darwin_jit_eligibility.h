#pragma once

#include "art_method-inl.h"
#include "class_linker-inl.h"
#include "compilation_kind.h"
#include "dex/code_item_accessors-inl.h"
#include "dex/dex_instruction-inl.h"
#include "mirror/class-inl.h"
#include "runtime.h"

namespace art::jit {

inline ArtMethod* DarwinJitLookupResolvedMethod(ArtMethod* referrer, uint32_t index)
    REQUIRES_SHARED(Locks::mutator_lock_) {
  auto* cached = referrer->GetDexCache()->GetResolvedMethod(index);
  if (cached != nullptr) return cached;
  auto* linker = Runtime::Current()->GetClassLinker();
  auto type = linker->LookupResolvedType(
      referrer->GetDexFile()->GetMethodId(index).class_idx_, referrer);
  if (type == nullptr) return nullptr;
  return linker->FindResolvedMethod(type, referrer->GetDexCache(),
                                    referrer->GetClassLoader(), index);
}

// Darwin uses the same method-level admission contract as ART's JIT. Backend
// correctness belongs in the ARM64 lowering and runtime ABI, not in a DEX
// opcode or method-shape allowlist. Keep only lifecycle states for which AOSP
// itself cannot produce an installable JIT method.
inline bool DarwinJitCanCompile(ArtMethod* method, CompilationKind kind)
    REQUIRES_SHARED(Locks::mutator_lock_) {
  (void)kind;
  Runtime* runtime = Runtime::Current();
  if (runtime->IsZygote() || runtime->IsAotCompiler() ||
      runtime->IsJavaDebuggable() || method->IsRuntimeMethod() ||
      method->IsNative() || method->IsProxyMethod() || method->IsObsolete() ||
      !method->IsInvokable() || !method->IsCompilable() ||
      method->GetCodeItem() == nullptr) {
    return false;
  }
  auto klass = method->GetDeclaringClass();
  return klass->IsVerified() && klass->IsVisiblyInitialized() &&
         !klass->IsObsoleteObject() && !method->StillNeedsClinitCheck();
}

}  // namespace art::jit
