#pragma once
#include "interpreter/interpreter.h"
namespace art::jit {
extern "C" void art_quick_osr_stub(void**, size_t, const uint8_t*, JValue*, const char*, Thread*);
}
namespace aim_jni_acceptance_phase {
inline bool CheckJitOsrReferences(JNIEnv* env,art::Thread* self,art::jit::Jit* jit,
                                  art::Handle<art::mirror::Class> owner,jclass java_owner) {
  const char* sig="(Ljava/lang/Object;Ljava/lang/Object;I)Ljava/lang/Object;";
  auto* method=owner->FindClassMethod("jitOsrReferences",sig,art::kRuntimePointerSize);
  jmethodID id=env->GetStaticMethodID(java_owner,"jitOsrReferences",sig);
  jobject a=env->AllocObject(java_owner), b=env->AllocObject(java_owner);
  if(!method || !id || !a || !b || env->ExceptionCheck()) return false;
  jobject warm=env->CallStaticObjectMethod(java_owner,id,a,b,0);
  if(env->ExceptionCheck() || !env->IsSameObject(warm,a)) return false;
  env->DeleteLocalRef(warm);
  if(!jit->CompileMethod(method,self,art::CompilationKind::kOptimized,false) ||
     !jit->CompileMethod(method,self,art::CompilationKind::kOsr,false)) return false;
  unsigned cases=0;
  for(unsigned pair=0;pair<3;++pair) for(uint32_t n:{17u,18u,1000u,1001u}) {
    jobject left=pair==1 ? nullptr:a, right=pair==2 ? nullptr:b;
    uint32_t args[]={
      art::mirror::CompressedReference<art::mirror::Object>::FromMirrorPtr(self->DecodeJObject(left).Ptr()).AsVRegValue(),
      art::mirror::CompressedReference<art::mirror::Object>::FromMirrorPtr(self->DecodeJObject(right).Ptr()).AsVRegValue(),n};
    art::JValue result;
    art::interpreter::EnterInterpreterFromInvoke(self,method,nullptr,args,&result,true);
    if(env->ExceptionCheck() || result.GetL()!=self->DecodeJObject(n%2 ? right:left).Ptr()) {
      std::cerr<<"ART JIT OSR reference mismatch pair="<<pair<<" n="<<n<<"\n"; return false;
    }
    ++cases;
  }
  env->DeleteLocalRef(a); env->DeleteLocalRef(b);
  std::cerr<<"ART JIT OSR reference: swapping live references/null and native return PASS cases="<<cases<<"\n";
  return true;
}
inline bool CheckJitOsrException(JNIEnv* env,art::Thread* self,art::jit::Jit* jit,
                                 art::Handle<art::mirror::Class> owner,jclass java_owner) {
  auto* method=owner->FindClassMethod("jitOsrDivide","(II)I",art::kRuntimePointerSize);
  jmethodID id=env->GetStaticMethodID(java_owner,"jitOsrDivide","(II)I");
  jclass arithmetic=env->FindClass("java/lang/ArithmeticException");
  if(!method || !id || !arithmetic || env->ExceptionCheck()) return false;
  // Resolve before compilation without reaching the throwing iteration.
  if(env->CallStaticIntMethod(java_owner,id,7,0)!=7 || env->ExceptionCheck()) return false;
  if(!jit->CompileMethod(method,self,art::CompilationKind::kOptimized,false) ||
     !jit->CompileMethod(method,self,art::CompilationKind::kOsr,false)) return false;
  auto* original_shadow=self->GetManagedStack()->GetTopShadowFrame();
  for(unsigned cycle=0;cycle<3;++cycle) {
    uint32_t args[]={7,2000}; art::JValue result;
    art::interpreter::EnterInterpreterFromInvoke(self,method,nullptr,args,&result,true);
    if(!env->ExceptionCheck()) return false;
    jthrowable error=env->ExceptionOccurred(); env->ExceptionClear();
    bool correct=env->IsInstanceOf(error,arithmetic); env->DeleteLocalRef(error);
    if(!correct || self->GetManagedStack()->GetTopShadowFrame()!=original_shadow) return false;
    args[1]=17;
    art::interpreter::EnterInterpreterFromInvoke(self,method,nullptr,args,&result,true);
    jint expected=7;
    for(int i=0;i<17;++i) expected+=12345/(1000-i);
    if(env->ExceptionCheck() || result.GetI()!=expected ||
       self->GetManagedStack()->GetTopShadowFrame()!=original_shadow) return false;
  }
  env->DeleteLocalRef(arithmetic);
  std::cerr<<"ART JIT OSR exception: ArithmeticException, shadow restoration and recovery PASS cycles=3\n";
  return true;
}
inline bool CheckJitAutomaticOsr(art::Thread* self,art::jit::Jit* jit,
                                 art::Handle<art::mirror::Class> owner) {
  auto* method=owner->FindClassMethod("jitAutoLoop","(II)I",art::kRuntimePointerSize);
  if(!method || jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
     jit->GetCodeCache()->IsOsrCompiled(method)) return false;
  uint32_t args[]={12345,5000000};
  art::JValue result;
  art::interpreter::EnterInterpreterFromInvoke(self,method,nullptr,args,&result,true);
  uint32_t expected=12345;
  for(uint32_t i=0;i<args[1];++i) expected=(expected*31+i)^(expected>>3);
  bool compiled=jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode());
  bool osr=jit->GetCodeCache()->IsOsrCompiled(method);
  if(self->IsExceptionPending() || static_cast<uint32_t>(result.GetI())!=expected || !compiled || !osr) {
    std::cerr<<"ART JIT automatic OSR failed compiled="<<compiled<<" osr="<<osr<<"\n"; return false;
  }
  std::cerr<<"ART JIT automatic OSR: cold method to background compilation and exact loop result PASS\n";
  return true;
}
inline bool CheckJitOsrWide(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                            art::Handle<art::mirror::Class> owner, jclass java_owner) {
  const char* names[]={"jitLoopJ","jitLoopF","jitLoopD"};
  const char* sigs[]={"(JI)J","(FI)F","(DI)D"};
  for(unsigned kind=0;kind<3;++kind) {
    auto* method=owner->FindClassMethod(names[kind],sigs[kind],art::kRuntimePointerSize);
    jmethodID id=env->GetStaticMethodID(java_owner,names[kind],sigs[kind]);
    if(!method || !id || env->ExceptionCheck()) return false;
    uint64_t expected[3]={}; uint32_t raw[3][3]={};
    for(unsigned sample=0;sample<3;++sample) {
      jvalue args[2]={}; args[1].i=sample==0 ? 17 : 1000;
      if(kind==0) {
        args[0].j=sample==0 ? 0x123456789abcdefLL : sample==1 ? -12345678987654LL : 0;
        std::memcpy(raw[sample],&args[0].j,8); raw[sample][2]=args[1].i;
        jlong r=env->CallStaticLongMethodA(java_owner,id,args); std::memcpy(&expected[sample],&r,8);
      } else if(kind==1) {
        args[0].f=sample==0 ? 1.25f : sample==1 ? -17.5f : std::numeric_limits<float>::infinity();
        std::memcpy(raw[sample],&args[0].f,4); raw[sample][1]=args[1].i;
        jfloat r=env->CallStaticFloatMethodA(java_owner,id,args); std::memcpy(&expected[sample],&r,4);
      } else {
        args[0].d=sample==0 ? 1.25 : sample==1 ? -17.5 : std::numeric_limits<double>::infinity();
        std::memcpy(raw[sample],&args[0].d,8); raw[sample][2]=args[1].i;
        jdouble r=env->CallStaticDoubleMethodA(java_owner,id,args); std::memcpy(&expected[sample],&r,8);
      }
      if(env->ExceptionCheck()) return false;
    }
    if(!jit->CompileMethod(method,self,art::CompilationKind::kOsr,false)) return false;
    for(unsigned sample=0;sample<3;++sample) {
      art::JValue r;
      art::interpreter::EnterInterpreterFromInvoke(self,method,nullptr,raw[sample],&r,true);
      uint64_t actual=0;
      if(kind==1) { float v=r.GetF(); std::memcpy(&actual,&v,4); }
      else { int64_t v=r.GetJ(); std::memcpy(&actual,&v,8); }
      if(self->IsExceptionPending() || actual!=expected[sample]) {
        std::cerr<<"ART JIT OSR wide mismatch "<<names[kind]<<" sample="<<sample
                 <<" expected="<<expected[sample]<<" actual="<<actual<<"\n"; return false;
      }
    }
    std::cerr<<"ART JIT OSR wide: "<<names[kind]<<" exact result bits PASS cases=3\n";
  }
  return true;
}
inline bool CheckJitOsrFrame(art::Thread* self, art::jit::Jit* jit,
                            art::Handle<art::mirror::Class> owner) {
  auto* method=owner->FindClassMethod("jitLoopI","(II)I",art::kRuntimePointerSize);
  if (!method || !jit->CompileMethod(method,self,art::CompilationKind::kOsr,false)) return false;
  art::CodeItemDataAccessor code(*method->GetDexFile(),method->GetCodeItem());
  // Assert fixture layout before synthesizing an interpreter state at loop header.
  if (code.RegistersSize()!=4 || code.InsSize()!=2 ||
      art::Instruction::At(code.Insns()+1)->Opcode()!=art::Instruction::IF_GE) return false;
  const auto* branch=art::Instruction::At(code.Insns()+1);
  if (branch->VRegA_22t()!=0 || branch->VRegB_22t()!=3) return false;
  unsigned cases=0;
  for (uint32_t start : {0u,3u,16u}) {
    uint32_t regs[]={start,0,12345,17};
    auto* data=jit->PrepareForOsr(method,1,regs);
    if (!data) { std::cerr<<"ART JIT OSR frame unavailable\n"; return false; }
    art::JValue result;
    {
      art::ManagedStack fragment;
      self->PushManagedStackFragment(&fragment);
      art::jit::art_quick_osr_stub(data->memory,data->frame_size,data->native_pc,&result,method->GetShorty(),self);
      self->PopManagedStackFragment(fragment);
    }
    free(data);
    uint32_t expected=12345;
    for (uint32_t i=start;i<17;++i) expected=(expected*31+i)^(expected>>3);
    if (self->IsExceptionPending() || static_cast<uint32_t>(result.GetI())!=expected) {
      std::cerr<<"ART JIT OSR result mismatch start="<<start<<"\n"; return false;
    }
    ++cases;
  }
  std::cerr<<"ART JIT OSR: prepared frame and real OSR stub resumed integer loop PASS cases="<<cases<<"\n";
  for (uint32_t seed : {1u,12345u,0xffffffffu}) {
    uint32_t args[]={seed,1000};
    art::JValue result;
    // Skip entry-time compiled dispatch, but retain ordinary branch OSR handling.
    art::interpreter::EnterInterpreterFromInvoke(self,method,nullptr,args,&result,true);
    uint32_t expected=seed;
    for(uint32_t i=0;i<1000;++i) expected=(expected*31+i)^(expected>>3);
    if(self->IsExceptionPending() || static_cast<uint32_t>(result.GetI())!=expected) return false;
  }
  std::cerr<<"ART JIT OSR: interpreter-entry loop results PASS cases=3 (confirm transfer in JIT trace)\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
