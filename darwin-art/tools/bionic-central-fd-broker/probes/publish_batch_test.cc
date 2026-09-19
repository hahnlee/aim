#include "darwin_art_bionic_fd_broker.h"

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

DarwinArtFdOwnerV1 Callbacks(Fixture *fixture) {
  DarwinArtFdOwnerV1 callbacks{};
  callbacks.abi_version = DARWIN_ART_FD_OWNER_ABI_V7;
  callbacks.struct_size = sizeof(callbacks);
  callbacks.context = fixture;
  callbacks.read = Read;
  callbacks.close = Close;
  return callbacks;
}

void Expect(DarwinArtFdBrokerStatus actual, DarwinArtFdBrokerStatus expected) {
  assert(actual == expected);
}

void CloseFd(DarwinArtFdBroker *broker, int fd) {
  DarwinArtFdIoResult result{};
  Expect(darwin_art_fd_broker_close(broker, fd, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 0 && result.android_errno == 0);
}

struct SnapshotExpectation {
  DarwinArtFdOwnerHandle owner;
  uint64_t object;
  int status_flags;
  size_t calls = 0;
};

intptr_t Snapshot(void *context,
                  const DarwinArtFdDescriptionSnapshotV1 *snapshot,
                  int *android_errno) {
  auto *expectation = static_cast<SnapshotExpectation *>(context);
  assert(snapshot->owner == expectation->owner);
  assert(snapshot->object == expectation->object);
  assert(snapshot->kind == DARWIN_ART_FD_FS_FILE);
  assert(snapshot->status_flags == expectation->status_flags);
  ++expectation->calls;
  *android_errno = 0;
  return 1;
}

void TestMultiOwnerBatchAndPair() {
  Fixture first;
  Fixture second;
  DarwinArtFdBroker *broker = darwin_art_fd_broker_create();
  assert(broker != nullptr);
  DarwinArtFdOwnerHandle first_owner = 0;
  DarwinArtFdOwnerHandle second_owner = 0;
  auto first_callbacks = Callbacks(&first);
  auto second_callbacks = Callbacks(&second);
  Expect(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_FS_FILE,
                                            &first_callbacks, &first_owner),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_FS_FILE,
                                            &second_callbacks, &second_owner),
         DARWIN_ART_FD_BROKER_OK);

  const DarwinArtFdPublishBatchEntryV1 rejected_entries[2] = {
      {first_owner, 10, 0, 0},
      {UINT64_C(9999), 20, 0, 0},
  };
  int rejected_fds[2] = {-101, -102};
  Expect(darwin_art_fd_broker_publish_batch_with_flags(broker, rejected_entries,
                                                       2, rejected_fds),
         DARWIN_ART_FD_BROKER_STALE);
  assert(rejected_fds[0] == -101 && rejected_fds[1] == -102);
  DarwinArtFdKind rejected_kind = DARWIN_ART_FD_FS_FILE;
  Expect(darwin_art_fd_broker_get_kind(broker, rejected_fds[0], &rejected_kind),
         DARWIN_ART_FD_BROKER_STALE);

  const DarwinArtFdPublishBatchEntryV1 entries[2] = {
      {first_owner, 11, 0x120, DARWIN_ART_FD_CLOEXEC},
      {second_owner, 22, 0x240, 0},
  };
  int fds[2] = {-11, -22};
  Expect(darwin_art_fd_broker_publish_batch_with_flags(broker, entries, 2, fds),
         DARWIN_ART_FD_BROKER_OK);
  assert(fds[0] != fds[1]);
  int value = -1;
  Expect(darwin_art_fd_broker_get_status_flags(broker, fds[0], &value),
         DARWIN_ART_FD_BROKER_OK);
  assert(value == 0x120);
  Expect(darwin_art_fd_broker_get_descriptor_flags(broker, fds[0], &value),
         DARWIN_ART_FD_BROKER_OK);
  assert(value == DARWIN_ART_FD_CLOEXEC);
  Expect(darwin_art_fd_broker_get_status_flags(broker, fds[1], &value),
         DARWIN_ART_FD_BROKER_OK);
  assert(value == 0x240);

  char byte = 0;
  DarwinArtFdIoResult io_result{};
  Expect(darwin_art_fd_broker_read(broker, fds[0], &byte, 1, &io_result),
         DARWIN_ART_FD_BROKER_OK);
  assert(io_result.value == 1 && byte == 'L');
  Expect(darwin_art_fd_broker_read(broker, fds[1], &byte, 1, &io_result),
         DARWIN_ART_FD_BROKER_OK);
  assert(io_result.value == 1 && byte == 'W');

  SnapshotExpectation first_snapshot{first_owner, 11, 0x120};
  DarwinArtFdDescriptionPin *pin = nullptr;
  Expect(darwin_art_fd_broker_retain_description(broker, fds[0], first_owner,
                                                 Snapshot, &first_snapshot,
                                                 &pin, &io_result),
         DARWIN_ART_FD_BROKER_OK);
  assert(pin != nullptr && io_result.value == 1 && first_snapshot.calls == 1);
  Expect(darwin_art_fd_broker_release_description(broker, pin),
         DARWIN_ART_FD_BROKER_OK);

  CloseFd(broker, fds[0]);
  CloseFd(broker, fds[1]);
  assert(first.close_counts[11] == 1);
  assert(second.close_counts[22] == 1);

  const uint64_t pair_objects[2] = {31, 32};
  const int pair_status[2] = {0x310, 0x320};
  const int pair_descriptor[2] = {0, DARWIN_ART_FD_CLOEXEC};
  int pair_fds[2] = {-31, -32};
  Expect(darwin_art_fd_broker_publish_pair_with_flags(
             broker, first_owner, pair_objects, pair_status, pair_descriptor,
             pair_fds),
         DARWIN_ART_FD_BROKER_OK);
  SnapshotExpectation pair_snapshot{first_owner, 32, 0x320};
  pin = nullptr;
  Expect(darwin_art_fd_broker_retain_description(
             broker, pair_fds[1], first_owner, Snapshot, &pair_snapshot, &pin,
             &io_result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_release_description(broker, pin),
         DARWIN_ART_FD_BROKER_OK);
  assert(pair_snapshot.calls == 1);
  Expect(darwin_art_fd_broker_read(broker, pair_fds[0], &byte, 1, &io_result),
         DARWIN_ART_FD_BROKER_OK);
  assert(io_result.value == 1 && byte == 'F');
  CloseFd(broker, pair_fds[0]);
  CloseFd(broker, pair_fds[1]);
  assert(first.close_counts[31] == 1 && first.close_counts[32] == 1);

  Expect(darwin_art_fd_broker_uninstall_owner(broker, first_owner),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_uninstall_owner(broker, second_owner),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_destroy(broker), DARWIN_ART_FD_BROKER_OK);
}

void TestAllOrNoneCapacity() {
  Fixture fixture;
  DarwinArtFdBroker *broker = darwin_art_fd_broker_create();
  assert(broker != nullptr);
  DarwinArtFdOwnerHandle owner = 0;
  auto callbacks = Callbacks(&fixture);
  Expect(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_FS_FILE,
                                            &callbacks, &owner),
         DARWIN_ART_FD_BROKER_OK);

  // Fill all but one bounded namespace slot through real batch transactions.
  constexpr size_t kLive = 1023;
  int live_fds[kLive];
  size_t published = 0;
  while (published != kLive) {
    DarwinArtFdPublishBatchEntryV1
        entries[DARWIN_ART_FD_BROKER_MAX_PUBLISH_BATCH]{};
    int fds[DARWIN_ART_FD_BROKER_MAX_PUBLISH_BATCH]{};
    const size_t count =
        (kLive - published < DARWIN_ART_FD_BROKER_MAX_PUBLISH_BATCH)
            ? kLive - published
            : DARWIN_ART_FD_BROKER_MAX_PUBLISH_BATCH;
    for (size_t index = 0; index < count; ++index)
      entries[index] = {owner, 1000 + published + index, 0, 0};
    Expect(darwin_art_fd_broker_publish_batch_with_flags(broker, entries, count,
                                                         fds),
           DARWIN_ART_FD_BROKER_OK);
    for (size_t index = 0; index < count; ++index)
      live_fds[published + index] = fds[index];
    published += count;
  }

  const uint64_t pair_objects[2] = {7001, 7002};
  const int pair_status[2] = {0, 0};
  const int pair_descriptor[2] = {0, 0};
  int pair_fds[2] = {-701, -702};
  Expect(
      darwin_art_fd_broker_publish_pair_with_flags(
          broker, owner, pair_objects, pair_status, pair_descriptor, pair_fds),
      DARWIN_ART_FD_BROKER_EXHAUSTED);
  assert(pair_fds[0] == -701 && pair_fds[1] == -702);
  DarwinArtFdKind kind = DARWIN_ART_FD_FS_FILE;
  Expect(darwin_art_fd_broker_get_kind(broker, pair_fds[0], &kind),
         DARWIN_ART_FD_BROKER_STALE);

  // The single remaining slot is still available, proving the failed pair did
  // not expose its first object as a partial publication.
  int last_fd = -703;
  Expect(darwin_art_fd_broker_publish(broker, owner, 7003, &last_fd),
         DARWIN_ART_FD_BROKER_OK);
  CloseFd(broker, last_fd);
  assert(fixture.close_counts[7003] == 1);
  for (int fd : live_fds)
    CloseFd(broker, fd);
  assert(fixture.close_counts.size() == kLive + 1);
  for (const auto &entry : fixture.close_counts)
    assert(entry.second == 1);

  Expect(darwin_art_fd_broker_uninstall_owner(broker, owner),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_destroy(broker), DARWIN_ART_FD_BROKER_OK);
}

} // namespace

int main() {
  TestMultiOwnerBatchAndPair();
  TestAllOrNoneCapacity();
  return 0;
}
