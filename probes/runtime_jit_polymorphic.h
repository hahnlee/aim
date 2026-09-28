#pragma once
#include "mirror/method_handle_impl-inl.h"
#include "runtime_jit_polymorphic_fp.h"
#include "runtime_jit_polymorphic_inexact.h"
#include "runtime_jit_polymorphic_accessors.h"
#include "runtime_jit_var_handle.h"
namespace aim_jni_acceptance_phase {
inline bool CheckJitPolymorphic(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  auto factory = env->GetStaticMethodID(java_owner, "jitPolymorphicHandle", "()Ljava/lang/invoke/MethodHandle;");
  if (!factory || env->ExceptionCheck()) return false;
  jobject handle = env->CallStaticObjectMethod(java_owner, factory);
  if (!handle || env->ExceptionCheck()) {
    std::cerr << "ART JIT MethodHandle factory failed\n";
    env->ExceptionDescribe();
    return false;
  }
  const char* sig = "(Ljava/lang/invoke/MethodHandle;I)I";
  auto id = env->GetStaticMethodID(java_owner, "jitPolymorphicExact", sig);
  auto* method = owner->FindClassMethod("jitPolymorphicExact", sig, art::kRuntimePointerSize);
  jclass npe = env->FindClass("java/lang/NullPointerException");
  if (!id || !method || !npe || env->ExceptionCheck()) return false;
  art::CodeItemDataAccessor code(*method->GetDexFile(), method->GetCodeItem());
  if (art::Instruction::At(code.Insns())->Opcode() != art::Instruction::INVOKE_POLYMORPHIC) return false;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase && (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()))) {
      std::cerr << "ART JIT invoke-polymorphic compile failed phase=" << phase << "\n";
      return false;
    }
    for (jint input : {jint(0), jint(1), jint(-1), jint(INT32_MIN), jint(INT32_MAX), jint(0x12345678)}) {
      jint actual = env->CallStaticIntMethod(java_owner, id, handle, input);
      uint32_t expected = static_cast<uint32_t>(input) * 31u + 7u;
      if (env->ExceptionCheck() || static_cast<uint32_t>(actual) != expected) {
        std::cerr << "ART JIT invoke-polymorphic result failed phase=" << phase << " input=" << input << "\n";
        env->ExceptionDescribe();
        return false;
      }
    }
    env->CallStaticIntMethod(java_owner, id, nullptr, jint(1));
    jthrowable thrown = env->ExceptionOccurred();
    if (thrown) env->ExceptionClear();
    bool correct = thrown && env->IsInstanceOf(thrown, npe);
    if (thrown) env->DeleteLocalRef(thrown);
    if (!correct) return false;
  }
  auto ref_factory = env->GetStaticMethodID(java_owner, "jitPolymorphicReferenceHandle", "()Ljava/lang/invoke/MethodHandle;");
  if (!ref_factory || env->ExceptionCheck()) return false;
  jobject ref_handle = env->CallStaticObjectMethod(java_owner, ref_factory);
  const char* ref_sig = "(Ljava/lang/invoke/MethodHandle;Ljava/lang/Object;)Ljava/lang/Object;";
  auto ref_id = env->GetStaticMethodID(java_owner, "jitPolymorphicReference", ref_sig);
  auto* ref_method = owner->FindClassMethod("jitPolymorphicReference", ref_sig, art::kRuntimePointerSize);
  jclass wrong_type = env->FindClass("java/lang/invoke/WrongMethodTypeException");
  jobject object = env->AllocObject(java_owner);
  if (!ref_handle || !ref_id || !ref_method || !wrong_type || !object || env->ExceptionCheck()) return false;
  art::CodeItemDataAccessor ref_code(*ref_method->GetDexFile(), ref_method->GetCodeItem());
  if (art::Instruction::At(ref_code.Insns())->Opcode() != art::Instruction::INVOKE_POLYMORPHIC) return false;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase && (!jit->CompileMethod(ref_method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(ref_method->GetEntryPointFromQuickCompiledCode()))) return false;
    for (jobject input : {jobject(nullptr), object, jobject(java_owner), ref_handle}) {
      uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
      jobject result = env->CallStaticObjectMethod(java_owner, ref_id, ref_handle, input);
      bool correct = !env->ExceptionCheck() && env->IsSameObject(result, input) &&
          art::Runtime::Current()->GetHeap()->GetGcCount() > before;
      if (result) env->DeleteLocalRef(result);
      if (!correct) {
        std::cerr << "ART JIT polymorphic reference/GC failed phase=" << phase << "\n";
        env->ExceptionDescribe();
        return false;
      }
    }
    for (jobject bad_handle : {jobject(nullptr), handle}) {
      jobject result = env->CallStaticObjectMethod(java_owner, ref_id, bad_handle, object);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = thrown && env->IsInstanceOf(thrown, bad_handle ? wrong_type : npe);
      if (result) env->DeleteLocalRef(result);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) return false;
    }
  }
  auto adapted_factory = env->GetStaticMethodID(java_owner, "jitPolymorphicAdaptedHandle", "()Ljava/lang/invoke/MethodHandle;");
  if (!adapted_factory || env->ExceptionCheck()) return false;
  jobject adapted_handle = env->CallStaticObjectMethod(java_owner, adapted_factory);
  auto adapted_id = env->GetStaticMethodID(java_owner, "jitPolymorphicAdapted", ref_sig);
  auto* adapted_method = owner->FindClassMethod("jitPolymorphicAdapted", ref_sig, art::kRuntimePointerSize);
  auto* adapted_target = owner->FindClassMethod("jitPolymorphicAdaptedTarget",
      "(Ldev/aim/probe/Hello;)Ljava/lang/Object;", art::kRuntimePointerSize);
  jclass cast_error = env->FindClass("java/lang/ClassCastException");
  if (!adapted_handle || !adapted_id || !adapted_method || !adapted_target || !cast_error || env->ExceptionCheck()) return false;
  if (art::ObjPtr<art::mirror::MethodHandle>::DownCast(self->DecodeJObject(adapted_handle))->GetHandleKind() !=
      art::mirror::MethodHandle::kInvokeTransform) return false;
  art::CodeItemDataAccessor adapted_code(*adapted_method->GetDexFile(), adapted_method->GetCodeItem());
  if (art::Instruction::At(adapted_code.Insns())->Opcode() != art::Instruction::INVOKE_POLYMORPHIC) return false;
  for (int phase = 0; phase < 4; ++phase) {
    if (phase == 3 && (!jit->CompileMethod(adapted_target, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(adapted_target->GetEntryPointFromQuickCompiledCode()))) return false;
    if (phase > 0 && phase < 3 && (!jit->CompileMethod(adapted_method, self,
        phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(adapted_method->GetEntryPointFromQuickCompiledCode()))) return false;
    for (jobject input : {jobject(nullptr), object}) {
      uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
      jobject result = env->CallStaticObjectMethod(java_owner, adapted_id, adapted_handle, input);
      bool correct = !env->ExceptionCheck() && env->IsSameObject(result, input) &&
          art::Runtime::Current()->GetHeap()->GetGcCount() > before;
      if (result) env->DeleteLocalRef(result);
      if (!correct) {
        std::cerr << "ART JIT adapted MethodHandle reference/GC failed phase=" << phase
                  << " input=" << input << " result=" << result
                  << " gc_before=" << before << " gc_after=" << art::Runtime::Current()->GetHeap()->GetGcCount() << "\n";
        env->ExceptionDescribe();
        return false;
      }
    }
    for (jobject input : {jobject(java_owner), ref_handle}) {
      jobject result = env->CallStaticObjectMethod(java_owner, adapted_id, adapted_handle, input);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = thrown && env->IsInstanceOf(thrown, cast_error);
      if (result) env->DeleteLocalRef(result);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) return false;
    }
  }
  if (!CheckJitPolymorphicInexact(env, self, jit, owner, java_owner, handle, ref_handle, adapted_handle, npe, cast_error)) return false;
  env->DeleteLocalRef(adapted_handle); env->DeleteLocalRef(cast_error);
  std::cerr << "ART JIT invoke-polymorphic: adapted transform/reference/GC=8 cast-errors=8 PASS\n";
  auto dispatch_factory = env->GetStaticMethodID(java_owner, "jitPolymorphicDispatchHandle", "(Z)Ljava/lang/invoke/MethodHandle;");
  auto base_factory = env->GetStaticMethodID(java_owner, "jitVirtualBaseClass", "()Ljava/lang/Class;");
  auto child_factory = env->GetStaticMethodID(java_owner, "jitVirtualChildClass", "()Ljava/lang/Class;");
  if (!dispatch_factory || !base_factory || !child_factory || env->ExceptionCheck()) return false;
  jclass base = static_cast<jclass>(env->CallStaticObjectMethod(java_owner, base_factory));
  jclass child = static_cast<jclass>(env->CallStaticObjectMethod(java_owner, child_factory));
  if (!base || !child || env->ExceptionCheck()) return false;
  jobject receivers[] = {env->AllocObject(base), env->AllocObject(child)};
  auto number = env->GetFieldID(base, "number", "I");
  auto alternative = env->GetFieldID(child, "alternative", "I");
  if (!receivers[0] || !receivers[1] || !number || !alternative || env->ExceptionCheck()) return false;
  env->SetIntField(receivers[0], number, 31);
  env->SetIntField(receivers[1], number, 31);
  env->SetIntField(receivers[1], alternative, 79);
  { art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    art::Runtime::Current()->GetClassLinker()->MakeInitializedClassesVisiblyInitialized(self, true); }
  const char* names[] = {"jitPolymorphicVirtual", "jitPolymorphicInterface"};
  const char* signatures[] = {
      "(Ljava/lang/invoke/MethodHandle;Ldev/aim/probe/JitVirtualBase;)I",
      "(Ljava/lang/invoke/MethodHandle;Ldev/aim/probe/JitCallable;)I"};
  for (int kind = 0; kind < 2; ++kind) {
    jobject dispatch_handle = env->CallStaticObjectMethod(java_owner, dispatch_factory, jboolean(kind));
    auto dispatch_id = env->GetStaticMethodID(java_owner, names[kind], signatures[kind]);
    auto* dispatch_method = owner->FindClassMethod(names[kind], signatures[kind], art::kRuntimePointerSize);
    if (!dispatch_handle || !dispatch_id || !dispatch_method || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor dispatch_code(*dispatch_method->GetDexFile(), dispatch_method->GetCodeItem());
    if (art::Instruction::At(dispatch_code.Insns())->Opcode() != art::Instruction::INVOKE_POLYMORPHIC) return false;
    for (int phase = 0; phase < 3; ++phase) {
      if (phase && (!jit->CompileMethod(dispatch_method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(dispatch_method->GetEntryPointFromQuickCompiledCode()))) return false;
      for (int receiver = 0; receiver < 2; ++receiver) {
        jint actual = env->CallStaticIntMethod(java_owner, dispatch_id, dispatch_handle, receivers[receiver]);
        if (env->ExceptionCheck() || actual != (receiver == 0 ? 31 : 79)) {
          std::cerr << "ART JIT MethodHandle dispatch failed kind=" << kind << " phase=" << phase << " receiver=" << receiver << "\n";
          env->ExceptionDescribe();
          return false;
        }
      }
      env->CallStaticIntMethod(java_owner, dispatch_id, dispatch_handle, nullptr);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = thrown && env->IsInstanceOf(thrown, npe);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) return false;
    }
    env->DeleteLocalRef(dispatch_handle);
  }
  auto range_factory = env->GetStaticMethodID(java_owner, "jitPolymorphicRangeHandle", "()Ljava/lang/invoke/MethodHandle;");
  if (!range_factory || env->ExceptionCheck()) return false;
  jobject range_handle = env->CallStaticObjectMethod(java_owner, range_factory);
  const char* range_sig = "(Ljava/lang/invoke/MethodHandle;IIIIIIIJDLjava/lang/Object;)J";
  auto range_id = env->GetStaticMethodID(java_owner, "jitPolymorphicRange", range_sig);
  auto* range_method = owner->FindClassMethod("jitPolymorphicRange", range_sig, art::kRuntimePointerSize);
  if (!range_handle || !range_id || !range_method || env->ExceptionCheck()) return false;
  art::CodeItemDataAccessor range_code(*range_method->GetDexFile(), range_method->GetCodeItem());
  if (art::Instruction::At(range_code.Insns())->Opcode() != art::Instruction::INVOKE_POLYMORPHIC_RANGE) return false;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase && (!jit->CompileMethod(range_method, self,
        phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(range_method->GetEntryPointFromQuickCompiledCode()))) return false;
    for (jlong wide : {jlong(INT64_MIN), jlong(INT64_MAX), jlong(INT64_C(0x123456789abcdef))})
      for (jdouble real : {jdouble(-1234.5), jdouble(0.0)})
        for (jobject reference : {jobject(nullptr), object})
          for (jint last : {jint(77), jint(78)}) {
            jvalue args[11]{};
            args[0].l = range_handle;
            for (int i = 1; i <= 6; ++i) args[i].i = i * 11;
            args[7].i = last; args[8].j = wide; args[9].d = real; args[10].l = reference;
            uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
            jlong actual = env->CallStaticLongMethodA(java_owner, range_id, args);
            jlong expected = last == 77 && real == -1234.5 && reference ? wide : -1;
            if (env->ExceptionCheck() || actual != expected ||
                art::Runtime::Current()->GetHeap()->GetGcCount() <= before) {
              std::cerr << "ART JIT MethodHandle range/GC failed phase=" << phase << "\n";
              env->ExceptionDescribe();
              return false;
            }
          }
    for (jobject bad_handle : {jobject(nullptr), handle}) {
      jvalue args[11]{}; args[0].l = bad_handle;
      env->CallStaticLongMethodA(java_owner, range_id, args);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = thrown && env->IsInstanceOf(thrown, bad_handle ? wrong_type : npe);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) return false;
    }
  }
  if (!CheckJitPolymorphicFloating(env, self, jit, owner, java_owner, object, handle, npe, wrong_type)) return false;
  if (!CheckJitPolymorphicAccessors(env, self, jit, owner, java_owner, object, npe)) return false;
  if (!CheckJitVarHandle(env, self, jit, owner, java_owner, object, npe)) return false;
  env->DeleteLocalRef(range_handle);
  for (jobject receiver : receivers) env->DeleteLocalRef(receiver);
  env->DeleteLocalRef(base); env->DeleteLocalRef(child);
  env->DeleteLocalRef(object); env->DeleteLocalRef(wrong_type); env->DeleteLocalRef(ref_handle);
  env->DeleteLocalRef(npe); env->DeleteLocalRef(handle);
  std::cerr << "ART JIT invoke-polymorphic: platform MethodHandle exact int arithmetic=18 null=3 PASS\n";
  std::cerr << "ART JIT invoke-polymorphic: reference identity/callee-GC=12 null/wrong-type=6 PASS\n";
  std::cerr << "ART JIT invoke-polymorphic: virtual/interface base/override=12 null-receiver=6 PASS\n";
  std::cerr << "ART JIT invoke-polymorphic: range/wide/stack/callee-GC=72 null/wrong-type=6 PASS\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
