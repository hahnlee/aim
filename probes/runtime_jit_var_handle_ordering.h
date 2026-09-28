#pragma once
#include <atomic>
#include <chrono>
#include <thread>
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleOrdering(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
    art::Handle<art::mirror::Class> owner, jclass java_owner, jobject handle) {
  const char* names[] = {"jitVarPublish", "jitVarObserve"};
  const char* sigs[] = {
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;IZ)V",
      "(Ljava/lang/invoke/VarHandle;Ldev/aim/probe/Hello;IZ)I"};
  jmethodID ids[2]{};
  art::ArtMethod* methods[2]{};
  for (int i = 0; i < 2; ++i) {
    ids[i] = env->GetStaticMethodID(java_owner, names[i], sigs[i]);
    methods[i] = owner->FindClassMethod(names[i], sigs[i], art::kRuntimePointerSize);
    if (!ids[i] || !methods[i] || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*methods[i]->GetDexFile(), methods[i]->GetCodeItem());
    int calls = 0;
    for (const auto& pair : code) if (pair.Inst().Opcode() == art::Instruction::INVOKE_POLYMORPHIC) ++calls;
    if (calls != 2) return false;
  }
  jclass global_owner = static_cast<jclass>(env->NewGlobalRef(java_owner));
  jobject global_handle = env->NewGlobalRef(handle);
  if (!global_owner || !global_handle || env->ExceptionCheck()) {
    if (global_owner) env->DeleteGlobalRef(global_owner);
    if (global_handle) env->DeleteGlobalRef(global_handle);
    return false;
  }
  bool all_ok = true;
  constexpr int iterations = 1024;
  for (int phase = 0; phase < 3 && all_ok; ++phase) {
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self,
          phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) all_ok = false;
    }
    if (!all_ok) break;
    if (phase) for (int kind = 0; kind < 2; ++kind) {
      const auto* header = art::OatQuickMethodHeader::FromEntryPoint(methods[kind]->GetEntryPointFromQuickCompiledCode());
      // Register fields are the low ten bits: STLR Wt,[Xn] and LDAR Wt,[Xn].
      uint32_t opcode = kind == 0 ? UINT32_C(0x889ffc00) : UINT32_C(0x88dffc00);
      int ordered = 0;
      for (size_t pc = 0; pc + sizeof(uint32_t) <= header->GetCodeSize(); pc += sizeof(uint32_t)) {
        uint32_t instruction;
        memcpy(&instruction, header->GetCode() + pc, sizeof(instruction));
        if ((instruction & UINT32_C(0xfffffc00)) == opcode) ++ordered;
      }
      // Each wrapper has distinct acquire/release and volatile branches.
      if (ordered < 2) {
        std::cerr << "ART JIT VarHandle ordered instruction missing kind=" << kind << " count=" << ordered << "\n";
        all_ok = false;
      }
    }
    if (!all_ok) break;
    for (int mode = 0; mode < 2 && all_ok; ++mode) {
      jobject channels[2]{};
      for (int i = 0; i < 2; ++i) {
        jobject local = env->AllocObject(java_owner);
        if (local) { channels[i] = env->NewGlobalRef(local); env->DeleteLocalRef(local); }
      }
      if (!channels[0] || !channels[1] || env->ExceptionCheck()) {
        for (jobject channel : channels) if (channel) env->DeleteGlobalRef(channel);
        all_ok = false; break;
      }
      std::atomic<int> ready{0};
      std::atomic<bool> start{false}, failed{false};
      int completed[2]{};
      auto run = [&](int actor) {
        auto* runtime = art::Runtime::Current();
        if (!runtime->AttachCurrentThread("jit-varhandle-publication", true, nullptr, true)) {
          failed.store(true); ready.fetch_add(1); return;
        }
        JNIEnv* worker_env = art::Thread::Current()->GetJniEnv();
        ready.fetch_add(1);
        while (!start.load(std::memory_order_acquire)) std::this_thread::yield();
        auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
        auto await = [&](jobject channel, int expected) {
          while (!failed.load(std::memory_order_relaxed)) {
            jint value = worker_env->CallStaticIntMethod(global_owner, ids[1],
                global_handle, channel, jint(expected), jboolean(mode));
            if (worker_env->ExceptionCheck()) return false;
            if (value == expected) return true;
            // A matched signal with a stale payload is failure, not another retry.
            if (value != -1 || std::chrono::steady_clock::now() >= deadline) return false;
            std::this_thread::yield();
          }
          return false;
        };
        for (int value = 1; value <= iterations && !failed.load(std::memory_order_relaxed); ++value) {
          if (!await(channels[actor == 0 ? 1 : 0], actor == 0 ? value - 1 : value)) {
            failed.store(true); break;
          }
          worker_env->CallStaticVoidMethod(global_owner, ids[0], global_handle,
              channels[actor], jint(value), jboolean(mode));
          if (worker_env->ExceptionCheck()) { failed.store(true); break; }
          ++completed[actor];
        }
        if (worker_env->ExceptionCheck()) { worker_env->ExceptionDescribe(); worker_env->ExceptionClear(); }
        runtime->DetachCurrentThread();
      };
      {
        art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
        std::thread producer(run, 0), consumer(run, 1);
        auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
        while (ready.load() != 2 && std::chrono::steady_clock::now() < deadline) std::this_thread::yield();
        if (ready.load() != 2) failed.store(true);
        start.store(true, std::memory_order_release);
        producer.join(); consumer.join();
      }
      all_ok = !failed.load() && completed[0] == iterations && completed[1] == iterations;
      for (jobject channel : channels) env->DeleteGlobalRef(channel);
      if (!all_ok) std::cerr << "ART JIT VarHandle publication failed phase=" << phase << " mode=" << mode << "\n";
    }
  }
  env->DeleteGlobalRef(global_handle); env->DeleteGlobalRef(global_owner);
  if (all_ok) std::cerr << "ART JIT VarHandle publication: release/acquire+volatile handoffs=6144 PASS\n";
  return all_ok;
}
}  // namespace aim_jni_acceptance_phase
