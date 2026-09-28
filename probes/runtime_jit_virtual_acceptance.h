#pragma once

#include "oat/oat_quick_method_header.h"
#include "jit/profiling_info.h"
#include "oat/stack_map.h"

namespace aim_jni_acceptance_phase {
inline bool CheckJitVirtual(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                            art::Handle<art::mirror::Class> owner, jclass java_owner,
                            bool baseline = false) {
  jmethodID base_class_id = env->GetStaticMethodID(java_owner, "jitVirtualBaseClass", "()Ljava/lang/Class;");
  jmethodID child_class_id = env->GetStaticMethodID(java_owner, "jitVirtualChildClass", "()Ljava/lang/Class;");
  if (base_class_id == nullptr || child_class_id == nullptr || env->ExceptionCheck()) return false;
  jclass base = static_cast<jclass>(env->CallStaticObjectMethod(java_owner, base_class_id));
  jclass child = static_cast<jclass>(env->CallStaticObjectMethod(java_owner, child_class_id));
  if (base == nullptr || child == nullptr || env->ExceptionCheck()) return false;
  jobject first = env->AllocObject(base), second = env->AllocObject(child);
  if (first == nullptr || second == nullptr || env->ExceptionCheck()) return false;
  jfieldID number = env->GetFieldID(base, "number", "I");
  jfieldID object = env->GetFieldID(base, "object", "Ljava/lang/Object;");
  jfieldID alternative = env->GetFieldID(child, "alternative", "I");
  jfieldID alternative_object = env->GetFieldID(child, "alternativeObject", "Ljava/lang/Object;");
  if (number == nullptr || object == nullptr || alternative == nullptr || alternative_object == nullptr ||
      env->ExceptionCheck()) return false;
  env->SetIntField(first, number, 41);
  env->SetIntField(second, number, -1);
  env->SetIntField(second, alternative, 73);
  env->SetObjectField(first, object, second);
  env->SetObjectField(second, object, nullptr);
  env->SetObjectField(second, alternative_object, first);
  const char* int_signature = "(Ldev/aim/probe/JitVirtualBase;)I";
  const char* ref_signature = "(Ldev/aim/probe/JitVirtualBase;)Ljava/lang/Object;";
  const char* int_name = baseline ? "jitBaselineVirtualValue" : "jitVirtualValue";
  const char* ref_name = baseline ? "jitBaselineVirtualReference" : "jitVirtualReference";
  jmethodID int_id = env->GetStaticMethodID(java_owner, int_name, int_signature);
  jmethodID ref_id = env->GetStaticMethodID(java_owner, ref_name, ref_signature);
  auto* int_caller = owner->FindClassMethod(int_name, int_signature, art::kRuntimePointerSize);
  auto* ref_caller = owner->FindClassMethod(ref_name, ref_signature, art::kRuntimePointerSize);
  if (int_id == nullptr || ref_id == nullptr || int_caller == nullptr || ref_caller == nullptr ||
      env->ExceptionCheck()) return false;
  auto check = [&]() {
    for (bool derived : {false, true}) {
      jobject receiver = derived ? second : first;
      if (env->CallStaticIntMethod(java_owner, int_id, receiver) != (derived ? 73 : 41) ||
          env->ExceptionCheck()) return false;
      jobject value = env->CallStaticObjectMethod(java_owner, ref_id, receiver);
      bool ok = !env->ExceptionCheck() && env->IsSameObject(value, derived ? first : second);
      env->DeleteLocalRef(value);
      if (!ok) return false;
    }
    return true;
  };
  if (!check()) return false;
  {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    art::Runtime::Current()->GetClassLinker()->MakeInitializedClassesVisiblyInitialized(self, true);
  }
  for (auto* target : {int_caller, ref_caller}) {
    const auto kind = baseline ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized;
    if (!jit->CompileMethod(target, self, kind, false) ||
        !jit->GetCodeCache()->ContainsPc(target->GetEntryPointFromQuickCompiledCode())) {
      std::cerr << "ART JIT virtual caller compile failed: " << target->PrettyMethod() << "\n";
      return false;
    }
    // Verify actual ARM64 vtable loads, not just an equivalent sharpened
    // direct call. These callers contain only invoke-virtual and return.
    art::CodeItemDataAccessor dex_code(*target->GetDexFile(), target->GetCodeItem());
    auto* resolved = target->GetDexCache()->GetResolvedMethod(
        art::Instruction::At(dex_code.Insns())->VRegB_35c());
    if (resolved == nullptr) return false;
    const size_t vtable_offset = art::mirror::Class::EmbeddedVTableEntryOffset(
        resolved->GetMethodIndex(), art::kRuntimePointerSize).SizeValue();
    if (vtable_offset % 8 != 0 || vtable_offset / 8 >= 4096) return false;
    const uint32_t class_load = 0xb9400000u;  // ldr w0, [x0, #Object.klass=0]
    const uint32_t method_load = 0xf9400000u | static_cast<uint32_t>((vtable_offset / 8) << 10);
    const auto* header = art::OatQuickMethodHeader::FromEntryPoint(target->GetEntryPointFromQuickCompiledCode());
    if (art::CodeInfo::IsBaseline(header->GetOptimizedCodeInfoPtr()) != baseline) return false;
    bool saw_class_load = false, saw_vtable_load = false;
    for (size_t offset = 0; offset + 4 <= header->GetCodeSize(); offset += 4) {
      uint32_t instruction;
      std::memcpy(&instruction, header->GetCode() + offset, sizeof(instruction));
      saw_class_load |= instruction == class_load;
      if (saw_class_load && instruction == method_load) saw_vtable_load = true;
    }
    if (!saw_vtable_load) {
      std::cerr << "ART JIT virtual: missing actual ARM64 vtable load in " << target->PrettyMethod() << "\n";
      return false;
    }
  }
  std::cerr << "ART JIT virtual: actual ARM64 receiver-class and vtable method loads PASS\n";
  auto cache_mask = [&](art::ArtMethod* method) {
    art::ScopedProfilingInfoUse use(jit, method, self);
    auto* info = use.GetProfilingInfo();
    if (info == nullptr) return -1;
    auto* cache = info->GetInlineCache(0);
    if (cache == nullptr) return -1;
    const auto* slots = reinterpret_cast<const uint8_t*>(cache) + art::InlineCache::ClassesOffset().Int32Value();
    const uint32_t base_ref = art::mirror::CompressedReference<art::mirror::Object>::FromMirrorPtr(
        self->DecodeJObject(base).Ptr()).AsVRegValue();
    const uint32_t child_ref = art::mirror::CompressedReference<art::mirror::Object>::FromMirrorPtr(
        self->DecodeJObject(child).Ptr()).AsVRegValue();
    int mask = 0;
    for (unsigned slot = 0; slot < art::InlineCache::kIndividualCacheSize; ++slot) {
      uint32_t value;
      std::memcpy(&value, slots + slot * sizeof(uint32_t), sizeof(value));
      if (value == base_ref) mask |= 1;
      else if (value == child_ref) mask |= 2;
      else if (value != 0) return -1;
    }
    return mask;
  };
  const bool profile_cache = baseline && cache_mask(int_caller) >= 0 && cache_mask(ref_caller) >= 0;
  if (baseline && !profile_cache) {
    std::cerr << "ART JIT baseline: missing required inline cache\n";
    return false;
  }
  if (profile_cache) {
    std::cerr << "ART JIT baseline cache initial: " << cache_mask(int_caller) << "," << cache_mask(ref_caller) << "\n";
    if (cache_mask(int_caller) != 0 || cache_mask(ref_caller) != 0) return false;
    if (env->CallStaticIntMethod(java_owner, int_id, first) != 41 || env->ExceptionCheck()) return false;
    jobject value = env->CallStaticObjectMethod(java_owner, ref_id, first);
    if (env->ExceptionCheck() || !env->IsSameObject(value, second)) return false;
    env->DeleteLocalRef(value);
    std::cerr << "ART JIT baseline cache mono: " << cache_mask(int_caller) << "," << cache_mask(ref_caller) << "\n";
    if (cache_mask(int_caller) != 1 || cache_mask(ref_caller) != 1) return false;
  }
  // Both receiver classes are loaded before compilation: overriding dispatch
  // must not be mistaken for a single final/direct target.
  if (!check()) return false;
  if (profile_cache && (cache_mask(int_caller) != 3 || cache_mask(ref_caller) != 3)) return false;
  std::vector<art::ArtMethod*> targets;
  for (jclass type : {base, child}) {
    for (const auto& descriptor : {std::pair{"value", "()I"},
                                   std::pair{"reference", "()Ljava/lang/Object;"}}) {
      auto* target = self->DecodeJObject(type)->AsClass()->FindClassMethod(
          descriptor.first, descriptor.second, art::kRuntimePointerSize);
      if (target == nullptr) return false;
      if (!jit->GetCodeCache()->ContainsPc(target->GetEntryPointFromQuickCompiledCode()) &&
          !jit->CompileMethod(target, self, art::CompilationKind::kOptimized, false)) return false;
      if (!jit->GetCodeCache()->ContainsPc(target->GetEntryPointFromQuickCompiledCode())) return false;
      targets.push_back(target);
    }
  }
  for (unsigned cycle = 0; cycle != 3; ++cycle) {
    if (!check()) return false;
    {
      art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
      art::Runtime::Current()->GetHeap()->CollectGarbage(false);
    }
    if (!check()) return false;
  }
  env->CallStaticIntMethod(java_owner, int_id, nullptr);
  if (!ClearExpectedArrayException(env, "java/lang/NullPointerException")) return false;
  env->CallStaticObjectMethod(java_owner, ref_id, nullptr);
  if (!ClearExpectedArrayException(env, "java/lang/NullPointerException") || !check()) return false;
  env->SetObjectField(first, object, nullptr);
  jobject null_result = env->CallStaticObjectMethod(java_owner, ref_id, first);
  if (null_result != nullptr || env->ExceptionCheck()) return false;
  env->SetObjectField(first, object, second);
  if (!check()) return false;
  targets.push_back(int_caller);
  targets.push_back(ref_caller);
  for (auto* target : targets)
    if (!jit->GetCodeCache()->ContainsPc(target->GetEntryPointFromQuickCompiledCode())) return false;
  if (profile_cache && (cache_mask(int_caller) != 3 || cache_mask(ref_caller) != 3)) return false;
  if (profile_cache) std::cerr << "ART JIT baseline virtual: inline cache empty->mono->poly and GC PASS\n";
  if (baseline) {
    for (bool reference : {false, true}) {
      const char* name = reference ? "jitSpeculativeVirtualReference" : "jitSpeculativeVirtualValue";
      const char* signature = reference ? ref_signature : int_signature;
      jmethodID id = env->GetStaticMethodID(java_owner, name, signature);
      auto* method = owner->FindClassMethod(name, signature, art::kRuntimePointerSize);
      if (id == nullptr || method == nullptr || env->ExceptionCheck()) return false;
      auto invoke = [&](bool derived) {
        jobject receiver = derived ? second : first;
        if (!reference) return env->CallStaticIntMethod(java_owner, id, receiver) == (derived ? 73 : 41) && !env->ExceptionCheck();
        jobject value = env->CallStaticObjectMethod(java_owner, id, receiver);
        bool ok = !env->ExceptionCheck() && env->IsSameObject(value, derived ? first : second);
        env->DeleteLocalRef(value);
        return ok;
      };
      if (!invoke(false) || !jit->CompileMethod(method, self, art::CompilationKind::kBaseline, false) ||
          cache_mask(method) != 0 || !invoke(false) || cache_mask(method) != 1) return false;
      const void* baseline_entry = method->GetEntryPointFromQuickCompiledCode();
      if (!jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false)) return false;
      const void* optimized_entry = method->GetEntryPointFromQuickCompiledCode();
      if (optimized_entry == baseline_entry || !jit->GetCodeCache()->ContainsPc(optimized_entry) ||
          art::CodeInfo::IsBaseline(art::OatQuickMethodHeader::FromEntryPoint(optimized_entry)->GetOptimizedCodeInfoPtr())) return false;
      if (reference) {
        jmethodID outer_id = env->GetStaticMethodID(java_owner, "jitNestedSpeculativeReference", signature);
        auto* outer = owner->FindClassMethod("jitNestedSpeculativeReference", signature, art::kRuntimePointerSize);
        if (outer_id == nullptr || outer == nullptr || env->ExceptionCheck()) return false;
        jobject warm = env->CallStaticObjectMethod(java_owner, outer_id, first);
        if (env->ExceptionCheck() || !env->IsSameObject(warm, second)) return false;
        env->DeleteLocalRef(warm);
        if (!jit->CompileMethod(outer, self, art::CompilationKind::kOptimized, false) ||
            !jit->GetCodeCache()->ContainsPc(outer->GetEntryPointFromQuickCompiledCode())) return false;
        const auto* outer_header = art::OatQuickMethodHeader::FromEntryPoint(outer->GetEntryPointFromQuickCompiledCode());
        if (!art::CodeInfo::HasInlineInfo(outer_header->GetOptimizedCodeInfoPtr())) {
          std::cerr << "ART JIT speculative: missing nested inline environment\n";
          return false;
        }
        art::CodeInfo info(outer_header);
        bool found_inner = false;
        for (const auto& map : info.GetStackMaps()) {
          if (map.GetDexPc() != 0 || !map.HasInlineInfo()) continue;
          for (const auto& frame : info.GetInlineInfosOf(map)) {
            if (frame.EncodesArtMethod() && frame.GetArtMethod() == method && frame.GetDexPc() == 0)
              found_inner = true;
          }
        }
        if (!found_inner) return false;
        id = outer_id;
        std::cerr << "ART JIT speculative: nested inline metadata present\n";
      }
      const uint32_t before = art::Runtime::Current()->GetNumberOfDeoptimizations();
      if (!invoke(false) || art::Runtime::Current()->GetNumberOfDeoptimizations() != before) return false;
      if (!invoke(true)) return false;
      const uint32_t after = art::Runtime::Current()->GetNumberOfDeoptimizations();
      std::cerr << "ART JIT speculative " << name << ": deopts=" << before << "->" << after << "\n";
      if (after != before + 1 || !invoke(false) || !invoke(true)) return false;
      {
        art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
        art::Runtime::Current()->GetHeap()->CollectGarbage(false);
      }
      if (!invoke(false) || !invoke(true)) return false;
    }
  }
  // Range invocation exercises receiver plus overflowing integer argument
  // registers, a wide stack value, floating register and compressed reference.
  if (baseline) {
    const char* sig = "(Ldev/aim/probe/JitVirtualBase;IIIIIIIJDLjava/lang/Object;)J";
    jmethodID id = env->GetStaticMethodID(java_owner, "jitVirtualRange", sig);
    auto* method = owner->FindClassMethod("jitVirtualRange", sig, art::kRuntimePointerSize);
    if (id == nullptr || method == nullptr || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
    if (art::Instruction::At(code.Insns())->Opcode() != art::Instruction::INVOKE_VIRTUAL_RANGE) return false;
    jvalue args[11] = {};
    for (int i = 1; i <= 7; ++i) args[i].i = i * 11;
    args[8].j = 0x123456789abcdefLL;
    args[9].d = -1234.5;
    args[10].l = second;
    auto check_range = [&]() {
      for (bool derived : {false, true}) {
        args[0].l = derived ? second : first;
        jlong value = env->CallStaticLongMethodA(java_owner, id, args);
        if (env->ExceptionCheck() || value != (derived ? -args[8].j : args[8].j)) return false;
      }
      return true;
    };
    if (!check_range() || !jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    for (int cycle = 0; cycle < 3; ++cycle) {
      if (!check_range()) return false;
      args[8].j = -args[8].j;
    }
    args[0].l = nullptr;
    env->CallStaticLongMethodA(java_owner, id, args);
    if (!ClearExpectedArrayException(env, "java/lang/NullPointerException") || !check_range() ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    std::cerr << "ART JIT virtual range: override mixed wide/FP/reference calleeGC/null recovery PASS\n";
    jmethodID inner_id = env->GetStaticMethodID(java_owner, "jitWideSpeculative", sig);
    auto* inner = owner->FindClassMethod("jitWideSpeculative", sig, art::kRuntimePointerSize);
    jmethodID outer_id = env->GetStaticMethodID(java_owner, "jitNestedWideSpeculative", sig);
    auto* outer = owner->FindClassMethod("jitNestedWideSpeculative", sig, art::kRuntimePointerSize);
    if (!inner_id || !inner || !outer_id || !outer || env->ExceptionCheck()) return false;
    args[0].l = first;
    if (env->CallStaticLongMethodA(java_owner, inner_id, args) != args[8].j || env->ExceptionCheck() ||
        !jit->CompileMethod(inner, self, art::CompilationKind::kBaseline, false) || cache_mask(inner) != 0) return false;
    if (env->CallStaticLongMethodA(java_owner, inner_id, args) != args[8].j || env->ExceptionCheck() || cache_mask(inner) != 1) return false;
    if (env->CallStaticLongMethodA(java_owner, outer_id, args) != args[8].j || env->ExceptionCheck() ||
        !jit->CompileMethod(outer, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(outer->GetEntryPointFromQuickCompiledCode())) return false;
    const auto* header = art::OatQuickMethodHeader::FromEntryPoint(outer->GetEntryPointFromQuickCompiledCode());
    if (!art::CodeInfo::HasInlineInfo(header->GetOptimizedCodeInfoPtr())) return false;
    uint32_t before = art::Runtime::Current()->GetNumberOfDeoptimizations();
    if (env->CallStaticLongMethodA(java_owner, outer_id, args) != args[8].j || env->ExceptionCheck() ||
        art::Runtime::Current()->GetNumberOfDeoptimizations() != before) return false;
    args[0].l = second;
    if (env->CallStaticLongMethodA(java_owner, outer_id, args) != -args[8].j || env->ExceptionCheck()) return false;
    uint32_t after = art::Runtime::Current()->GetNumberOfDeoptimizations();
    std::cerr << "ART JIT nested wide deopt: " << before << "->" << after << " mixed argument result restored\n";
    if (after != before + 1) return false;
  }
  env->DeleteLocalRef(second);
  env->DeleteLocalRef(first);
  env->DeleteLocalRef(child);
  env->DeleteLocalRef(base);
  std::cerr << "ART JIT virtual: base/override interpreted+compiled int/reference targets, GC/null PASS\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
