#pragma once

#if defined(__APPLE__)

#include <errno.h>
#include <mach/arm/thread_status.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <signal.h>
#include <pthread.h>
#include <sys/ptrace.h>
#include <sys/uio.h>
#include <ucontext.h>
#include <unistd.h>

#include "darwin_remote_task_cache.h"

#include <algorithm>
#include <cstdint>
#include <cstring>

#ifndef SIGRTMIN
#define SIGRTMIN SIGUSR2
#endif

#ifndef TEMP_FAILURE_RETRY
#define TEMP_FAILURE_RETRY(expression)                  \
  ({                                                    \
    decltype(expression) aim_result;             \
    do {                                                \
      aim_result = (expression);                 \
    } while (aim_result == -1 && errno == EINTR); \
    aim_result;                                  \
  })
#endif

#ifndef PTRACE_PEEKTEXT
#define PTRACE_PEEKTEXT 1
#endif
#ifndef PTRACE_GETREGSET
#define PTRACE_GETREGSET 0x4204
#endif
#ifndef NT_PRSTATUS
#define NT_PRSTATUS 1
#endif

namespace aim::unwindstack_abi {

inline int PageSize() {
  const long value = sysconf(_SC_PAGESIZE);
  return value > 0 ? static_cast<int>(value) : 16384;
}

inline mach_port_t TaskForPid(pid_t pid) {
  return aim::remote_task::Acquire(pid);
}

inline thread_act_t FindThread(mach_port_t task, uint64_t requested_id) {
  thread_act_array_t threads = nullptr;
  mach_msg_type_number_t count = 0;
  if (task_threads(task, &threads, &count) != KERN_SUCCESS || count == 0) return MACH_PORT_NULL;
  thread_act_t found = MACH_PORT_NULL;
  for (mach_msg_type_number_t index = 0; index < count; ++index) {
    thread_identifier_info_data_t identifier{};
    mach_msg_type_number_t info_count = THREAD_IDENTIFIER_INFO_COUNT;
    if (thread_info(threads[index], THREAD_IDENTIFIER_INFO,
                    reinterpret_cast<thread_info_t>(&identifier), &info_count) == KERN_SUCCESS &&
        (requested_id == 0 || identifier.thread_id == requested_id)) {
      found = threads[index];
      mach_port_mod_refs(mach_task_self(), found, MACH_PORT_RIGHT_SEND, 1);
      break;
    }
  }
  for (mach_msg_type_number_t index = 0; index < count; ++index) {
    mach_port_deallocate(mach_task_self(), threads[index]);
  }
  vm_deallocate(mach_task_self(), reinterpret_cast<vm_address_t>(threads),
                count * sizeof(thread_act_t));
  return found;
}

inline ssize_t ProcessVmRead(pid_t pid, const iovec* local, unsigned long local_count,
                             const iovec* remote, unsigned long remote_count, unsigned long) {
  mach_port_t task = TaskForPid(pid);
  if (task == MACH_PORT_NULL || local_count != 1) {
    errno = EINVAL;
    return -1;
  }
  size_t destination_offset = 0;
  for (unsigned long index = 0; index < remote_count; ++index) {
    if (destination_offset >= local[0].iov_len) break;
    mach_vm_size_t copied = 0;
    const size_t amount = std::min(remote[index].iov_len, local[0].iov_len - destination_offset);
    kern_return_t result = mach_vm_read_overwrite(
        task, reinterpret_cast<mach_vm_address_t>(remote[index].iov_base), amount,
        reinterpret_cast<mach_vm_address_t>(local[0].iov_base) + destination_offset, &copied);
    if (result != KERN_SUCCESS) {
      if (pid != getpid()) mach_port_deallocate(mach_task_self(), task);
      errno = EFAULT;
      return destination_offset == 0 ? -1 : static_cast<ssize_t>(destination_offset);
    }
    destination_offset += copied;
    if (copied != amount) break;
  }
  if (pid != getpid()) mach_port_deallocate(mach_task_self(), task);
  return static_cast<ssize_t>(destination_offset);
}

inline long Ptrace(int request, pid_t pid, void* address, void* data) {
  mach_port_t task = TaskForPid(pid);
  if (task == MACH_PORT_NULL) {
    errno = ESRCH;
    return -1;
  }
  if (request == PTRACE_PEEKTEXT) {
    long value = 0;
    mach_vm_size_t copied = 0;
    kern_return_t result = mach_vm_read_overwrite(
        task, reinterpret_cast<mach_vm_address_t>(address), sizeof(value),
        reinterpret_cast<mach_vm_address_t>(&value), &copied);
    if (pid != getpid()) mach_port_deallocate(mach_task_self(), task);
    if (result != KERN_SUCCESS || copied != sizeof(value)) {
      errno = EFAULT;
      return -1;
    }
    return value;
  }
  if (request == PTRACE_GETREGSET) {
    auto* io = static_cast<iovec*>(data);
    thread_act_t thread = FindThread(task, 0);
    arm_thread_state64_t state{};
    mach_msg_type_number_t state_count = ARM_THREAD_STATE64_COUNT;
    kern_return_t result = thread == MACH_PORT_NULL
                               ? KERN_INVALID_ARGUMENT
                               : thread_get_state(thread, ARM_THREAD_STATE64,
                                                  reinterpret_cast<thread_state_t>(&state),
                                                  &state_count);
    if (thread != MACH_PORT_NULL) mach_port_deallocate(mach_task_self(), thread);
    if (pid != getpid()) mach_port_deallocate(mach_task_self(), task);
    struct Arm64UserRegs {
      uint64_t regs[31];
      uint64_t sp;
      uint64_t pc;
      uint64_t pstate;
    } regs{};
    if (result != KERN_SUCCESS || io == nullptr || io->iov_len < sizeof(regs)) {
      errno = EIO;
      return -1;
    }
    for (size_t index = 0; index < 29; ++index) regs.regs[index] = state.__x[index];
    regs.regs[29] = state.__fp;
    regs.regs[30] = state.__lr;
    regs.sp = state.__sp;
    regs.pc = state.__pc;
    regs.pstate = state.__cpsr;
    std::memcpy(io->iov_base, &regs, sizeof(regs));
    io->iov_len = sizeof(regs);
    return 0;
  }
  if (pid != getpid()) mach_port_deallocate(mach_task_self(), task);
  errno = ENOTSUP;
  return -1;
}

inline long Ptrace(int request, pid_t pid, int address, void* data) {
  return Ptrace(request, pid, reinterpret_cast<void*>(static_cast<uintptr_t>(address)), data);
}

inline int TgKill(pid_t process, pid_t thread_id, int signal_number) {
  if (process != getpid()) {
    errno = ESRCH;
    return -1;
  }
  thread_act_t thread = FindThread(mach_task_self(), static_cast<uint64_t>(thread_id));
  if (thread == MACH_PORT_NULL) {
    errno = ESRCH;
    return -1;
  }
  pthread_t pthread = pthread_from_mach_thread_np(thread);
  int result = pthread == nullptr ? ESRCH : (signal_number == 0 ? 0 : pthread_kill(pthread, signal_number));
  mach_port_deallocate(mach_task_self(), thread);
  if (result != 0) {
    errno = result;
    return -1;
  }
  return 0;
}

}  // namespace aim::unwindstack_abi

#define process_vm_readv aim::unwindstack_abi::ProcessVmRead
#define ptrace aim::unwindstack_abi::Ptrace
#define tgkill aim::unwindstack_abi::TgKill
#define getpagesize aim::unwindstack_abi::PageSize

#endif
