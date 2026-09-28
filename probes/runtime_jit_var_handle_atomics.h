#pragma once
#include "runtime_jit_var_handle_contention.h"
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleAtomics(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject receiver,
    jobject handle, jfieldID field, jclass npe) {
  const char* names[] = {"jitVarCas", "jitVarExchange", "jitVarAdd", "jitVarSwap", "jitVarXor"};
  const char* signatures[] = {
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;II)Z",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;II)I",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;I)I"};
  jmethodID ids[5]{};
  art::ArtMethod* methods[5]{};
  for (int kind = 0; kind < 5; ++kind) {
    const char* sig = signatures[kind < 2 ? kind : 2];
    ids[kind] = env->GetStaticMethodID(java_owner, names[kind], sig);
    methods[kind] = owner->FindClassMethod(names[kind], sig, art::kRuntimePointerSize);
    if (!ids[kind] || !methods[kind] || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*methods[kind]->GetDexFile(), methods[kind]->GetCodeItem());
    if (art::Instruction::At(code.Insns())->Opcode() != art::Instruction::INVOKE_POLYMORPHIC) return false;
  }
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT VarHandle atomic compile failed phase=" << phase << "\n";
        return false;
      }
    }
    for (jint seed : {jint(0), jint(1), jint(-1), jint(INT32_MIN), jint(INT32_MAX), jint(0x12345678)}) {
      jint update = seed ^ 0x5a5a5a5a;
      auto check = [&](bool result, uint32_t expected, const char* operation) {
        bool correct = result && !env->ExceptionCheck() &&
            static_cast<uint32_t>(env->GetIntField(receiver, field)) == expected;
        if (!correct) {
          std::cerr << "ART JIT VarHandle atomic failed op=" << operation
                    << " phase=" << phase << " seed=" << seed << "\n";
          env->ExceptionDescribe();
        }
        return correct;
      };
      env->SetIntField(receiver, field, seed);
      if (!check(env->CallStaticBooleanMethod(java_owner, ids[0], handle, receiver, seed ^ 1, update) == JNI_FALSE,
                 static_cast<uint32_t>(seed), "CAS mismatch")) return false;
      if (!check(env->CallStaticBooleanMethod(java_owner, ids[0], handle, receiver, seed, update) == JNI_TRUE,
                 static_cast<uint32_t>(update), "CAS success")) return false;
      if (!check(env->CallStaticIntMethod(java_owner, ids[1], handle, receiver, seed, jint(0)) == update,
                 static_cast<uint32_t>(update), "exchange mismatch")) return false;
      if (!check(env->CallStaticIntMethod(java_owner, ids[1], handle, receiver, update, seed) == update,
                 static_cast<uint32_t>(seed), "exchange success")) return false;
      uint32_t sum_bits = static_cast<uint32_t>(seed) + UINT32_C(0x3456789);
      if (!check(env->CallStaticIntMethod(java_owner, ids[2], handle, receiver, jint(0x3456789)) == seed,
                 sum_bits, "get-and-add")) return false;
      if (!check(static_cast<uint32_t>(env->CallStaticIntMethod(java_owner, ids[3], handle, receiver, update)) == sum_bits,
                 static_cast<uint32_t>(update), "get-and-set")) return false;
      if (!check(env->CallStaticIntMethod(java_owner, ids[4], handle, receiver, jint(0x13579bdf)) == update,
                 static_cast<uint32_t>(update) ^ UINT32_C(0x13579bdf), "get-and-xor")) return false;
    }
    for (int kind = 0; kind < 5; ++kind) for (int null_handle = 0; null_handle < 2; ++null_handle) {
      jvalue args[4]{};
      args[0].l = null_handle ? nullptr : handle;
      args[1].l = null_handle ? receiver : nullptr;
      if (kind == 0) env->CallStaticBooleanMethodA(java_owner, ids[kind], args);
      else env->CallStaticIntMethodA(java_owner, ids[kind], args);
      jthrowable thrown = env->ExceptionOccurred();
      if (thrown) env->ExceptionClear();
      bool correct = thrown && env->IsInstanceOf(thrown, npe);
      if (thrown) env->DeleteLocalRef(thrown);
      if (!correct) return false;
    }
  }
  if (!CheckJitVarHandleContention(env, self, java_owner, handle, receiver, field, ids[2])) return false;
  std::cerr << "ART JIT VarHandle: CAS/exchange/add/swap/xor operations=126 null-failures=30 PASS\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
