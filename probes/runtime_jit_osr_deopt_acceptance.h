#pragma once
#include "runtime_jit_loop_checkpoint.h"
#include "instrumentation.h"
#include "thread_list.h"
#include "jni/jni_env_ext-inl.h"
#include "jvalue-inl.h"
namespace aim_jni_acceptance_phase {
inline bool CheckJitOsrDeopt(JNIEnv* env,art::Thread* self,art::jit::Jit* jit,
                            art::Handle<art::mirror::Class> owner,jclass java_owner) {
  auto* method=owner->FindClassMethod("jitOsrReferences",
      "(Ljava/lang/Object;Ljava/lang/Object;I)Ljava/lang/Object;",art::kRuntimePointerSize);
  if(!method) return false;
  art::jit::ScopedJitSuspend jit_workers;
  bool removed=false;
  {
    art::ScopedThreadSuspension suspended(self,art::ThreadState::kSuspended);
    art::gc::ScopedGCCriticalSection gc(self,art::gc::kGcCauseInstrumentation,
                                       art::gc::kCollectorTypeInstrumentation);
    art::ScopedSuspendAll all("OSR debug recompilation");
    removed=jit->GetCodeCache()->RemoveMethod(method,true);
  }
  if(!removed) return false;
  jit->GetJitCompiler()->SetDebuggableCompilerOption(true);
  bool compiled=jit->CompileMethod(method,self,art::CompilationKind::kOptimized,false) &&
                jit->CompileMethod(method,self,art::CompilationKind::kOsr,false);
  jit->GetJitCompiler()->SetDebuggableCompilerOption(false);
  if(!compiled) return false;
  const auto* osr=jit->GetCodeCache()->LookupOsrMethodHeader(method);
  if(!osr || !art::CodeInfo::IsDebuggable(osr->GetOptimizedCodeInfoPtr())) return false;
  jobject a=env->AllocObject(java_owner),b=env->AllocObject(java_owner);
  if(!a || !b || env->ExceptionCheck()) return false;
  jweak weak_a=env->NewWeakGlobalRef(a),weak_b=env->NewWeakGlobalRef(b);
  if(!weak_a || !weak_b || env->ExceptionCheck()) return false;
  uint32_t args[]={
    art::mirror::CompressedReference<art::mirror::Object>::FromMirrorPtr(self->DecodeJObject(a).Ptr()).AsVRegValue(),
    art::mirror::CompressedReference<art::mirror::Object>::FromMirrorPtr(self->DecodeJObject(b).Ptr()).AsVRegValue(),100000001};
  LoopCheckpoint checkpoint(method);
  std::atomic<bool> done{false},requested{false};
  std::atomic<uintptr_t> observed{0};
  std::atomic<uintptr_t> old_address{0},new_address{0};
  std::atomic<bool> moved{false};
  auto* runtime=art::Runtime::Current();
  const uint32_t deopts=runtime->GetNumberOfDeoptimizations();
  std::thread worker([&] {
    if(!runtime->AttachCurrentThread("osr-deopt",true,nullptr,false)) return;
    auto* current=art::Thread::Current();
    auto deadline=std::chrono::steady_clock::now()+std::chrono::seconds(10);
    while(!done.load() && std::chrono::steady_clock::now()<deadline) {
      checkpoint.done.store(false,std::memory_order_relaxed);
      bool queued;
      {
        art::MutexLock lock(current,*art::Locks::thread_suspend_count_lock_);
        queued=self->RequestCheckpoint(&checkpoint);
      }
      if(!queued) { std::this_thread::yield(); continue; }
      while(!checkpoint.done.load(std::memory_order_acquire)) std::this_thread::yield();
      uintptr_t pc=checkpoint.native_pc.load();
      if(pc && osr->Contains(pc)) {
        observed.store(pc);
        if(!moved.load()) {
          {
            art::ScopedObjectAccess soa(current);
            old_address.store(reinterpret_cast<uintptr_t>(current->DecodeJObject(weak_b).Ptr()));
          }
          runtime->GetHeap()->CollectGarbage(false);
          {
            art::ScopedObjectAccess soa(current);
            new_address.store(reinterpret_cast<uintptr_t>(current->DecodeJObject(weak_b).Ptr()));
          }
          if(!old_address.load() || !new_address.load() || old_address.load()==new_address.load()) break;
          moved.store(true);
          continue;  // Require a second OSR checkpoint after relocation before deoptimizing.
        }
        {
          art::ScopedSuspendAll all("OSR deoptimization acceptance");
          runtime->GetInstrumentation()->Deoptimize(method);
        }
        requested.store(true);
        break;
      }
    }
    if(requested.load()) {
      while(!done.load()) std::this_thread::yield();
      art::ScopedSuspendAll all("OSR deoptimization cleanup");
      runtime->GetInstrumentation()->Undeoptimize(method);
    }
    runtime->DetachCurrentThread();
  });
  art::JValue result;
  env->DeleteLocalRef(a); env->DeleteLocalRef(b);
  art::interpreter::EnterInterpreterFromInvoke(self,method,nullptr,args,&result,true);
  jobject returned=self->GetJniEnv()->AddLocalReference<jobject>(result.GetL());
  done.store(true);
  {
    art::ScopedThreadStateChange native(self,art::ThreadState::kNative);
    worker.join();
  }
  uint32_t after=runtime->GetNumberOfDeoptimizations();
  bool pass=moved.load() && requested.load() && after>deopts && !env->ExceptionCheck() && returned &&
      !env->IsSameObject(weak_a,nullptr) && env->IsSameObject(returned,weak_b);
  env->DeleteLocalRef(returned); result.SetL(nullptr);
  runtime->GetHeap()->CollectGarbage(false);
  bool reclaimed=env->IsSameObject(weak_a,nullptr) && env->IsSameObject(weak_b,nullptr);
  pass=pass && reclaimed;
  std::cerr<<"ART JIT OSR deopt: pc="<<reinterpret_cast<void*>(observed.load())
           <<" old="<<reinterpret_cast<void*>(old_address.load())<<" new="<<reinterpret_cast<void*>(new_address.load())
           <<" requested="<<requested.load()<<" deopts="<<deopts<<"->"<<after
           <<" reclaimed="<<reclaimed<<" correct="<<pass<<"\n";
  env->DeleteWeakGlobalRef(weak_a); env->DeleteWeakGlobalRef(weak_b);
  return pass;
}
}  // namespace aim_jni_acceptance_phase
