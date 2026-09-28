#include <setjmp.h>
#include <signal.h>

#include <cstdio>

#include "sigchain.h"

extern "C" int aim_sigchain_sigaction(
    int signal_number, const struct sigaction* action, struct sigaction* old_action);
extern "C" bool aim_sigchain_dispatch_user(
    int signal_number, siginfo_t* info, void* context);

namespace {

volatile sig_atomic_t g_a_count;
volatile sig_atomic_t g_b_count;
volatile sig_atomic_t g_user_count;
volatile sig_atomic_t g_new_user_count;
volatile sig_atomic_t g_a_handles;
volatile sig_atomic_t g_b_handles;
volatile sig_atomic_t g_mask_user_count;
sigjmp_buf g_nested_jump;

bool HandlerA(int, siginfo_t*, void*) {
  ++g_a_count;
  return g_a_handles != 0;
}

bool HandlerB(int, siginfo_t*, void*) {
  ++g_b_count;
  return g_b_handles != 0;
}

void UserHandler(int) { ++g_user_count; }
void NewUserHandler(int) { ++g_new_user_count; }

void MaskChangingUserHandler(int) {
  ++g_mask_user_count;
  sigset_t blocked;
  sigemptyset(&blocked);
  sigaddset(&blocked, SIGALRM);
  sigprocmask(SIG_BLOCK, &blocked, nullptr);
}

bool NoReturnHandler(int signal_number, siginfo_t*, void*) {
  ++g_b_count;
  if (g_b_count == 1) {
    sigset_t unblocked;
    sigemptyset(&unblocked);
    sigprocmask(SIG_SETMASK, &unblocked, nullptr);
    raise(signal_number);
  }
  siglongjmp(g_nested_jump, 1);
}

art::SigchainAction MakeAction(
    bool (*handler)(int, siginfo_t*, void*), uint64_t flags = 0) {
  art::SigchainAction action{};
  action.sc_sigaction = handler;
  sigemptyset(&action.sc_mask);
  action.sc_flags = flags;
  return action;
}

bool InstallUserHandler(int signal_number, void (*handler)(int)) {
  struct sigaction action {};
  action.sa_handler = handler;
  sigemptyset(&action.sa_mask);
  return sigaction(signal_number, &action, nullptr) == 0;
}

}  // namespace

int main() {
  constexpr int kOrderedSignal = SIGUSR1;
  if (!InstallUserHandler(kOrderedSignal, UserHandler)) return 1;
  art::SigchainAction action_a = MakeAction(HandlerA);
  art::SigchainAction action_b = MakeAction(HandlerB);
  art::AddSpecialSignalHandlerFn(kOrderedSignal, &action_a);
  art::AddSpecialSignalHandlerFn(kOrderedSignal, &action_b);

  g_a_handles = 1;
  raise(kOrderedSignal);
  if (g_a_count != 1 || g_b_count != 0 || g_user_count != 0) return 2;

  g_a_handles = 0;
  g_b_handles = 1;
  raise(kOrderedSignal);
  if (g_a_count != 2 || g_b_count != 1 || g_user_count != 0) return 3;

  g_b_handles = 0;
  raise(kOrderedSignal);
  if (g_a_count != 3 || g_b_count != 2 || g_user_count != 1) return 4;

  art::RemoveSpecialSignalHandlerFn(kOrderedSignal, HandlerA);
  g_b_handles = 1;
  raise(kOrderedSignal);
  if (g_a_count != 3 || g_b_count != 3) return 5;

  struct sigaction replacement {};
  replacement.sa_handler = NewUserHandler;
  sigemptyset(&replacement.sa_mask);
  if (aim_sigchain_sigaction(
          kOrderedSignal, &replacement, nullptr) != 0) return 6;
  g_b_handles = 0;
  raise(kOrderedSignal);
  if (g_new_user_count != 1) return 7;
  art::EnsureFrontOfChain(kOrderedSignal);
  art::RemoveSpecialSignalHandlerFn(kOrderedSignal, HandlerB);

  constexpr int kNestedSignal = SIGUSR2;
  if (!InstallUserHandler(kNestedSignal, UserHandler)) return 8;
  art::SigchainAction no_return =
      MakeAction(NoReturnHandler, art::SIGCHAIN_ALLOW_NORETURN);
  g_b_count = 0;
  if (sigsetjmp(g_nested_jump, 1) == 0) {
    art::AddSpecialSignalHandlerFn(kNestedSignal, &no_return);
    raise(kNestedSignal);
    return 9;
  }
  if (g_b_count != 2) return 10;
  art::RemoveSpecialSignalHandlerFn(kNestedSignal, NoReturnHandler);

  // The Mach SIGBUS bridge invokes a chained SIGSEGV action as an ordinary
  // function, without a kernel sigreturn boundary. Verify that the generic
  // direct-dispatch path still restores the mask from the fault context.
  constexpr int kBridgeSignal = SIGTERM;
  sigset_t original_mask;
  sigprocmask(SIG_SETMASK, nullptr, &original_mask);
  sigset_t fault_mask = original_mask;
  sigdelset(&fault_mask, SIGALRM);
  sigprocmask(SIG_SETMASK, &fault_mask, nullptr);
  if (!InstallUserHandler(kBridgeSignal, MaskChangingUserHandler)) return 11;
  art::SigchainAction bridge_special = MakeAction(HandlerA);
  art::AddSpecialSignalHandlerFn(kBridgeSignal, &bridge_special);
  ucontext_t context {};
  context.uc_sigmask = fault_mask;
  if (!aim_sigchain_dispatch_user(kBridgeSignal, nullptr, &context)) return 12;
  sigset_t after_dispatch;
  sigprocmask(SIG_SETMASK, nullptr, &after_dispatch);
  if (g_mask_user_count != 1 || sigismember(&after_dispatch, SIGALRM) != 0) return 13;
  art::RemoveSpecialSignalHandlerFn(kBridgeSignal, HandlerA);
  sigprocmask(SIG_SETMASK, &original_mask, nullptr);

  std::puts(
      "darwin-sigchain: ordered=pass user=pass nested-noreturn=pass bridge-mask=pass");
  return 0;
}
