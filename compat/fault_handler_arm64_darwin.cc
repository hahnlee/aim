#include "fault_handler.h"

#include <mach/arm/thread_status.h>
#include <sys/ucontext.h>

#include "arch/instruction_set.h"
#include "art_method.h"
#include "base/logging.h"
#include "base/pointer_size.h"
#include "darwin_jit_memory.h"
#include "runtime_globals.h"
#include "thread-current-inl.h"

extern "C" void art_quick_throw_stack_overflow();
extern "C" void art_quick_throw_null_pointer_exception_from_signal();
extern "C" void art_quick_implicit_suspend();

namespace art {
namespace {

mcontext_t MachineContext(void* context) {
  return reinterpret_cast<ucontext_t*>(context)->uc_mcontext;
}

uintptr_t GetPc(mcontext_t context) {
  return arm_thread_state64_get_pc(context->__ss);
}

uintptr_t GetSp(mcontext_t context) {
  return arm_thread_state64_get_sp(context->__ss);
}

void SetPc(mcontext_t context, void (*entrypoint)()) {
  arm_thread_state64_set_pc_fptr(context->__ss, entrypoint);
}

void SetSp(mcontext_t context, uintptr_t stack_pointer) {
  arm_thread_state64_set_sp(context->__ss, stack_pointer);
}

void SetLr(mcontext_t context, uintptr_t value) {
  arm_thread_state64_set_lr_fptr(context->__ss, reinterpret_cast<void (*)()>(value));
}

}  // namespace

uintptr_t FaultManager::GetFaultPc(siginfo_t* info, void* context) {
#ifdef SEGV_MTEAERR
  if (info->si_signo == SIGSEGV && info->si_code == SEGV_MTEAERR) {
    return 0u;
  }
#else
  (void)info;
#endif
  mcontext_t machine_context = MachineContext(context);
  return GetSp(machine_context) == 0u ? 0u : GetPc(machine_context);
}

uintptr_t FaultManager::GetFaultSp(void* context) {
  return GetSp(MachineContext(context));
}

bool NullPointerHandler::Action(int, siginfo_t* info, void* context) {
  uintptr_t fault_address = reinterpret_cast<uintptr_t>(info->si_addr);
  // Managed references retain Android's low-32-bit value on Darwin, while
  // generated memory operands decode them into the reserved high host window.
  // A null receiver therefore faults at heap-base + field-offset rather than
  // literal field-offset. Feed ART's ordinary implicit-null predicate the
  // logical Android address. The quick throw entrypoint forwards this value
  // to ThrowNullPointerExceptionFromDexPC(), whose AOSP validation applies
  // the same low-page implicit-null contract.
  uintptr_t logical_fault_address =
      ArtHostAddressToCompressedReferenceAddress(fault_address);
  if (!IsValidFaultAddress(logical_fault_address)) {
    return false;
  }

  mcontext_t machine_context = MachineContext(context);
  const uintptr_t published_method =
      AimLookupJitMethod(GetPc(machine_context));
  // ART's implicit-null handler is process-global on Darwin, but Android ELF
  // libraries (for example Chromium) also fault in the same process.  Never
  // reinterpret a native image PC as a quick frame merely because an
  // arbitrary native stack word happens to look like an ArtMethod.  Only code
  // ranges explicitly published by ART's JIT/AOT paths may enter this
  // exception transformation.
  if (!AimLookupJitCode(GetPc(machine_context))) {
    return false;
  }
  uintptr_t stack_pointer = GetSp(machine_context);
  ArtMethod** stack = reinterpret_cast<ArtMethod**>(stack_pointer);
  uintptr_t return_pc = GetPc(machine_context) + 4u;
  ArtMethod* method = IsValidMethod(*stack) ? *stack : nullptr;
  if (method == nullptr) {
    method = reinterpret_cast<ArtMethod*>(published_method);
    if (method != nullptr) {
      // The signal throw stub walks the interrupted quick frame through this
      // slot. Publish the recovered method in-place before changing SP.
      *stack = method;
    }
  }
  if (!IsValidMethod(method) || !IsValidReturnPc(stack, return_pc)) {
    return false;
  }

  stack_pointer -= sizeof(uintptr_t);
  *reinterpret_cast<uintptr_t*>(stack_pointer) = return_pc;
  SetSp(machine_context, stack_pointer);
  SetLr(machine_context, logical_fault_address);
  SetPc(machine_context, art_quick_throw_null_pointer_exception_from_signal);
  return true;
}

bool SuspensionHandler::Action(int, siginfo_t*, void* context) {
  constexpr uint32_t kSuspendCheckRegister = 21;
  constexpr uint32_t kSuspendCheck =
      0xf9400000 | (kSuspendCheckRegister << 5) | kSuspendCheckRegister;

  mcontext_t machine_context = MachineContext(context);
  uintptr_t program_counter = GetPc(machine_context);
  if (*reinterpret_cast<uint32_t*>(program_counter) != kSuspendCheck) {
    return false;
  }

  SetLr(machine_context, program_counter + 4u);
  SetPc(machine_context, art_quick_implicit_suspend);
  Thread::Current()->RemoveSuspendTrigger();
  return true;
}

bool StackOverflowHandler::Action(int, siginfo_t*, void* context) {
  mcontext_t machine_context = MachineContext(context);
  uintptr_t stack_pointer = GetSp(machine_context);
  uintptr_t fault_address = machine_context->__es.__far;
  uintptr_t overflow_address =
      stack_pointer - GetStackOverflowReservedBytes(InstructionSet::kArm64);
  if (fault_address != overflow_address) {
    return false;
  }
  SetPc(machine_context, art_quick_throw_stack_overflow);
  return true;
}

}  // namespace art
