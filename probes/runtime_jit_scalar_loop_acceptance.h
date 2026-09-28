#pragma once
#include <vector>
namespace aim_jni_acceptance_phase {
inline bool CheckJitScalarLoops(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                                art::Handle<art::mirror::Class> owner, jclass java_owner) {
  const char* names[]={"jitLoopI","jitLoopJ","jitLoopF","jitLoopD"};
  const char* sigs[]={"(II)I","(JI)J","(FI)F","(DI)D"};
  const jint rounds[]={-1,0,1,2,17,64,257};
  unsigned cases=0;
  for(unsigned kind=0;kind<4;++kind) {
    auto* method=owner->FindClassMethod(names[kind],sigs[kind],art::kRuntimePointerSize);
    jmethodID id=env->GetStaticMethodID(java_owner,names[kind],sigs[kind]);
    if(!method || !id || env->ExceptionCheck() ||
       jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    bool backedge=false;
    art::CodeItemDataAccessor code(*method->GetDexFile(),method->GetCodeItem());
    for(art::DexInstructionPcPair pair:code) {
      const auto& i=pair.Inst();
      backedge |= (i.Opcode()==art::Instruction::GOTO && i.VRegA_10t()<0) ||
        (i.Opcode()==art::Instruction::GOTO_16 && i.VRegA_20t()<0) ||
        (i.Opcode()==art::Instruction::GOTO_32 && i.VRegA_30t()<0);
    }
    if(!backedge) return false;
    auto invoke=[&](int seed,jint n) {
      jvalue args[2]={}; args[1].i=n; uint64_t bits=0;
      if(kind==0) { args[0].i=seed; jint r=env->CallStaticIntMethodA(java_owner,id,args); std::memcpy(&bits,&r,sizeof(r)); }
      if(kind==1) { args[0].j=seed; jlong r=env->CallStaticLongMethodA(java_owner,id,args); std::memcpy(&bits,&r,sizeof(r)); }
      if(kind==2) { args[0].f=seed; jfloat r=env->CallStaticFloatMethodA(java_owner,id,args); std::memcpy(&bits,&r,sizeof(r)); }
      if(kind==3) { args[0].d=seed; jdouble r=env->CallStaticDoubleMethodA(java_owner,id,args); std::memcpy(&bits,&r,sizeof(r)); }
      return bits;
    };
    std::vector<uint64_t> expected;
    for(int seed:{0,1,-1,12345,-6789}) for(jint n:rounds) expected.push_back(invoke(seed,n));
    if(env->ExceptionCheck() || jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
       !jit->CompileMethod(method,self,art::CompilationKind::kOptimized,false) ||
       !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    size_t index=0;
    for(int seed:{0,1,-1,12345,-6789}) for(jint n:rounds) {
      if(invoke(seed,n)!=expected[index++] || env->ExceptionCheck()) {
        std::cerr<<"ART JIT scalar loop mismatch "<<names[kind]<<" n="<<n<<"\n"; return false;
      }
      ++cases;
    }
  }
  std::cerr<<"ART JIT scalar loops: IJFD backedges and loop-carried values PASS cases="<<cases<<"\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
