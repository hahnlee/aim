#pragma once
#include "runtime_jit_loop_checkpoint.h"
#include "jni/jni_env_ext-inl.h"
#include "jvalue-inl.h"
namespace aim_jni_acceptance_phase {
inline bool CheckJitOsrGc(JNIEnv* env,art::Thread* self,art::jit::Jit* jit,
                          art::Handle<art::mirror::Class> owner,jclass java_owner,bool require_move=false) {
  auto* method=owner->FindClassMethod("jitOsrReferences",
      "(Ljava/lang/Object;Ljava/lang/Object;I)Ljava/lang/Object;",art::kRuntimePointerSize);
  if(!method) return false;
  const auto* osr_header=jit->GetCodeCache()->LookupOsrMethodHeader(method);
  if(!osr_header) return false;
  jobject a=env->AllocObject(java_owner),b=env->AllocObject(java_owner);
  if(!a || !b || env->ExceptionCheck()) return false;
  jweak weak_a=env->NewWeakGlobalRef(a),weak_b=env->NewWeakGlobalRef(b);
  if(!weak_a || !weak_b || env->ExceptionCheck()) return false;
  constexpr uint32_t iterations=500000001;
  uint32_t args[]={
    art::mirror::CompressedReference<art::mirror::Object>::FromMirrorPtr(self->DecodeJObject(a).Ptr()).AsVRegValue(),
    art::mirror::CompressedReference<art::mirror::Object>::FromMirrorPtr(self->DecodeJObject(b).Ptr()).AsVRegValue(),iterations};
  LoopCheckpoint checkpoint(method);
  std::atomic<bool> call_done{false},ok{false};
  std::atomic<uintptr_t> before{0},after{0};
  std::atomic<uint64_t> collections{0};
  std::atomic<uintptr_t> old_address{0},new_address{0};
  std::atomic<bool> compact_ok{false};
  std::thread worker([&] {
    auto* runtime=art::Runtime::Current();
    if(!runtime->AttachCurrentThread("osr-gc-checkpoint",true,nullptr,false)) return;
    auto* current=art::Thread::Current();
    auto deadline=std::chrono::steady_clock::now()+std::chrono::seconds(10);
    auto observe=[&]() -> uintptr_t {
      while(!call_done.load() && std::chrono::steady_clock::now()<deadline) {
        checkpoint.done.store(false,std::memory_order_relaxed);
        bool queued;
        {
          art::MutexLock lock(current,*art::Locks::thread_suspend_count_lock_);
          queued=self->RequestCheckpoint(&checkpoint);
        }
        if(!queued) { std::this_thread::yield(); continue; }
        // A queued closure must finish before reuse/destruction, including after return.
        while(!checkpoint.done.load(std::memory_order_acquire)) std::this_thread::yield();
        uintptr_t pc=checkpoint.native_pc.load();
        if(pc && osr_header->Contains(pc)) return pc;
      }
      return 0;
    };
    before.store(observe());
    if(before.load()) {
      uint64_t count=runtime->GetHeap()->GetGcCount();
      if(require_move) {
        {
          art::ScopedObjectAccess soa(current);
          old_address.store(reinterpret_cast<uintptr_t>(current->DecodeJObject(weak_b).Ptr()));
        }
        // CC is the configured AOSP moving collector. A homogeneous-space
        // transition is only supported by CMS and is rejected under CC.
        runtime->GetHeap()->CollectGarbage(false);
        {
          art::ScopedObjectAccess soa(current);
          new_address.store(reinterpret_cast<uintptr_t>(current->DecodeJObject(weak_b).Ptr()));
        }
        compact_ok.store(old_address.load()!=0 && new_address.load()!=0 &&
                         old_address.load()!=new_address.load());
      } else {
        runtime->GetHeap()->CollectGarbage(false);
      }
      collections.store(runtime->GetHeap()->GetGcCount()-count);
      after.store(observe());
      ok.store(after.load()!=0 && (require_move ? compact_ok.load() : collections.load()>0));
    }
    runtime->DetachCurrentThread();
  });
  art::JValue result;
  // No suspension between dropping JNI roots and installing the interpreter frame.
  env->DeleteLocalRef(a); env->DeleteLocalRef(b);
  art::interpreter::EnterInterpreterFromInvoke(self,method,nullptr,args,&result,true);
  // Root the returned object before the Native transition used to join the worker.
  jobject returned=self->GetJniEnv()->AddLocalReference<jobject>(result.GetL());
  call_done.store(true);
  {
    art::ScopedThreadStateChange native(self,art::ThreadState::kNative);
    worker.join();
  }
  bool survived=!env->IsSameObject(weak_a,nullptr) && !env->IsSameObject(weak_b,nullptr);
  bool pass=ok.load() && !env->ExceptionCheck() && returned && survived && env->IsSameObject(returned,weak_b);
  env->DeleteLocalRef(returned);
  result.SetL(nullptr);
  art::Runtime::Current()->GetHeap()->CollectGarbage(false);
  bool reclaimed=env->IsSameObject(weak_a,nullptr) && env->IsSameObject(weak_b,nullptr);
  pass=pass && reclaimed;
  std::cerr<<"ART JIT OSR GC: require_move="<<require_move<<" old="<<reinterpret_cast<void*>(old_address.load())
           <<" new="<<reinterpret_cast<void*>(new_address.load())<<" before="<<reinterpret_cast<void*>(before.load())
           <<" after="<<reinterpret_cast<void*>(after.load())<<" collections="<<collections.load()
           <<" survived="<<survived<<" reclaimed="<<reclaimed<<" correct="<<pass<<"\n";
  env->DeleteWeakGlobalRef(weak_a); env->DeleteWeakGlobalRef(weak_b);
  return pass;
}
}  // namespace aim_jni_acceptance_phase
