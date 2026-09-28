#include <signal.h>
#include <pthread.h>
#include <array>
#include <cstdint>
#include <cstring>

#if defined(__APPLE__) && defined(__aarch64__)
#include <mach/arm/thread_status.h>
#include <sys/ucontext.h>
#endif

#include "sigchain.h"

extern "C" int aim_bionic_process_state_recover_runtime_signal(
    int host_signal, void* host_info, void* host_context);

namespace art {
namespace {

struct SignalSlot {
  std::array<SigchainAction, 2> specials{};
  struct sigaction previous {};
  bool installed = false;
};

std::array<SignalSlot, NSIG> g_signal_slots;
thread_local std::array<sig_atomic_t, NSIG> g_handling_signal{};

void DarwinSignalDispatcher(int signal_number, siginfo_t* info, void* context);

#if defined(__APPLE__) && defined(__aarch64__)
uintptr_t GetInterruptedPc(void* context) {
  if (context == nullptr) return 0u;
  auto* ucontext = static_cast<ucontext_t*>(context);
  return arm_thread_state64_get_pc(ucontext->uc_mcontext->__ss);
}

void NormalizeAndroidHandlerPc(void* context, uintptr_t interrupted_pc) {
  if (context == nullptr || interrupted_pc == 0u) return;
  auto* ucontext = static_cast<ucontext_t*>(context);
  arm_thread_state64_t& state = ucontext->uc_mcontext->__ss;
  const uintptr_t resumed_pc = arm_thread_state64_get_pc(state);

  // Android aarch64 signal handlers update an inline mcontext_t::pc field.
  // Darwin exposes that field through an indirect host ucontext and, for a
  // compatibility-compiled Android handler, can retain only the low 32 bits
  // after an in-place PC increment. Preserve the handler's requested offset
  // while restoring the interrupted address window. This is the sigreturn ABI
  // boundary; doing it here keeps native bridge and application handlers
  // source-identical and does not treat any individual fault address specially.
  constexpr uintptr_t kLowAddressMask = 0xffffffffu;
  if ((interrupted_pc & ~kLowAddressMask) != 0u &&
      (resumed_pc & ~kLowAddressMask) == 0u) {
    const uintptr_t normalized_pc =
        (interrupted_pc & ~kLowAddressMask) | (resumed_pc & kLowAddressMask);
    using ProgramCounter = void (*)();
    arm_thread_state64_set_pc_fptr(
        state, reinterpret_cast<ProgramCounter>(normalized_pc));
  }
}
#endif

struct sigaction DispatcherAction() {
  struct sigaction dispatcher {};
  dispatcher.sa_sigaction = DarwinSignalDispatcher;
  sigfillset(&dispatcher.sa_mask);
  dispatcher.sa_flags = SA_SIGINFO | SA_RESTART | SA_ONSTACK;
  return dispatcher;
}

bool DispatchUserHandler(int signal_number, siginfo_t* info, void* context) {
  SignalSlot& slot = g_signal_slots[signal_number];
  struct sigaction action = slot.previous;
  const ucontext_t* ucontext = static_cast<const ucontext_t*>(context);
  sigset_t interrupted_mask;
  const bool have_interrupted_mask = ucontext != nullptr;
  if (have_interrupted_mask) {
    interrupted_mask = ucontext->uc_sigmask;
  }
  const auto restore_interrupted_mask = [&]() {
    if (have_interrupted_mask) {
      pthread_sigmask(SIG_SETMASK, &interrupted_mask, nullptr);
    }
  };
  // Match AOSP sigchain: special handlers run with ART's private mask, but
  // the chained application handler observes the mask that was active at the
  // fault plus its own sa_mask (and the delivered signal unless SA_NODEFER).
  // In particular this lets an explicitly unblocked signal raised by the
  // application handler run synchronously instead of inheriting ART's mask.
  if (action.sa_handler != SIG_IGN && action.sa_handler != SIG_DFL &&
      action.sa_handler != nullptr) {
    sigset_t handler_mask;
    sigemptyset(&handler_mask);
    for (int candidate = 1; candidate < NSIG; ++candidate) {
      const bool was_blocked =
          ucontext != nullptr && sigismember(&ucontext->uc_sigmask, candidate) == 1;
      const bool requested = sigismember(&action.sa_mask, candidate) == 1;
      if (was_blocked || requested) sigaddset(&handler_mask, candidate);
    }
    if ((action.sa_flags & SA_NODEFER) == 0) {
      sigaddset(&handler_mask, signal_number);
    }
    pthread_sigmask(SIG_SETMASK, &handler_mask, nullptr);
  }
  if ((action.sa_flags & SA_SIGINFO) != 0 && action.sa_sigaction != nullptr) {
    if ((action.sa_flags & SA_RESETHAND) != 0) {
      slot.previous = {};
    }
    action.sa_sigaction(signal_number, info, context);
    // The application handler is called as an ordinary function by the
    // Darwin chain (including from the Mach SIGBUS-to-SIGSEGV bridge), so
    // there is no kernel sigreturn at this boundary to restore the mask that
    // was active at the fault. Recreate that sigreturn behavior explicitly.
    restore_interrupted_mask();
    return true;
  }
  if (action.sa_handler == SIG_IGN) {
    return true;
  }
  if (action.sa_handler != nullptr && action.sa_handler != SIG_DFL) {
    if ((action.sa_flags & SA_RESETHAND) != 0) {
      slot.previous = {};
    }
    action.sa_handler(signal_number);
    restore_interrupted_mask();
    return true;
  }
  return false;
}

void DarwinSignalDispatcher(int signal_number, siginfo_t* info, void* context) {
  // Resolve compatibility-internal Android RWX JIT page transitions before
  // ART examines the signal. On Android these mappings are genuinely RWX and
  // never enter sigchain; Darwin's W^X implementation deliberately produces
  // a protection fault to select the page's next mode. Letting ART inspect it
  // first both adds avoidable work and misclassifies a recovered V8 transition
  // as an unhandled ART generated-code fault.
  if (aim_bionic_process_state_recover_runtime_signal(
          signal_number, info, context) != 0) {
    return;
  }
  SignalSlot& slot = g_signal_slots[signal_number];
  if (g_handling_signal[signal_number] == 0) {
    for (const SigchainAction& special : slot.specials) {
      if (special.sc_sigaction == nullptr) break;
      const bool may_not_return =
          (special.sc_flags & SIGCHAIN_ALLOW_NORETURN) != 0;
      sigset_t previous_mask;
      // Darwin sigprocmask updates every thread's mask. A signal handler's
      // private mask must affect only the interrupted thread, as on Android.
      pthread_sigmask(SIG_SETMASK, &special.sc_mask, &previous_mask);
      if (!may_not_return) g_handling_signal[signal_number] = 1;
#if defined(__APPLE__) && defined(__aarch64__)
      const uintptr_t interrupted_pc = GetInterruptedPc(context);
#endif
      const bool handled = special.sc_sigaction(signal_number, info, context);
#if defined(__APPLE__) && defined(__aarch64__)
      if (handled) NormalizeAndroidHandlerPc(context, interrupted_pc);
#endif
      if (!may_not_return) g_handling_signal[signal_number] = 0;
      if (handled) return;
      pthread_sigmask(SIG_SETMASK, &previous_mask, nullptr);
    }
  }

  if (DispatchUserHandler(signal_number, info, context)) return;
  signal(signal_number, SIG_DFL);
  raise(signal_number);
}

}  // namespace

extern "C" void AddSpecialSignalHandlerFn(int signal_number, SigchainAction* action) {
  if (signal_number <= 0 || signal_number >= NSIG || action == nullptr) {
    return;
  }
  SignalSlot& slot = g_signal_slots[signal_number];
  for (SigchainAction& special : slot.specials) {
    if (special.sc_sigaction != nullptr) continue;
    if (!slot.installed) {
      struct sigaction dispatcher = DispatcherAction();
      if (sigaction(signal_number, &dispatcher, &slot.previous) != 0) return;
      slot.installed = true;
    }
    special = *action;
    return;
  }
}

extern "C" void RemoveSpecialSignalHandlerFn(
    int signal_number, bool (*handler)(int, siginfo_t*, void*)) {
  if (signal_number <= 0 || signal_number >= NSIG) {
    return;
  }
  SignalSlot& slot = g_signal_slots[signal_number];
  if (!slot.installed) return;
  for (size_t index = 0; index < slot.specials.size(); ++index) {
    if (slot.specials[index].sc_sigaction != handler) continue;
    for (size_t next = index + 1; next < slot.specials.size(); ++next) {
      slot.specials[next - 1] = slot.specials[next];
    }
    slot.specials.back() = {};
    if (slot.specials.front().sc_sigaction == nullptr) {
      sigaction(signal_number, &slot.previous, nullptr);
      slot = {};
    }
    return;
  }
}

extern "C" __attribute__((visibility("default"))) void EnsureFrontOfChain(
    int signal_number) {
  if (signal_number <= 0 || signal_number >= NSIG) {
    return;
  }
  SignalSlot& slot = g_signal_slots[signal_number];
  if (!slot.installed) {
    return;
  }
  struct sigaction current {};
  if (sigaction(signal_number, nullptr, &current) != 0) {
    return;
  }
  const bool dispatcher_is_current =
      (current.sa_flags & SA_SIGINFO) != 0 &&
      current.sa_sigaction == DarwinSignalDispatcher;
  if (dispatcher_is_current) {
    return;
  }
  // Match AOSP sigchain's ownership repair: retain an application-installed
  // replacement as the next action, then put ART's dispatcher back in front.
  slot.previous = current;
  struct sigaction dispatcher = DispatcherAction();
  sigaction(signal_number, &dispatcher, nullptr);
}

extern "C" __attribute__((visibility("default"))) int
aim_sigchain_sigaction(int signal_number,
                              const struct sigaction* action,
                              struct sigaction* old_action) {
  if (signal_number <= 0 || signal_number >= NSIG) {
    return sigaction(signal_number, action, old_action);
  }
  SignalSlot& slot = g_signal_slots[signal_number];
  if (!slot.installed) {
    return sigaction(signal_number, action, old_action);
  }
  if (old_action != nullptr) {
    *old_action = slot.previous;
  }
  if (action == nullptr) {
    return 0;
  }
  slot.previous = *action;
  struct sigaction dispatcher = DispatcherAction();
  return sigaction(signal_number, &dispatcher, nullptr);
}

extern "C" __attribute__((visibility("default"))) int
aim_sigchain_owns_signal(int signal_number) {
  return signal_number > 0 && signal_number < NSIG &&
                 g_signal_slots[signal_number].installed
             ? 1
             : 0;
}

extern "C" __attribute__((visibility("default"))) bool
aim_sigchain_dispatch_user(int signal_number, siginfo_t* info, void* context) {
  if (signal_number <= 0 || signal_number >= NSIG) return false;
  return DispatchUserHandler(signal_number, info, context);
}

}  // namespace art
