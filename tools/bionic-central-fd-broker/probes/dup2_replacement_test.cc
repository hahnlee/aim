#include "aim_bionic_fd_broker.h"

#include <cassert>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <map>
#include <mutex>
#include <thread>

namespace {

struct Fixture {
  std::mutex mutex;
  std::condition_variable changed;
  bool blocked_read_entered = false;
  bool release_blocked_read = false;
  uint64_t blocked_object = 0;
  uint64_t last_read_object = 0;
  std::map<uint64_t, size_t> close_counts;
};

intptr_t Read(void* context, uint64_t object, void* bytes, size_t count,
              int* android_errno) {
  auto* fixture = static_cast<Fixture*>(context);
  std::unique_lock lock(fixture->mutex);
  fixture->last_read_object = object;
  if (object == fixture->blocked_object && !fixture->blocked_read_entered) {
    fixture->blocked_read_entered = true;
    fixture->changed.notify_all();
    fixture->changed.wait(lock, [&] { return fixture->release_blocked_read; });
  }
  if (bytes != nullptr && count != 0) {
    static_cast<char*>(bytes)[0] = 'x';
  }
  *android_errno = 0;
  return count == 0 ? 0 : 1;
}

int Close(void* context, uint64_t object, int* android_errno) {
  auto* fixture = static_cast<Fixture*>(context);
  {
    std::lock_guard lock(fixture->mutex);
    ++fixture->close_counts[object];
  }
  fixture->changed.notify_all();
  *android_errno = 0;
  return 0;
}

AimFdOwnerV1 Callbacks(Fixture* fixture) {
  AimFdOwnerV1 callbacks{};
  callbacks.abi_version = AIM_FD_OWNER_ABI_V7;
  callbacks.struct_size = sizeof(callbacks);
  callbacks.context = fixture;
  callbacks.read = Read;
  callbacks.close = Close;
  return callbacks;
}

void Expect(AimFdBrokerStatus actual,
            AimFdBrokerStatus expected) {
  assert(actual == expected);
}

bool WaitForClose(Fixture* fixture, uint64_t object, size_t expected) {
  std::unique_lock lock(fixture->mutex);
  return fixture->changed.wait_for(lock, std::chrono::seconds(2), [&] {
    auto found = fixture->close_counts.find(object);
    return found != fixture->close_counts.end() && found->second == expected;
  });
}

void WaitForBlockedRead(Fixture* fixture) {
  std::unique_lock lock(fixture->mutex);
  bool entered = fixture->changed.wait_for(
      lock, std::chrono::seconds(2),
      [&] { return fixture->blocked_read_entered; });
  assert(entered);
}

void CloseFd(AimFdBroker* broker, int fd) {
  AimFdIoResult result{};
  Expect(aim_fd_broker_close(broker, fd, &result),
         AIM_FD_BROKER_OK);
  assert(result.value == 0 && result.android_errno == 0);
}

void TestReplacementAndDeferredLease(AimFdBroker* broker,
                                     AimFdOwnerHandle owner,
                                     Fixture* fixture) {
  int source = -1;
  int target = -1;
  Expect(aim_fd_broker_publish(broker, owner, 100, &source),
         AIM_FD_BROKER_OK);
  Expect(aim_fd_broker_publish_with_flags(
             broker, owner, 200, 0, AIM_FD_CLOEXEC, &target),
         AIM_FD_BROKER_OK);
  const int stable_target = target;

  fixture->blocked_object = 200;
  std::thread reader([&] {
    char byte = 0;
    AimFdIoResult result{};
    Expect(aim_fd_broker_read(broker, target, &byte, 1, &result),
           AIM_FD_BROKER_OK);
    assert(result.value == 1 && result.android_errno == 0);
  });
  WaitForBlockedRead(fixture);

  // Replacement must not wait for the old I/O lease, and the target token is
  // stable while its old description is retired asynchronously.
  AimFdIoResult result{};
  Expect(aim_fd_broker_dup2(broker, source, target, &result),
         AIM_FD_BROKER_OK);
  assert(result.value == target && result.android_errno == 0);
  assert(target == stable_target);
  int flags = -1;
  Expect(aim_fd_broker_get_descriptor_flags(broker, target, &flags),
         AIM_FD_BROKER_OK);
  assert(flags == 0);
  {
    std::lock_guard lock(fixture->mutex);
    assert(fixture->close_counts[200] == 0);
    fixture->release_blocked_read = true;
  }
  fixture->changed.notify_all();
  reader.join();
  assert(WaitForClose(fixture, 200, 1));

  char byte = 0;
  Expect(aim_fd_broker_read(broker, target, &byte, 1, &result),
         AIM_FD_BROKER_OK);
  assert(result.value == 1 && fixture->last_read_object == 100);

  // An invalid source leaves the replacement target untouched.
  result = {123, 456};
  Expect(aim_fd_broker_dup2(broker, -1, target, &result),
         AIM_FD_BROKER_STALE);
  assert(target == stable_target);
  Expect(aim_fd_broker_read(broker, target, &byte, 1, &result),
         AIM_FD_BROKER_OK);
  assert(fixture->last_read_object == 100);

  CloseFd(broker, source);
  CloseFd(broker, target);
  assert(WaitForClose(fixture, 100, 1));
}

void TestSameFdNoop(AimFdBroker* broker, AimFdOwnerHandle owner) {
  int fd = -1;
  Expect(aim_fd_broker_publish_with_flags(
             broker, owner, 300, 0, AIM_FD_CLOEXEC, &fd),
         AIM_FD_BROKER_OK);
  AimFdIoResult result{};
  Expect(aim_fd_broker_dup2(broker, fd, fd, &result),
         AIM_FD_BROKER_OK);
  assert(result.value == fd && result.android_errno == 0);
  int flags = -1;
  Expect(aim_fd_broker_get_descriptor_flags(broker, fd, &flags),
         AIM_FD_BROKER_OK);
  assert(flags == AIM_FD_CLOEXEC);
  CloseFd(broker, fd);
}

void TestAliasTarget(AimFdBroker* broker, AimFdOwnerHandle owner,
                     Fixture* fixture) {
  int replacement = -1;
  int target = -1;
  Expect(aim_fd_broker_publish(broker, owner, 500, &replacement),
         AIM_FD_BROKER_OK);
  Expect(aim_fd_broker_publish(broker, owner, 600, &target),
         AIM_FD_BROKER_OK);
  int alias = -1;
  Expect(aim_fd_broker_dup(broker, target, &alias),
         AIM_FD_BROKER_OK);
  const int stable_target = target;

  AimFdIoResult result{};
  Expect(aim_fd_broker_dup2(broker, replacement, target, &result),
         AIM_FD_BROKER_OK);
  assert(result.value == target && result.android_errno == 0);
  assert(target == stable_target);

  char byte = 0;
  Expect(aim_fd_broker_read(broker, alias, &byte, 1, &result),
         AIM_FD_BROKER_OK);
  assert(fixture->last_read_object == 600);
  Expect(aim_fd_broker_read(broker, target, &byte, 1, &result),
         AIM_FD_BROKER_OK);
  assert(fixture->last_read_object == 500);

  // The alias keeps the old description alive; replacing target must not
  // close it until that alias is closed, and each description closes once.
  CloseFd(broker, alias);
  assert(WaitForClose(fixture, 600, 1));
  CloseFd(broker, replacement);
  CloseFd(broker, target);
  assert(WaitForClose(fixture, 500, 1));
}

void TestEpollTarget(AimFdBroker* broker, AimFdOwnerHandle owner) {
  int replacement = -1;
  Expect(aim_fd_broker_publish(broker, owner, 700, &replacement),
         AIM_FD_BROKER_OK);
  int epoll_fd = -1;
  Expect(aim_fd_broker_epoll_create1(broker, 0, &epoll_fd),
         AIM_FD_BROKER_OK);
  AimFdIoResult result{};
  // The EPOLL description has owner 0 and no close callback. Replacing it
  // must retire it without dereferencing a nonexistent owner.
  Expect(aim_fd_broker_dup2(broker, replacement, epoll_fd, &result),
         AIM_FD_BROKER_OK);
  assert(result.value == epoll_fd && result.android_errno == 0);
  CloseFd(broker, replacement);
  CloseFd(broker, epoll_fd);
}

void TestMarkerCycles(AimFdBroker* broker, AimFdOwnerHandle owner,
                      Fixture* fixture) {
  int target = -1;
  Expect(aim_fd_broker_publish(broker, owner, 400, &target),
         AIM_FD_BROKER_OK);
  const int stable_target = target;
  constexpr size_t kCycles = 2048;
  for (size_t cycle = 0; cycle < kCycles; ++cycle) {
    int marker = -1;
    Expect(aim_fd_broker_publish(broker, owner, 1000 + cycle, &marker),
           AIM_FD_BROKER_OK);
    AimFdIoResult result{};
    Expect(aim_fd_broker_dup2(broker, marker, target, &result),
           AIM_FD_BROKER_OK);
    assert(result.value == target && result.android_errno == 0);
    assert(target == stable_target);
    CloseFd(broker, marker);
  }
  CloseFd(broker, target);
  assert(WaitForClose(fixture, 400, 1));
  for (size_t cycle = 0; cycle < kCycles; ++cycle) {
    assert(WaitForClose(fixture, 1000 + cycle, 1));
  }
}

}  // namespace

int main() {
  Fixture fixture;
  AimFdBroker* broker = aim_fd_broker_create();
  assert(broker != nullptr);
  AimFdOwnerHandle owner = 0;
  AimFdOwnerV1 callbacks = Callbacks(&fixture);
  Expect(aim_fd_broker_install_owner(
             broker, AIM_FD_FS_FILE, &callbacks, &owner),
         AIM_FD_BROKER_OK);

  TestReplacementAndDeferredLease(broker, owner, &fixture);
  TestSameFdNoop(broker, owner);
  TestAliasTarget(broker, owner, &fixture);
  TestEpollTarget(broker, owner);
  TestMarkerCycles(broker, owner, &fixture);

  Expect(aim_fd_broker_uninstall_owner(broker, owner),
         AIM_FD_BROKER_OK);
  Expect(aim_fd_broker_destroy(broker), AIM_FD_BROKER_OK);
  std::cout << "fd-broker dup2 replacement/lease/cycles PASS\n";
}
