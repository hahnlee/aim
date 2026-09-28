#pragma once

#include <signal.h>
#include <sys/ucontext.h>
#include <unistd.h>

#include <array>

extern "C" int aim_sigchain_sigaction(
    int signal, const struct sigaction* action, struct sigaction* old_action);

// Bionic aarch64 exposes uc_mcontext inline and names the program-counter
// field `pc`; Darwin stores a pointer to an ARM thread-state whose field is
// `__pc`. This source-compatibility view lets upstream native ART diagnostics
// use their Android spelling while the actual signal frame remains owned by
// Darwin. It is injected only for native sources that consume that ABI.
#if defined(__APPLE__) && defined(__aarch64__)
#define uc_mcontext uc_mcontext->__ss
#define mcontext_t _STRUCT_ARM_THREAD_STATE64
#define pc __pc
#endif

// Darwin silently clears unmaskable/reserved signal bits from sa_mask when an
// action is queried. Bionic exposes the logical mask supplied by the caller.
// Preserve that API view while the kernel still receives Darwin's legal mask.
inline int aim_test_sigaction(int signal,
                                     const struct sigaction* action,
                                     struct sigaction* old_action) {
  static std::array<struct sigaction, NSIG> logical_actions{};
  static std::array<bool, NSIG> installed{};
  const int result = aim_sigchain_sigaction(signal, action, old_action);
  if (result != 0 || signal <= 0 || signal >= NSIG) return result;
  if (action != nullptr) {
    logical_actions[signal] = *action;
    installed[signal] = true;
  } else if (old_action != nullptr && installed[signal]) {
    old_action->sa_mask = logical_actions[signal].sa_mask;
  }
  return result;
}

#define sigaction(...) aim_test_sigaction(__VA_ARGS__)

#if defined(AIM_TEST_SYNCHRONOUS_SELF_KILL)
// Darwin may defer process-directed self signals until a later safe point,
// while this Bionic NativeBridge callback contract requires delivery before
// kill() returns. A thread-directed raise() supplies that ordering.
inline int aim_test_kill(pid_t process, int signal) {
  return process == getpid() ? ::raise(signal) : ::kill(process, signal);
}

#define kill(...) aim_test_kill(__VA_ARGS__)
#endif
