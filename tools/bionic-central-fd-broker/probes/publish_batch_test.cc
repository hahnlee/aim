#include "aim_bionic_fd_broker.h"

#include <cassert>
#include <cstdint>
#include <map>

namespace {

struct Fixture {
  std::map<uint64_t, size_t> close_counts;
  size_t read_calls = 0;
  size_t snapshot_calls = 0;
};

intptr_t Read(void *context, uint64_t object, void *bytes, size_t count,
              int *android_errno) {
  auto *fixture = static_cast<Fixture *>(context);
  ++fixture->read_calls;
  if (bytes != nullptr && count != 0)
    static_cast<char *>(bytes)[0] = static_cast<char>('A' + object % 26);
  *android_errno = 0;
  return count == 0 ? 0 : 1;
}

int Close(void *context, uint64_t object, int *android_errno) {
  auto *fixture = static_cast<Fixture *>(context);
  ++fixture->close_counts[object];
  *android_errno = 0;
  return 0;
}

AimFdOwnerV1 Callbacks(Fixture *fixture) {
  AimFdOwnerV1 callbacks{};
  callbacks.abi_version = AIM_FD_OWNER_ABI_V7;
  callbacks.struct_size = sizeof(callbacks);
  callbacks.context = fixture;
  callbacks.read = Read;
  callbacks.close = Close;
  return callbacks;
}

void Expect(AimFdBrokerStatus actual, AimFdBrokerStatus expected) {
  assert(actual == expected);
}

void CloseFd(AimFdBroker *broker, int fd) {
  AimFdIoResult result{};
  Expect(aim_fd_broker_close(broker, fd, &result),
         AIM_FD_BROKER_OK);
  assert(result.value == 0 && result.android_errno == 0);
}

struct SnapshotExpectation {
  AimFdOwnerHandle owner;
  uint64_t object;
  int status_flags;
  size_t calls = 0;
};

intptr_t Snapshot(void *context,
                  const AimFdDescriptionSnapshotV1 *snapshot,
                  int *android_errno) {
  auto *expectation = static_cast<SnapshotExpectation *>(context);
  assert(snapshot->owner == expectation->owner);
  assert(snapshot->object == expectation->object);
  assert(snapshot->kind == AIM_FD_FS_FILE);
  assert(snapshot->status_flags == expectation->status_flags);
  ++expectation->calls;
  *android_errno = 0;
  return 1;
}

void TestMultiOwnerBatchAndPair() {
  Fixture first;
  Fixture second;
  AimFdBroker *broker = aim_fd_broker_create();
  assert(broker != nullptr);
  AimFdOwnerHandle first_owner = 0;
  AimFdOwnerHandle second_owner = 0;
  auto first_callbacks = Callbacks(&first);
  auto second_callbacks = Callbacks(&second);
  Expect(aim_fd_broker_install_owner(broker, AIM_FD_FS_FILE,
                                            &first_callbacks, &first_owner),
         AIM_FD_BROKER_OK);
  Expect(aim_fd_broker_install_owner(broker, AIM_FD_FS_FILE,
                                            &second_callbacks, &second_owner),
         AIM_FD_BROKER_OK);

  const AimFdPublishBatchEntryV1 rejected_entries[2] = {
      {first_owner, 10, 0, 0},
      {UINT64_C(9999), 20, 0, 0},
  };
  int rejected_fds[2] = {-101, -102};
  Expect(aim_fd_broker_publish_batch_with_flags(broker, rejected_entries,
                                                       2, rejected_fds),
         AIM_FD_BROKER_STALE);
  assert(rejected_fds[0] == -101 && rejected_fds[1] == -102);
  AimFdKind rejected_kind = AIM_FD_FS_FILE;
  Expect(aim_fd_broker_get_kind(broker, rejected_fds[0], &rejected_kind),
         AIM_FD_BROKER_STALE);

  const AimFdPublishBatchEntryV1 entries[2] = {
      {first_owner, 11, 0x120, AIM_FD_CLOEXEC},
      {second_owner, 22, 0x240, 0},
  };
  int fds[2] = {-11, -22};
  Expect(aim_fd_broker_publish_batch_with_flags(broker, entries, 2, fds),
         AIM_FD_BROKER_OK);
  assert(fds[0] != fds[1]);
  int value = -1;
  Expect(aim_fd_broker_get_status_flags(broker, fds[0], &value),
         AIM_FD_BROKER_OK);
  assert(value == 0x120);
  Expect(aim_fd_broker_get_descriptor_flags(broker, fds[0], &value),
         AIM_FD_BROKER_OK);
  assert(value == AIM_FD_CLOEXEC);
  Expect(aim_fd_broker_get_status_flags(broker, fds[1], &value),
         AIM_FD_BROKER_OK);
  assert(value == 0x240);

  char byte = 0;
  AimFdIoResult io_result{};
  Expect(aim_fd_broker_read(broker, fds[0], &byte, 1, &io_result),
         AIM_FD_BROKER_OK);
  assert(io_result.value == 1 && byte == 'L');
  Expect(aim_fd_broker_read(broker, fds[1], &byte, 1, &io_result),
         AIM_FD_BROKER_OK);
  assert(io_result.value == 1 && byte == 'W');

  SnapshotExpectation first_snapshot{first_owner, 11, 0x120};
  AimFdDescriptionPin *pin = nullptr;
  Expect(aim_fd_broker_retain_description(broker, fds[0], first_owner,
                                                 Snapshot, &first_snapshot,
                                                 &pin, &io_result),
         AIM_FD_BROKER_OK);
  assert(pin != nullptr && io_result.value == 1 && first_snapshot.calls == 1);
  Expect(aim_fd_broker_release_description(broker, pin),
         AIM_FD_BROKER_OK);

  CloseFd(broker, fds[0]);
  CloseFd(broker, fds[1]);
  assert(first.close_counts[11] == 1);
  assert(second.close_counts[22] == 1);

  const uint64_t pair_objects[2] = {31, 32};
  const int pair_status[2] = {0x310, 0x320};
  const int pair_descriptor[2] = {0, AIM_FD_CLOEXEC};
  int pair_fds[2] = {-31, -32};
  Expect(aim_fd_broker_publish_pair_with_flags(
             broker, first_owner, pair_objects, pair_status, pair_descriptor,
             pair_fds),
         AIM_FD_BROKER_OK);
  SnapshotExpectation pair_snapshot{first_owner, 32, 0x320};
  pin = nullptr;
  Expect(aim_fd_broker_retain_description(
             broker, pair_fds[1], first_owner, Snapshot, &pair_snapshot, &pin,
             &io_result),
         AIM_FD_BROKER_OK);
  Expect(aim_fd_broker_release_description(broker, pin),
         AIM_FD_BROKER_OK);
  assert(pair_snapshot.calls == 1);
  Expect(aim_fd_broker_read(broker, pair_fds[0], &byte, 1, &io_result),
         AIM_FD_BROKER_OK);
  assert(io_result.value == 1 && byte == 'F');
  CloseFd(broker, pair_fds[0]);
  CloseFd(broker, pair_fds[1]);
  assert(first.close_counts[31] == 1 && first.close_counts[32] == 1);

  Expect(aim_fd_broker_uninstall_owner(broker, first_owner),
         AIM_FD_BROKER_OK);
  Expect(aim_fd_broker_uninstall_owner(broker, second_owner),
         AIM_FD_BROKER_OK);
  Expect(aim_fd_broker_destroy(broker), AIM_FD_BROKER_OK);
}

void TestAllOrNoneCapacity() {
  Fixture fixture;
  AimFdBroker *broker = aim_fd_broker_create();
  assert(broker != nullptr);
  AimFdOwnerHandle owner = 0;
  auto callbacks = Callbacks(&fixture);
  Expect(aim_fd_broker_install_owner(broker, AIM_FD_FS_FILE,
                                            &callbacks, &owner),
         AIM_FD_BROKER_OK);

  // Fill all but one bounded namespace slot through real batch transactions.
  constexpr size_t kLive = 1023;
  int live_fds[kLive];
  size_t published = 0;
  while (published != kLive) {
    AimFdPublishBatchEntryV1
        entries[AIM_FD_BROKER_MAX_PUBLISH_BATCH]{};
    int fds[AIM_FD_BROKER_MAX_PUBLISH_BATCH]{};
    const size_t count =
        (kLive - published < AIM_FD_BROKER_MAX_PUBLISH_BATCH)
            ? kLive - published
            : AIM_FD_BROKER_MAX_PUBLISH_BATCH;
    for (size_t index = 0; index < count; ++index)
      entries[index] = {owner, 1000 + published + index, 0, 0};
    Expect(aim_fd_broker_publish_batch_with_flags(broker, entries, count,
                                                         fds),
           AIM_FD_BROKER_OK);
    for (size_t index = 0; index < count; ++index)
      live_fds[published + index] = fds[index];
    published += count;
  }

  const uint64_t pair_objects[2] = {7001, 7002};
  const int pair_status[2] = {0, 0};
  const int pair_descriptor[2] = {0, 0};
  int pair_fds[2] = {-701, -702};
  Expect(
      aim_fd_broker_publish_pair_with_flags(
          broker, owner, pair_objects, pair_status, pair_descriptor, pair_fds),
      AIM_FD_BROKER_EXHAUSTED);
  assert(pair_fds[0] == -701 && pair_fds[1] == -702);
  AimFdKind kind = AIM_FD_FS_FILE;
  Expect(aim_fd_broker_get_kind(broker, pair_fds[0], &kind),
         AIM_FD_BROKER_STALE);

  // The single remaining slot is still available, proving the failed pair did
  // not expose its first object as a partial publication.
  int last_fd = -703;
  Expect(aim_fd_broker_publish(broker, owner, 7003, &last_fd),
         AIM_FD_BROKER_OK);
  CloseFd(broker, last_fd);
  assert(fixture.close_counts[7003] == 1);
  for (int fd : live_fds)
    CloseFd(broker, fd);
  assert(fixture.close_counts.size() == kLive + 1);
  for (const auto &entry : fixture.close_counts)
    assert(entry.second == 1);

  Expect(aim_fd_broker_uninstall_owner(broker, owner),
         AIM_FD_BROKER_OK);
  Expect(aim_fd_broker_destroy(broker), AIM_FD_BROKER_OK);
}

} // namespace

int main() {
  TestMultiOwnerBatchAndPair();
  TestAllOrNoneCapacity();
  return 0;
}
