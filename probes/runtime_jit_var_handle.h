#pragma once
#include "runtime_jit_var_handle_atomics.h"
#include "runtime_jit_var_handle_array.h"
#include "runtime_jit_var_handle_byte_view.h"
#include "runtime_jit_var_handle_byte_view_narrow16.h"
#include "runtime_jit_var_handle_byte_view_fp.h"
#include "runtime_jit_var_handle_buffer_view.h"
#include "runtime_jit_var_handle_buffer_view_narrow16.h"
#include "runtime_jit_var_handle_buffer_view_fp.h"
#include "runtime_jit_var_handle_double.h"
#include "runtime_jit_var_handle_float.h"
#include "runtime_jit_var_handle_fp_ordering.h"
#include "runtime_jit_var_handle_long.h"
#include "runtime_jit_var_handle_long_ordering.h"
#include "runtime_jit_var_handle_narrow8.h"
#include "runtime_jit_var_handle_narrow16.h"
#include "runtime_jit_var_handle_narrow_ordering.h"
#include "runtime_jit_var_handle_ordering.h"
#include "runtime_jit_var_handle_ordering_modes.h"
#include "runtime_jit_var_handle_ordering_shapes.h"
#include "runtime_jit_var_handle_reference.h"
#include "runtime_jit_var_handle_reference_ordering.h"
#include "runtime_jit_var_handle_view_ordering.h"
#include "runtime_jit_var_handle_view_narrow16_ordering.h"
#include "runtime_jit_var_handle_view_fp_ordering.h"
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandle(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject receiver, jclass npe) {
  auto factory = env->GetStaticMethodID(java_owner, "jitVarHandle", "()Ljava/lang/invoke/VarHandle;");
  if (!factory || env->ExceptionCheck()) return false;
  jobject handle = env->CallStaticObjectMethod(java_owner, factory);
  const char* names[] = {"jitVarGet", "jitVarSet"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;)I",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;I)V"};
  jmethodID ids[2]{};
  art::ArtMethod* methods[2]{};
  for (int i = 0; i < 2; ++i) {
    ids[i] = env->GetStaticMethodID(java_owner, names[i], sigs[i]);
    methods[i] = owner->FindClassMethod(names[i], sigs[i], art::kRuntimePointerSize);
    if (!handle || !ids[i] || !methods[i] || env->ExceptionCheck()) {
      env->ExceptionDescribe(); return false;
    }
    art::CodeItemDataAccessor code(*methods[i]->GetDexFile(), methods[i]->GetCodeItem());
    if (art::Instruction::At(code.Insns())->Opcode() != art::Instruction::INVOKE_POLYMORPHIC) return false;
  }
  auto field = env->GetFieldID(java_owner, "jitVarValue", "I");
  if (!field || env->ExceptionCheck()) return false;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT VarHandle compile failed phase=" << phase << "\n";
        return false;
      }
    }
    for (jint value : {jint(0), jint(1), jint(-1), jint(INT32_MIN), jint(INT32_MAX), jint(0x12345678)}) {
      env->CallStaticVoidMethod(java_owner, ids[1], handle, receiver, value);
      if (env->ExceptionCheck() || env->GetIntField(receiver, field) != value) {
        std::cerr << "ART JIT VarHandle set failed phase=" << phase << "\n";
        env->ExceptionDescribe(); return false;
      }
      { art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
        art::Runtime::Current()->GetHeap()->CollectGarbage(false); }
      jint actual = env->CallStaticIntMethod(java_owner, ids[0], handle, receiver);
      if (env->ExceptionCheck() || actual != value) {
        std::cerr << "ART JIT VarHandle get failed phase=" << phase << "\n";
        env->ExceptionDescribe(); return false;
      }
      env->SetIntField(receiver, field, value ^ 123);
      if (env->CallStaticIntMethod(java_owner, ids[0], handle, receiver) != (value ^ 123) ||
          env->ExceptionCheck()) return false;
    }
    for (int null_handle = 0; null_handle < 2; ++null_handle) {
      for (int setter = 0; setter < 2; ++setter) {
        if (setter) env->CallStaticVoidMethod(java_owner, ids[1],
            null_handle ? nullptr : handle, null_handle ? receiver : nullptr, jint(1));
        else env->CallStaticIntMethod(java_owner, ids[0],
            null_handle ? nullptr : handle, null_handle ? receiver : nullptr);
        jthrowable thrown = env->ExceptionOccurred();
        if (thrown) env->ExceptionClear();
        bool correct = thrown && env->IsInstanceOf(thrown, npe);
        if (thrown) env->DeleteLocalRef(thrown);
        if (!correct) return false;
      }
    }
  }
  if (!CheckJitVarHandleAtomics(env, self, jit, owner, java_owner, receiver, handle, field, npe)) return false;
  if (!CheckJitVarHandleOrdering(env, self, jit, owner, java_owner, handle)) return false;
  if (!CheckJitVarHandleOrderingModes(
          env, self, jit, owner, java_owner, receiver, handle, field, npe)) return false;
  if (!CheckJitVarHandleOrderingShapes(env, self, jit, owner, java_owner, npe)) return false;
  if (!CheckJitVarHandleReference(env, self, jit, owner, java_owner, receiver, npe)) return false;
  if (!CheckJitVarHandleReferenceOrdering(
          env, self, jit, owner, java_owner, receiver, npe)) return false;
  if (!CheckJitVarHandleLong(env, self, jit, owner, java_owner, receiver, npe)) return false;
  if (!CheckJitVarHandleLongOrdering(
          env, self, jit, owner, java_owner, receiver, npe)) return false;
  if (!CheckJitVarHandleArray(env, self, jit, owner, java_owner, receiver, npe)) return false;
  if (!CheckJitVarHandleDouble(env, self, jit, owner, java_owner, receiver, npe)) return false;
  if (!CheckJitVarHandleFloat(env, self, jit, owner, java_owner, receiver, npe)) return false;
  if (!CheckJitVarHandleFpOrdering(env, self, jit, owner, java_owner, receiver, npe)) return false;
  if (!CheckJitVarHandleNarrow8(env, self, jit, owner, java_owner, receiver, npe)) return false;
  if (!CheckJitVarHandleNarrow16(env, self, jit, owner, java_owner, receiver, npe)) return false;
  if (!CheckJitVarHandleNarrowOrdering(
          env, self, jit, owner, java_owner, receiver, npe)) return false;
  if (!CheckJitVarHandleByteView(env, self, jit, owner, java_owner, npe)) return false;
  if (!CheckJitVarHandleByteViewNarrow16(env, self, jit, owner, java_owner, npe)) return false;
  if (!CheckJitVarHandleByteViewFp(env, self, jit, owner, java_owner, npe)) return false;
  if (!CheckJitVarHandleByteBufferView(env, self, jit, owner, java_owner, npe)) return false;
  if (!CheckJitVarHandleByteBufferViewNarrow16(env, self, jit, owner, java_owner, npe)) return false;
  if (!CheckJitVarHandleByteBufferViewFp(env, self, jit, owner, java_owner, npe)) return false;
  if (!CheckJitVarHandleViewOrdering(env, self, jit, owner, java_owner, npe)) return false;
  if (!CheckJitVarHandleViewNarrow16Ordering(env, self, jit, owner, java_owner, npe)) return false;
  if (!CheckJitVarHandleViewFpOrdering(env, self, jit, owner, java_owner, npe)) return false;
  env->DeleteLocalRef(handle);
  std::cerr << "ART JIT VarHandle: int get/set/GC=18 independent-store/read=18 null-failures=12 PASS\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
