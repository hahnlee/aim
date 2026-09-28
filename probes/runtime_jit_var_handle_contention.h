#pragma once
#include <atomic>
#include <chrono>
#include <thread>
namespace aim_jni_acceptance_phase {
inline bool CheckJitVarHandleContention(JNIEnv* env, art::Thread* self, jclass java_owner,
    jobject handle, jobject receiver, jfieldID field, jmethodID add) {
  constexpr int workers = 4, iterations = 512, total = workers * iterations;
  jclass global_owner = static_cast<jclass>(env->NewGlobalRef(java_owner));
  jobject global_handle = env->NewGlobalRef(handle), global_receiver = env->NewGlobalRef(receiver);
  if (!global_owner || !global_handle || !global_receiver || env->ExceptionCheck()) {
    if (global_owner) env->DeleteGlobalRef(global_owner);
    if (global_handle) env->DeleteGlobalRef(global_handle);
    if (global_receiver) env->DeleteGlobalRef(global_receiver);
    return false;
  }
  env->SetIntField(receiver, field, 0);
  jint observed[workers][iterations]{};
  std::atomic<int> ready{0}, active{0}, maximum{0};
  std::atomic<bool> start{false}, ok{true};
  auto run = [&](int worker) {
    auto* runtime = art::Runtime::Current();
    if (!runtime->AttachCurrentThread("jit-varhandle-add", true, nullptr, true)) {
      ok.store(false); ready.fetch_add(1, std::memory_order_release); return;
    }
    JNIEnv* worker_env = art::Thread::Current()->GetJniEnv();
    ready.fetch_add(1, std::memory_order_release);
    while (!start.load(std::memory_order_acquire)) std::this_thread::yield();
    for (int i = 0; i < iterations; ++i) {
      int current = active.fetch_add(1) + 1;
      int previous = maximum.load();
      while (previous < current && !maximum.compare_exchange_weak(previous, current)) {}
      observed[worker][i] = worker_env->CallStaticIntMethod(global_owner, add,
          global_handle, global_receiver, jint(1));
      active.fetch_sub(1);
      if (worker_env->ExceptionCheck()) {
        worker_env->ExceptionDescribe(); worker_env->ExceptionClear(); ok.store(false); break;
      }
    }
    runtime->DetachCurrentThread();
  };
  {
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    std::thread threads[workers];
    for (int i = 0; i < workers; ++i) threads[i] = std::thread(run, i);
    auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
    while (ready.load(std::memory_order_acquire) != workers && std::chrono::steady_clock::now() < deadline)
      std::this_thread::yield();
    if (ready.load(std::memory_order_acquire) != workers) ok.store(false);
    start.store(true, std::memory_order_release);
    for (auto& thread : threads) thread.join();
  }
  bool seen[total]{};
  bool correct = ok.load() && maximum.load() > 1 && env->GetIntField(receiver, field) == total && !env->ExceptionCheck();
  for (const auto& row : observed) for (jint value : row) {
    if (value < 0 || value >= total || seen[value]) correct = false;
    else seen[value] = true;
  }
  for (bool present : seen) if (!present) correct = false;
  env->DeleteGlobalRef(global_receiver); env->DeleteGlobalRef(global_handle); env->DeleteGlobalRef(global_owner);
  std::cerr << "ART JIT VarHandle contention: threads=4 additions=2048 overlapping-calls="
            << maximum.load() << " " << (correct ? "PASS" : "FAIL") << "\n";
  return correct;
}
}  // namespace aim_jni_acceptance_phase
