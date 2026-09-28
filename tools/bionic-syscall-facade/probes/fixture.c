#include <stdint.h>

extern long syscall(long number, ...);

struct AndroidTimespec {
  int64_t seconds;
  int64_t nanoseconds;
};

__attribute__((visibility("default"))) long SyscallFixtureGettid(void) {
  return syscall(178);
}

__attribute__((visibility("default"))) long SyscallFixtureGetpid(void) {
  return syscall(172);
}

__attribute__((visibility("default"))) long SyscallFixtureRtTgSigqueueinfo(
    int process, int tid, int signal_number, const void* information) {
  return syscall(240, process, tid, signal_number, information);
}

__attribute__((visibility("default"))) long SyscallFixtureGetrandom(
    void* buffer, uint64_t length, uint32_t flags) {
  return syscall(278, buffer, length, flags);
}

__attribute__((visibility("default"))) long SyscallFixtureGetaffinity(
    int tid, uint64_t size, void* mask) {
  return syscall(123, tid, size, mask);
}

__attribute__((visibility("default"))) long SyscallFixtureSetaffinity(
    int tid, uint64_t size, const void* mask) {
  return syscall(122, tid, size, mask);
}

__attribute__((visibility("default"))) long SyscallFixtureWait(
    int32_t* address, int32_t expected, int64_t timeout_nanoseconds) {
  struct AndroidTimespec timeout = {
      timeout_nanoseconds / 1000000000LL,
      timeout_nanoseconds % 1000000000LL,
  };
  return syscall(98, address, 128, expected, &timeout, 0, 0);
}

__attribute__((visibility("default"))) long SyscallFixtureWakeOne(
    int32_t* address) {
  return syscall(98, address, 129, 1, 0, 0, 0);
}

__attribute__((visibility("default"))) long SyscallFixtureWakeAll(
    int32_t* address) {
  return syscall(98, address, 129, 0x7fffffff, 0, 0, 0);
}

__attribute__((visibility("default"))) long SyscallFixturePlainWait(
    int32_t* address, int32_t expected, int64_t timeout_nanoseconds) {
  struct AndroidTimespec timeout = {
      timeout_nanoseconds / 1000000000LL,
      timeout_nanoseconds % 1000000000LL,
  };
  return syscall(98, address, 0, expected, &timeout);
}

__attribute__((visibility("default"))) long SyscallFixturePlainWake(
    int32_t* address, int32_t count) {
  return syscall(98, address, 1, count);
}

__attribute__((visibility("default"))) long SyscallFixtureWaitBitset(
    int32_t* address) {
  return syscall(98, address, 0x189, 0, 0, 0, 0xffffffffu);
}

__attribute__((visibility("default"))) long SyscallFixtureWakeBitset(
    int32_t* address) {
  return syscall(98, address, 0x8a, 1, 0, 0, 0xffffffffu);
}

__attribute__((visibility("default"))) long SyscallFixtureReadable(
    const void* address) {
  return syscall(135, -1, address, 0, 8);
}

__attribute__((visibility("default"))) long SyscallFixtureUnknown(void) {
  return syscall(9999);
}

__attribute__((visibility("default"))) long SyscallFixtureBadFutex(
    int32_t* address) {
  return syscall(98, address, 2, 0, 0, 0, 0);
}
