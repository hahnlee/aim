#pragma once
#include <limits>
#include <vector>
namespace aim_jni_acceptance_phase {
inline bool CheckJitIntDiv(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                           art::Handle<art::mirror::Class> owner, jclass java_owner) {
  const char* names[] = {"jitIntDivide", "jitIntRemainder", "jitIntDivideSeven",
    "jitIntRemainderSeven", "jitIntDivideMinusOne", "jitIntRemainderMinusOne"};
  const jint values[] = {0,1,-1,7,-7,17,-29,0x12345678,-0x12345678,
    std::numeric_limits<jint>::min(),std::numeric_limits<jint>::max()};
  jclass arithmetic = env->FindClass("java/lang/ArithmeticException");
  if (!arithmetic || env->ExceptionCheck()) return false;
  unsigned cases = 0;
  for (unsigned op = 0; op < 6; ++op) {
    jmethodID id = env->GetStaticMethodID(java_owner,names[op],"(II)I");
    auto* method = owner->FindClassMethod(names[op],"(II)I",art::kRuntimePointerSize);
    if (!id || !method || env->ExceptionCheck() ||
        jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    if (op < 4) {
      art::CodeItemDataAccessor code(*method->GetDexFile(),method->GetCodeItem());
      bool found=false;
      for (art::DexInstructionPcPair pair:code) {
        auto opcode=pair.Inst().Opcode();
        if (op==0) found |= opcode==art::Instruction::DIV_INT || opcode==art::Instruction::DIV_INT_2ADDR;
        if (op==1) found |= opcode==art::Instruction::REM_INT || opcode==art::Instruction::REM_INT_2ADDR;
        if (op==2) found |= opcode==art::Instruction::DIV_INT_LIT8 || opcode==art::Instruction::DIV_INT_LIT16;
        if (op==3) found |= opcode==art::Instruction::REM_INT_LIT8 || opcode==art::Instruction::REM_INT_LIT16;
      }
      if (!found) return false;
    }
    struct Result { jint value; bool threw; bool valid; };
    auto invoke = [&](jint a,jint b) {
      jvalue args[2] = {}; args[0].i=a; args[1].i=b;
      Result r{env->CallStaticIntMethodA(java_owner,id,args),false,true};
      if (env->ExceptionCheck()) {
        jthrowable e=env->ExceptionOccurred(); env->ExceptionClear();
        r.threw=true; r.valid=env->IsInstanceOf(e,arithmetic); env->DeleteLocalRef(e);
      }
      return r;
    };
    std::vector<Result> expected;
    for (jint a:values) for (jint b:values) expected.push_back(invoke(a,b));
    if (jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
        !jit->CompileMethod(method,self,art::CompilationKind::kOptimized,false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    size_t index=0;
    for (jint a:values) for (jint b:values) {
      Result actual=invoke(a,b), before=expected[index++];
      jint denominator=op<2 ? b : op<4 ? 7 : -1;
      bool throws=denominator==0;
      // Widen first: native signed int MIN/-1 would be undefined behavior.
      int64_t wide=throws ? 0 : (op%2 ? int64_t(a)%denominator : int64_t(a)/denominator);
      uint32_t oracle=static_cast<uint32_t>(wide);
      if (!actual.valid || !before.valid || actual.threw!=throws || before.threw!=throws ||
          (!throws && (actual.value!=before.value || static_cast<uint32_t>(actual.value)!=oracle))) {
        std::cerr << "ART JIT int div mismatch " << names[op] << " case=" << index-1 << "\n";
        return false;
      }
      ++cases;
    }
  }
  env->DeleteLocalRef(arithmetic);
  std::cerr << "ART JIT int div/rem: variable and literal arithmetic/exception oracles PASS cases=" << cases << "\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
