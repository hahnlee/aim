// Diagnostic-only executable: reuse the real GC-cycle handler/ack helpers.
#define main SignalCycleOriginalMain
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wreturn-type"
// Renaming main removes its implicit return; the renamed helper is not called.
#include "signal_cycle_stress.cc"
#pragma clang diagnostic pop
#undef main

#include "sigchain.h"
#include <sys/ucontext.h>

namespace {
AimAndroidPthreadMutex wait_mutex{};
AimAndroidPthreadCond wait_cond{};
unsigned requested_round = 0;
std::atomic<unsigned> completed_round{0}, faults{0};
std::atomic<unsigned> interrupted_mask{0}, active_mask{0};
pthread_t fault_recipient;

bool FaultHandler(int signal, siginfo_t*, void* context) {
  if (signal != SIGBUS) _exit(93);
  if (!pthread_equal(pthread_self(), fault_recipient)) _exit(94);
  sigset_t current = 0;
  pthread_sigmask(SIG_SETMASK, nullptr, &current);
  active_mask.store(current, std::memory_order_relaxed);
  interrupted_mask.store(static_cast<ucontext_t*>(context)->uc_sigmask,
                         std::memory_order_relaxed);
  faults.fetch_add(1, std::memory_order_release);
  return true;
}

void CheckMaskAndPending() {
  sigset_t mask = 0, pending = 0;
  assert(pthread_sigmask(SIG_SETMASK, nullptr, &mask) == 0);
  assert(sigpending(&pending) == 0);
  if (sigismember(&mask, SIGINFO) || sigismember(&mask, SIGXCPU) ||
      sigismember(&pending, SIGINFO) || sigismember(&pending, SIGXCPU))
    bad_mask.store(true, std::memory_order_release);
}

void* ConditionWorker(void*) {
  fault_recipient = pthread_self();
  uint64_t empty = 0;
  assert(aim_bionic_pthread_sigmask(2, &empty, nullptr) == 0);
  assert(aim_bionic_pthread_mutex_lock(&wait_mutex) == 0);
  ready.store(true, std::memory_order_release);
  while (!done.load(std::memory_order_acquire)) {
    while (!done.load(std::memory_order_acquire) &&
           requested_round == completed_round.load())
      assert(aim_bionic_pthread_cond_wait(&wait_cond, &wait_mutex) == 0);
    if (done.load(std::memory_order_acquire)) break;
    CheckMaskAndPending();
    // Real kernel-delivered SIGBUS exercises the unchanged special-handler
    // chain, but is deliberately not claimed to emulate a memory-fault PC.
    assert(pthread_kill(pthread_self(), SIGBUS) == 0);
    CheckMaskAndPending();
    completed_round.store(requested_round, std::memory_order_release);
  }
  assert(aim_bionic_pthread_mutex_unlock(&wait_mutex) == 0);
  return nullptr;
}
}  // namespace

int main() {
  alarm(15);
  sigset_t empty = 0, inherited = 0;
  assert(pthread_sigmask(SIG_SETMASK, &empty, &inherited) == 0);
  std::printf("fault-boundary: inherited-main-mask=%x normalized=0\n", inherited);
  art::SigchainAction special{FaultHandler, 0, 0};
  sigfillset(&special.sc_mask);
  for (int signal : {SIGABRT, SIGBUS, SIGFPE, SIGILL, SIGSEGV})
    sigdelset(&special.sc_mask, signal);
  art::AddSpecialSignalHandlerFn(SIGBUS, &special);
  AndroidAction suspend{0x10000004, reinterpret_cast<void (*)(int)>(Suspend), kHandlerMask, nullptr};
  AndroidAction restart{0x10000000, Restart, kHandlerMask, nullptr};
  assert(aim_bionic_sigaction(30, &suspend, nullptr) == 0);
  assert(aim_bionic_sigaction(24, &restart, nullptr) == 0);
  assert(aim_bionic_sem_init(ack, 0, 0) == 0);
  AimAndroidPthread worker = 0;
  assert(aim_bionic_pthread_create(&worker, nullptr, ConditionWorker, nullptr) == 0);
  assert(Wait([] { return ready.load(std::memory_order_acquire); }));
  for (unsigned cycle = 1; cycle <= 100; ++cycle) {
    // Acquiring this mutex proves the worker released it in CondWait.
    assert(aim_bionic_pthread_mutex_lock(&wait_mutex) == 0);
    assert(aim_bionic_pthread_mutex_unlock(&wait_mutex) == 0);
    assert(aim_bionic_pthread_kill(worker, 30) == 0);
    assert(Wait([] { int value = 0; return aim_bionic_sem_getvalue(ack, &value) == 0 && value == 1; }));
    assert(aim_bionic_sem_trywait(ack) == 0);
    assert(aim_bionic_pthread_kill(worker, 24) == 0);
    assert(Wait([=] { return returned.load() == cycle; }));
    assert(aim_bionic_pthread_mutex_lock(&wait_mutex) == 0);
    requested_round = cycle;
    assert(aim_bionic_pthread_cond_signal(&wait_cond) == 0);
    assert(aim_bionic_pthread_mutex_unlock(&wait_mutex) == 0);
    assert(Wait([=] { return completed_round.load() == cycle; }));
  }
  assert(aim_bionic_pthread_mutex_lock(&wait_mutex) == 0);
  done.store(true);
  assert(aim_bionic_pthread_cond_signal(&wait_cond) == 0);
  assert(aim_bionic_pthread_mutex_unlock(&wait_mutex) == 0);
  assert(aim_bionic_pthread_join(worker, nullptr) == 0);
  assert(!bad_mask.load() && faults.load() == 100);
  std::printf("fault-boundary: PASS condwait-gc=100 kernel-SIGBUS=100 mask-restored=yes pending=0 special-mask=%x interrupted=%x\n",
              active_mask.load(), interrupted_mask.load());

  // Call the installed dispatcher as a function, with no kernel sigreturn.
  // Measure (and immediately undo) its mask effect without changing runtime.
  struct sigaction dispatcher{};
  assert(sigaction(SIGBUS, nullptr, &dispatcher) == 0);
  sigset_t before = 0, after = 0;
  sigset_t before_reset = 0;
  pthread_sigmask(SIG_SETMASK, &empty, &before_reset);
  assert(before_reset == 0);  // A worker's private handler mask cannot leak here.
  pthread_sigmask(SIG_SETMASK, nullptr, &before);
  std::printf("fault-boundary: observer-before-direct-reset=%x\n", before_reset);
  _STRUCT_MCONTEXT64 machine{};
  ucontext_t context{};
  context.uc_sigmask = before;
  context.uc_mcontext = &machine;
  siginfo_t info{};
  fault_recipient = pthread_self();
  dispatcher.sa_sigaction(SIGBUS, &info, &context);
  pthread_sigmask(SIG_SETMASK, nullptr, &after);
  pthread_sigmask(SIG_SETMASK, &before, nullptr);
  std::printf("fault-boundary: ordinary-call before=%x after=%x changed=%d (not proof of Unity call path)\n",
              before, after, before != after);
  art::RemoveSpecialSignalHandlerFn(SIGBUS, FaultHandler);
  return 0;
}
