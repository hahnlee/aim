#pragma once
#include <limits>
#include <vector>
namespace aim_jni_acceptance_phase {
inline bool CheckJitSwitch(JNIEnv* env,art::Thread* self,art::jit::Jit* jit,
                            art::Handle<art::mirror::Class> owner,jclass java_owner) {
  const char* names[]={"jitPackedSwitch","jitSparseSwitch"};
  const jint dense[]={17,31,-91,5,23,-47,101,7,89,-11,73,41};
  const jint sparse_keys[]={std::numeric_limits<jint>::min(),-1000000,-7,0,13,65535,std::numeric_limits<jint>::max()};
  const jint sparse_values[]={17,31,-91,5,23,-47,101};
  std::vector<jint> keys;
  for(jint key=-12;key<=24;++key) keys.push_back(key);
  for(jint key:sparse_keys) {
    keys.push_back(key);
    if(key>std::numeric_limits<jint>::min()) keys.push_back(key-1);
    if(key<std::numeric_limits<jint>::max()) keys.push_back(key+1);
  }
  unsigned cases=0;
  for(unsigned kind=0;kind<2;++kind) {
    auto* method=owner->FindClassMethod(names[kind],"(I)I",art::kRuntimePointerSize);
    jmethodID id=env->GetStaticMethodID(java_owner,names[kind],"(I)I");
    if(!method || !id || env->ExceptionCheck() ||
       jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    art::CodeItemDataAccessor code(*method->GetDexFile(),method->GetCodeItem());
    bool found=false;
    for(auto pair:code) found |= pair.Inst().Opcode()==(kind==0 ? art::Instruction::PACKED_SWITCH:art::Instruction::SPARSE_SWITCH);
    if(!found) return false;
    std::vector<jint> expected;
    for(jint key:keys) expected.push_back(env->CallStaticIntMethod(java_owner,id,key));
    if(env->ExceptionCheck() || jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
       !jit->CompileMethod(method,self,art::CompilationKind::kOptimized,false) ||
       !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    for(size_t i=0;i<keys.size();++i) {
      jint key=keys[i],oracle=-999;
      if(kind==0 && key>=-4 && key<=7) oracle=dense[key+4];
      if(kind==1) for(unsigned j=0;j<7;++j) if(key==sparse_keys[j]) oracle=sparse_values[j];
      jint actual=env->CallStaticIntMethod(java_owner,id,key);
      if(env->ExceptionCheck() || actual!=expected[i] || actual!=oracle) return false;
      ++cases;
    }
  }
  std::cerr<<"ART JIT switch: packed/sparse opcodes and independent key mapping PASS cases="<<cases<<"\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
