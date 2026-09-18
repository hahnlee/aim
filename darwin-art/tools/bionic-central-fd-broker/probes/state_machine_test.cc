#include "darwin_art_bionic_fd_broker.h"

#include <algorithm>
#include <atomic>
#include <cassert>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <map>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace {

constexpr int kAndroidEbadf = 9;
constexpr uint64_t kBlockedObject = 99;
constexpr uint64_t kBlockedCloseObject = 100;
constexpr uint64_t kAliasBlockedObject = 199;
constexpr uint64_t kAliasReplacementObject = 1003;

struct ObjectState {
  std::string input;
  std::string output;
  size_t offset = 0;
  bool closed = false;
};

struct Fake {
  std::mutex mutex;
  std::condition_variable changed;
  std::map<uint64_t, ObjectState> objects;
  std::vector<std::string> events;
  bool blocked_read_entered = false;
  bool release_blocked_read = false;
  std::thread::id blocked_reader;
  bool blocked_close_entered = false;
  bool release_blocked_close = false;
  bool blocked_socket_entered = false;
  bool release_blocked_socket = false;
  size_t socket_calls = 0;
  size_t accept_calls = 0;
  int last_poll_timeout = -2;
  uint64_t next_accepted_object = 200;
  uint32_t accepted_kind = DARWIN_ART_FD_SOCKET;
  bool wake_wait_entered = false;
};

struct FakePollWake {
  std::mutex mutex;
  std::condition_variable changed;
  bool signaled = false;
};

intptr_t Read(void *context, uint64_t object, void *bytes, size_t count,
              int *android_errno) {
  Fake *fake = static_cast<Fake *>(context);
  std::unique_lock lock(fake->mutex);
  ObjectState &state = fake->objects.at(object);
  assert(!state.closed);
  fake->events.push_back(std::to_string(object) + ":read-enter");
  if ((object == kBlockedObject || object == kAliasBlockedObject) &&
      !fake->blocked_read_entered) {
    fake->blocked_read_entered = true;
    fake->blocked_reader = std::this_thread::get_id();
    fake->changed.notify_all();
    fake->changed.wait(lock, [&] { return fake->release_blocked_read; });
  } else if (object == kBlockedObject &&
             fake->blocked_reader != std::this_thread::get_id()) {
    *android_errno = 11;
    return -1;
  }
  const size_t available = state.input.size() - state.offset;
  const size_t copied = std::min(count, available);
  if (copied != 0) {
    std::memcpy(bytes, state.input.data() + state.offset, copied);
    state.offset += copied;
  }
  fake->events.push_back(std::to_string(object) + ":read-exit");
  *android_errno = 0;
  return static_cast<intptr_t>(copied);
}

intptr_t Write(void *context, uint64_t object, const void *bytes, size_t count,
               int *android_errno) {
  Fake *fake = static_cast<Fake *>(context);
  std::lock_guard lock(fake->mutex);
  ObjectState &state = fake->objects.at(object);
  assert(!state.closed);
  state.output.append(static_cast<const char *>(bytes), count);
  fake->events.push_back(std::to_string(object) + ":write");
  *android_errno = 0;
  return static_cast<intptr_t>(count);
}

intptr_t ReadAt(void *context, uint64_t object, int64_t *offset, void *bytes,
                size_t count, int *android_errno) {
  Fake *fake = static_cast<Fake *>(context);
  std::unique_lock lock(fake->mutex);
  ObjectState &state = fake->objects.at(object);
  assert(!state.closed && offset != nullptr && *offset >= 0);
  fake->events.push_back(std::to_string(object) + ":read-enter");
  if ((object == kBlockedObject || object == kAliasBlockedObject) &&
      !fake->blocked_read_entered) {
    fake->blocked_read_entered = true;
    fake->blocked_reader = std::this_thread::get_id();
    fake->changed.notify_all();
    fake->changed.wait(lock, [&] { return fake->release_blocked_read; });
  }
  const size_t start = static_cast<size_t>(*offset);
  const size_t available =
      start < state.input.size() ? state.input.size() - start : 0;
  const size_t copied = std::min(count, available);
  if (copied != 0) {
    std::memcpy(bytes, state.input.data() + start, copied);
    *offset += static_cast<int64_t>(copied);
  }
  fake->events.push_back(std::to_string(object) + ":read-exit");
  *android_errno = 0;
  return static_cast<intptr_t>(copied);
}

intptr_t WriteAt(void *context, uint64_t object, int64_t *offset,
                 const void *bytes, size_t count, int *android_errno) {
  Fake *fake = static_cast<Fake *>(context);
  std::lock_guard lock(fake->mutex);
  ObjectState &state = fake->objects.at(object);
  assert(!state.closed && offset != nullptr && *offset >= 0);
  state.output.append(static_cast<const char *>(bytes), count);
  *offset += static_cast<int64_t>(count);
  fake->events.push_back(std::to_string(object) + ":write");
  *android_errno = 0;
  return static_cast<intptr_t>(count);
}

int Poll(void *context, uint64_t object, int16_t events, int16_t *revents,
         int *android_errno) {
  Fake *fake = static_cast<Fake *>(context);
  std::lock_guard lock(fake->mutex);
  assert(!fake->objects.at(object).closed);
  *revents = events;
  *android_errno = 0;
  fake->events.push_back(std::to_string(object) + ":poll");
  return events == 0 ? 0 : 1;
}

int PollMany(void *context, const uint64_t *objects, const int16_t *events,
             int16_t *revents, size_t count, int timeout_ms,
             int *android_errno) {
  Fake *fake = static_cast<Fake *>(context);
  std::lock_guard lock(fake->mutex);
  fake->last_poll_timeout = timeout_ms;
  int ready = 0;
  for (size_t index = 0; index < count; ++index) {
    assert(!fake->objects.at(objects[index]).closed);
    revents[index] = events[index];
    if (revents[index] != 0)
      ++ready;
    fake->events.push_back(std::to_string(objects[index]) + ":poll-many");
  }
  *android_errno = 0;
  return ready;
}

uint64_t CreatePollWake(void *, int *android_errno) {
  auto *wake = new FakePollWake();
  *android_errno = 0;
  return reinterpret_cast<uint64_t>(wake);
}

int SignalPollWake(void *, uint64_t object, int *android_errno) {
  auto *wake = reinterpret_cast<FakePollWake *>(object);
  {
    std::lock_guard lock(wake->mutex);
    wake->signaled = true;
  }
  wake->changed.notify_one();
  *android_errno = 0;
  return 0;
}

int ClosePollWake(void *, uint64_t object, int *android_errno) {
  delete reinterpret_cast<FakePollWake *>(object);
  *android_errno = 0;
  return 0;
}

int PollManyWithWake(void *context, const uint64_t *objects,
                     const int16_t *events, int16_t *revents, size_t count,
                     int timeout_ms, uint64_t wake_object, int *woke,
                     int *android_errno) {
  bool ready = false;
  for (size_t index = 0; index < count; ++index)
    ready = ready || events[index] != 0;
  if (ready) {
    *woke = 0;
    return PollMany(context, objects, events, revents, count, timeout_ms,
                    android_errno);
  }
  auto *fake = static_cast<Fake *>(context);
  {
    std::lock_guard lock(fake->mutex);
    fake->wake_wait_entered = true;
  }
  fake->changed.notify_all();
  auto *wake = reinterpret_cast<FakePollWake *>(wake_object);
  std::unique_lock lock(wake->mutex);
  if (timeout_ms < 0) {
    wake->changed.wait(lock, [&] { return wake->signaled; });
  } else {
    (void)wake->changed.wait_for(lock, std::chrono::milliseconds(timeout_ms),
                                 [&] { return wake->signaled; });
  }
  *woke = wake->signaled ? 1 : 0;
  *android_errno = 0;
  return *woke;
}

int Ioctl(void *context, uint64_t object, uint64_t request, void *argument,
          int *android_errno) {
  Fake *fake = static_cast<Fake *>(context);
  std::lock_guard lock(fake->mutex);
  assert(!fake->objects.at(object).closed);
  if (request != 0x80045200 || argument == nullptr) {
    *android_errno = 25;
    return -1;
  }
  *static_cast<int *>(argument) = 32;
  fake->events.push_back(std::to_string(object) + ":ioctl");
  *android_errno = 0;
  return 0;
}

int Close(void *context, uint64_t object, int *android_errno) {
  Fake *fake = static_cast<Fake *>(context);
  std::unique_lock lock(fake->mutex);
  ObjectState &state = fake->objects.at(object);
  assert(!state.closed);
  if (object == kBlockedCloseObject) {
    fake->blocked_close_entered = true;
    fake->events.push_back(std::to_string(object) + ":close-enter");
    fake->changed.notify_all();
    fake->changed.wait(lock, [&] { return fake->release_blocked_close; });
  }
  state.closed = true;
  fake->events.push_back(std::to_string(object) + ":close");
  *android_errno = 0;
  return 0;
}

intptr_t SocketOperation(void *context, uint64_t object,
                         const DarwinArtFdSocketRequestV1 *request,
                         DarwinArtFdSocketAcceptResultV1 *accepted,
                         int *android_errno) {
  Fake *fake = static_cast<Fake *>(context);
  std::unique_lock lock(fake->mutex);
  ObjectState &state = fake->objects.at(object);
  assert(!state.closed);
  ++fake->socket_calls;
  fake->events.push_back(std::to_string(object) + ":socket-enter");
  if (object == 101 && !fake->blocked_socket_entered) {
    fake->blocked_socket_entered = true;
    fake->changed.notify_all();
    fake->changed.wait(lock, [&] { return fake->release_blocked_socket; });
  }
  if (request->argument == -777) {
    *android_errno = 111;
    return -1;
  }
  switch (request->operation) {
  case DARWIN_ART_FD_SOCKET_SEND:
  case DARWIN_ART_FD_SOCKET_SENDTO:
    state.output.append(static_cast<const char *>(request->input_bytes),
                        request->byte_count);
    *android_errno = 0;
    return static_cast<intptr_t>(request->byte_count);
  case DARWIN_ART_FD_SOCKET_RECV:
  case DARWIN_ART_FD_SOCKET_RECVFROM: {
    const size_t copied = std::min(request->byte_count, state.input.size());
    if (copied)
      std::memcpy(request->output_bytes, state.input.data(), copied);
    if (request->operation == DARWIN_ART_FD_SOCKET_RECVFROM &&
        request->output_address_length)
      *request->output_address_length = 16;
    *android_errno = 0;
    return static_cast<intptr_t>(copied);
  }
  case DARWIN_ART_FD_SOCKET_GETSOCKOPT:
    if (request->option_output_length)
      *request->option_output_length = sizeof(int);
    if (request->option_output &&
        request->option_output_capacity >= sizeof(int))
      *static_cast<int *>(request->option_output) = 1;
    break;
  case DARWIN_ART_FD_SOCKET_GETPEERNAME:
  case DARWIN_ART_FD_SOCKET_GETSOCKNAME:
    if (request->output_address_length)
      *request->output_address_length = 16;
    break;
  case DARWIN_ART_FD_SOCKET_ACCEPT4: {
    ++fake->accept_calls;
    const uint64_t child = fake->next_accepted_object++;
    fake->objects.emplace(child, ObjectState{});
    accepted->object = child;
    accepted->kind = fake->accepted_kind;
    accepted->status_flags = request->flags;
    accepted->descriptor_flags = request->flags ? DARWIN_ART_FD_CLOEXEC : 0;
    if (request->output_address_length)
      *request->output_address_length = 16;
    break;
  }
  case DARWIN_ART_FD_SOCKET_BIND:
  case DARWIN_ART_FD_SOCKET_CONNECT:
  case DARWIN_ART_FD_SOCKET_LISTEN:
  case DARWIN_ART_FD_SOCKET_SHUTDOWN:
  case DARWIN_ART_FD_SOCKET_SETSOCKOPT:
    break;
  default:
    assert(false && "broker admitted unknown socket operation");
  }
  fake->events.push_back(std::to_string(object) + ":socket-exit");
  *android_errno = 0;
  return 0;
}

DarwinArtFdOwnerV1 Callbacks(Fake *fake) {
  return DarwinArtFdOwnerV1{DARWIN_ART_FD_OWNER_ABI_V6,
                            sizeof(DarwinArtFdOwnerV1),
                            fake,
                            Read,
                            Write,
                            Poll,
                            Ioctl,
                            Close,
                            ReadAt,
                            WriteAt,
                            SocketOperation,
                            PollMany,
                            nullptr,
                            CreatePollWake,
                            SignalPollWake,
                            ClosePollWake,
                            PollManyWithWake,
                            nullptr};
}

size_t FindEvent(const std::vector<std::string> &events,
                 const std::string &wanted) {
  for (size_t index = 0; index < events.size(); ++index) {
    if (events[index] == wanted) {
      return index;
    }
  }
  assert(false && "event missing");
  return events.size();
}

void Expect(DarwinArtFdBrokerStatus actual, DarwinArtFdBrokerStatus expected) {
  assert(actual == expected);
}

struct DescriptionOperationContext {
  DarwinArtFdBroker *broker = nullptr;
  int guest_fd = -1;
  uint64_t expected_object = 0;
  bool close_during_operation = false;
  bool fail = false;
  size_t calls = 0;
};

intptr_t DescriptionOperation(void *context,
                              const DarwinArtFdDescriptionSnapshotV1 *snapshot,
                              int *android_errno) {
  auto *operation = static_cast<DescriptionOperationContext *>(context);
  assert(snapshot != nullptr);
  assert(snapshot->abi_version == DARWIN_ART_FD_DESCRIPTION_SNAPSHOT_ABI_V1);
  assert(snapshot->struct_size == sizeof(*snapshot));
  assert(snapshot->object == operation->expected_object);
  assert(snapshot->kind == DARWIN_ART_FD_FS_FILE);
  assert(snapshot->status_flags == 0);
  ++operation->calls;
  if (operation->close_during_operation) {
    DarwinArtFdIoResult close_result{};
    Expect(darwin_art_fd_broker_close(operation->broker, operation->guest_fd,
                                      &close_result),
           DARWIN_ART_FD_BROKER_OK);
    assert(close_result.value == 0 && close_result.android_errno == 0);
    operation->guest_fd = -1;
  }
  if (operation->fail) {
    *android_errno = 77;
    return -1;
  }
  *android_errno = 0;
  return static_cast<intptr_t>(snapshot->object);
}

bool WaitForClosed(Fake *fake, uint64_t object, bool closed) {
  for (size_t attempt = 0; attempt < 100000; ++attempt) {
    {
      std::lock_guard lock(fake->mutex);
      if (fake->objects.at(object).closed == closed)
        return true;
    }
    std::this_thread::yield();
  }
  return false;
}

void CloseTestFd(DarwinArtFdBroker *broker, int fd) {
  DarwinArtFdIoResult result{};
  Expect(darwin_art_fd_broker_close(broker, fd, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 0 && result.android_errno == 0);
}

void TestRetainedDescription(DarwinArtFdBroker *broker,
                             DarwinArtFdOwnerHandle file_owner,
                             DarwinArtFdOwnerHandle wrong_owner, Fake *fake) {
  int logical_close_fd = -1;
  Expect(
      darwin_art_fd_broker_publish(broker, file_owner, 800, &logical_close_fd),
      DARWIN_ART_FD_BROKER_OK);
  DescriptionOperationContext logical_close{
      broker, logical_close_fd, 800, true, false, 0};
  DarwinArtFdDescriptionPin *pin = nullptr;
  DarwinArtFdIoResult result{};
  Expect(darwin_art_fd_broker_retain_description(
             broker, logical_close_fd, file_owner, &DescriptionOperation,
             &logical_close, &pin, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(pin != nullptr && result.value == 800 && result.android_errno == 0);
  {
    std::lock_guard lock(fake->mutex);
    assert(!fake->objects.at(800).closed);
  }
  assert(darwin_art_fd_broker_destroy(broker) == DARWIN_ART_FD_BROKER_BUSY);
  Expect(darwin_art_fd_broker_release_description(broker, pin),
         DARWIN_ART_FD_BROKER_OK);
  assert(WaitForClosed(fake, 800, true));

  int failed_fd = -1;
  Expect(darwin_art_fd_broker_publish(broker, file_owner, 801, &failed_fd),
         DARWIN_ART_FD_BROKER_OK);
  DescriptionOperationContext failed{broker, failed_fd, 801, false, true, 0};
  pin = nullptr;
  result = {};
  Expect(darwin_art_fd_broker_retain_description(broker, failed_fd, file_owner,
                                                 &DescriptionOperation, &failed,
                                                 &pin, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(pin == nullptr && failed.calls == 1 && result.value == -1 &&
         result.android_errno == 77);
  CloseTestFd(broker, failed_fd);
  assert(WaitForClosed(fake, 801, true));

  int wrong_fd = -1;
  Expect(darwin_art_fd_broker_publish(broker, file_owner, 802, &wrong_fd),
         DARWIN_ART_FD_BROKER_OK);
  DescriptionOperationContext wrong{broker, wrong_fd, 802, false, false, 0};
  pin = nullptr;
  result = {};
  Expect(darwin_art_fd_broker_retain_description(broker, wrong_fd, wrong_owner,
                                                 &DescriptionOperation, &wrong,
                                                 &pin, &result),
         DARWIN_ART_FD_BROKER_WRONG_OWNER);
  assert(pin == nullptr && wrong.calls == 0);
  CloseTestFd(broker, wrong_fd);
  assert(WaitForClosed(fake, 802, true));

  int source_fd = -1;
  int target_fd = -1;
  Expect(darwin_art_fd_broker_publish(broker, file_owner, 803, &source_fd),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_publish(broker, file_owner, 804, &target_fd),
         DARWIN_ART_FD_BROKER_OK);
  DescriptionOperationContext duplicated{broker, target_fd, 804,
                                         false,  false,     0};
  pin = nullptr;
  result = {};
  Expect(darwin_art_fd_broker_retain_description(broker, target_fd, file_owner,
                                                 &DescriptionOperation,
                                                 &duplicated, &pin, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(pin != nullptr && result.value == 804);
  Expect(darwin_art_fd_broker_dup2(broker, source_fd, target_fd, &result),
         DARWIN_ART_FD_BROKER_OK);
  CloseTestFd(broker, source_fd);
  CloseTestFd(broker, target_fd);
  {
    std::lock_guard lock(fake->mutex);
    assert(!fake->objects.at(804).closed);
  }
  Expect(darwin_art_fd_broker_release_description(broker, pin),
         DARWIN_ART_FD_BROKER_OK);
  assert(WaitForClosed(fake, 804, true));
  assert(WaitForClosed(fake, 803, true));

  int repeated_fd = -1;
  Expect(darwin_art_fd_broker_publish(broker, file_owner, 805, &repeated_fd),
         DARWIN_ART_FD_BROKER_OK);
  DescriptionOperationContext repeated{broker, repeated_fd, 805,
                                       false,  false,       0};
  DarwinArtFdDescriptionPin *first = nullptr;
  DarwinArtFdDescriptionPin *second = nullptr;
  Expect(darwin_art_fd_broker_retain_description(
             broker, repeated_fd, file_owner, &DescriptionOperation, &repeated,
             &first, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_retain_description(
             broker, repeated_fd, file_owner, &DescriptionOperation, &repeated,
             &second, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(first != nullptr && second != nullptr && repeated.calls == 2);
  CloseTestFd(broker, repeated_fd);
  {
    std::lock_guard lock(fake->mutex);
    assert(!fake->objects.at(805).closed);
  }
  Expect(darwin_art_fd_broker_release_description(broker, first),
         DARWIN_ART_FD_BROKER_OK);
  {
    std::lock_guard lock(fake->mutex);
    assert(!fake->objects.at(805).closed);
  }
  Expect(darwin_art_fd_broker_release_description(broker, second),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_flush_deferred_closes(broker),
         DARWIN_ART_FD_BROKER_OK);
  assert(WaitForClosed(fake, 805, true));
}

DarwinArtFdSocketRequestV1 SocketRequest(uint32_t operation) {
  DarwinArtFdSocketRequestV1 request{};
  request.abi_version = DARWIN_ART_FD_SOCKET_REQUEST_ABI_V1;
  request.struct_size = sizeof(request);
  request.operation = operation;
  return request;
}

} // namespace

int main() {
  Fake fake;
  fake.objects.emplace(10, ObjectState{"hello", "", 0, false});
  fake.objects.emplace(20, ObjectState{});
  fake.objects.emplace(30, ObjectState{});
  fake.objects.emplace(40, ObjectState{"network", "", 0, false});
  fake.objects.emplace(50, ObjectState{"other", "", 0, false});
  fake.objects.emplace(60, ObjectState{"abcdef", "", 0, false});
  fake.objects.emplace(kBlockedObject, ObjectState{"blocked", "", 0, false});
  fake.objects.emplace(100, ObjectState{});
  fake.objects.emplace(101, ObjectState{});
  fake.objects.emplace(102, ObjectState{"v1", "", 0, false});
  fake.objects.emplace(103, ObjectState{});
  fake.objects.emplace(kAliasReplacementObject, ObjectState{});
  for (uint64_t object = 800; object <= 805; ++object)
    fake.objects.emplace(object, ObjectState{});

  DarwinArtFdBroker *broker = darwin_art_fd_broker_create();
  assert(broker != nullptr);
  DarwinArtFdOwnerV1 callbacks = Callbacks(&fake);
  DarwinArtFdOwnerHandle file_owner = 0;
  DarwinArtFdOwnerHandle random_owner = 0;
  DarwinArtFdOwnerHandle stdio_owner = 0;
  DarwinArtFdOwnerHandle socket_owner = 0;
  DarwinArtFdOwnerHandle second_file_owner = 0;
  Expect(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_FS_FILE,
                                            &callbacks, &file_owner),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_FS_RANDOM,
                                            &callbacks, &random_owner),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_STDIO,
                                            &callbacks, &stdio_owner),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_SOCKET,
                                            &callbacks, &socket_owner),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_FS_FILE,
                                            &callbacks, &second_file_owner),
         DARWIN_ART_FD_BROKER_OK);

  TestRetainedDescription(broker, file_owner, random_owner, &fake);

  int file_fd = -1;
  int random_fd = -1;
  int stdio_fd = -1;
  int socket_fd = -1;
  int second_file_fd = -1;
  Expect(darwin_art_fd_broker_publish(broker, file_owner, 10, &file_fd),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_publish(broker, random_owner, 20, &random_fd),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_publish(broker, stdio_owner, 30, &stdio_fd),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_publish(broker, socket_owner, 40, &socket_fd),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_publish(broker, second_file_owner, 50,
                                      &second_file_fd),
         DARWIN_ART_FD_BROKER_OK);
  assert(file_fd >= 0x40000000 && random_fd != file_fd && stdio_fd != file_fd &&
         socket_fd != file_fd && second_file_fd != file_fd);

  DarwinArtFdIoResult result{};
  DarwinArtFdOwnerV1 legacy_callbacks = callbacks;
  legacy_callbacks.abi_version = DARWIN_ART_FD_OWNER_ABI_V1;
  legacy_callbacks.struct_size = offsetof(DarwinArtFdOwnerV1, read_at);
  DarwinArtFdOwnerHandle legacy_owner = 0;
  Expect(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_FS_FILE,
                                            &legacy_callbacks, &legacy_owner),
         DARWIN_ART_FD_BROKER_OK);
  int legacy_fd = -1;
  Expect(darwin_art_fd_broker_publish(broker, legacy_owner, 102, &legacy_fd),
         DARWIN_ART_FD_BROKER_OK);
  char legacy_byte = 0;
  Expect(darwin_art_fd_broker_read(broker, legacy_fd, &legacy_byte, 1, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(legacy_byte == 'v');
  int64_t legacy_central_offset = -1;
  Expect(darwin_art_fd_broker_get_offset(broker, legacy_fd,
                                         &legacy_central_offset),
         DARWIN_ART_FD_BROKER_OK);
  assert(legacy_central_offset == 0);
  Expect(darwin_art_fd_broker_close(broker, legacy_fd, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_uninstall_owner(broker, legacy_owner),
         DARWIN_ART_FD_BROKER_OK);

  DarwinArtFdOwnerV1 v2_callbacks = callbacks;
  v2_callbacks.abi_version = DARWIN_ART_FD_OWNER_ABI_V2;
  v2_callbacks.struct_size = offsetof(DarwinArtFdOwnerV1, socket_operation);
  DarwinArtFdOwnerHandle v2_socket_owner = 0;
  Expect(darwin_art_fd_broker_install_owner(broker, DARWIN_ART_FD_SOCKET,
                                            &v2_callbacks, &v2_socket_owner),
         DARWIN_ART_FD_BROKER_OK);
  int v2_socket_fd = -1;
  Expect(
      darwin_art_fd_broker_publish(broker, v2_socket_owner, 103, &v2_socket_fd),
      DARWIN_ART_FD_BROKER_OK);
  DarwinArtFdSocketRequestV1 v2_request =
      SocketRequest(DARWIN_ART_FD_SOCKET_LISTEN);
  Expect(darwin_art_fd_broker_socket_operation(
             broker, v2_socket_owner, v2_socket_fd, &v2_request, &result),
         DARWIN_ART_FD_BROKER_UNSUPPORTED);
  Expect(darwin_art_fd_broker_close(broker, v2_socket_fd, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_uninstall_owner(broker, v2_socket_owner),
         DARWIN_ART_FD_BROKER_OK);

  int ofd_fd = -1;
  int ofd_dup = -1;
  int ofd_dup3 = -1;
  int ofd_fcntl = -1;
  Expect(darwin_art_fd_broker_publish_with_flags(
             broker, file_owner, 60, DARWIN_ART_FD_STATUS_APPEND,
             DARWIN_ART_FD_CLOEXEC, &ofd_fd),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_dup(broker, ofd_fd, &ofd_dup),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_duplicate_with_flags(
             broker, ofd_fd, DARWIN_ART_FD_CLOEXEC, &ofd_dup3),
         DARWIN_ART_FD_BROKER_OK);
  const int fcntl_minimum = ofd_dup3 + 10;
  Expect(darwin_art_fd_broker_fcntl_dupfd_cloexec(broker, ofd_fd, fcntl_minimum,
                                                  &ofd_fcntl),
         DARWIN_ART_FD_BROKER_OK);
  assert(ofd_fcntl == fcntl_minimum);
  int flags = -1;
  Expect(darwin_art_fd_broker_get_descriptor_flags(broker, ofd_fd, &flags),
         DARWIN_ART_FD_BROKER_OK);
  assert(flags == DARWIN_ART_FD_CLOEXEC);
  Expect(darwin_art_fd_broker_get_descriptor_flags(broker, ofd_dup, &flags),
         DARWIN_ART_FD_BROKER_OK);
  assert(flags == 0);
  Expect(darwin_art_fd_broker_get_descriptor_flags(broker, ofd_dup3, &flags),
         DARWIN_ART_FD_BROKER_OK);
  assert(flags == DARWIN_ART_FD_CLOEXEC);
  Expect(darwin_art_fd_broker_set_status_flags(broker, ofd_dup3,
                                               DARWIN_ART_FD_STATUS_NONBLOCK),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_get_status_flags(broker, ofd_fd, &flags),
         DARWIN_ART_FD_BROKER_OK);
  assert(flags == DARWIN_ART_FD_STATUS_NONBLOCK);
  char ofd_bytes[7]{};
  Expect(darwin_art_fd_broker_read(broker, ofd_fd, ofd_bytes, 2, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_read(broker, ofd_dup, ofd_bytes + 2, 2, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(std::memcmp(ofd_bytes, "abcd", 4) == 0);
  int64_t shared_offset = -1;
  Expect(darwin_art_fd_broker_get_offset(broker, ofd_fcntl, &shared_offset),
         DARWIN_ART_FD_BROKER_OK);
  assert(shared_offset == 4);
  Expect(darwin_art_fd_broker_close(broker, ofd_fd, &result),
         DARWIN_ART_FD_BROKER_OK);
  {
    std::lock_guard lock(fake.mutex);
    assert(!fake.objects.at(60).closed);
  }
  Expect(darwin_art_fd_broker_read(broker, ofd_dup3, ofd_bytes + 4, 2, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(std::memcmp(ofd_bytes, "abcdef", 6) == 0);
  Expect(darwin_art_fd_broker_close(broker, ofd_dup, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_close(broker, ofd_dup3, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_close(broker, ofd_fcntl, &result),
         DARWIN_ART_FD_BROKER_OK);
  {
    std::lock_guard lock(fake.mutex);
    assert(fake.objects.at(60).closed);
  }

  Expect(
      darwin_art_fd_broker_close_owned(broker, file_owner, socket_fd, &result),
      DARWIN_ART_FD_BROKER_WRONG_OWNER);
  int entropy = 0;
  Expect(darwin_art_fd_broker_ioctl(broker, socket_fd, DARWIN_ART_FD_FS_RANDOM,
                                    0x80045200, &entropy, &result),
         DARWIN_ART_FD_BROKER_WRONG_KIND);
  Expect(darwin_art_fd_broker_ioctl(broker, random_fd, DARWIN_ART_FD_FS_RANDOM,
                                    0x80045200, &entropy, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 0 && entropy == 32);

  DarwinArtFdSocketRequestV1 connect_request =
      SocketRequest(DARWIN_ART_FD_SOCKET_CONNECT);
  uint8_t loopback_address[16]{};
  connect_request.address = loopback_address;
  connect_request.address_length = sizeof(loopback_address);
  Expect(darwin_art_fd_broker_socket_operation(broker, file_owner, socket_fd,
                                               &connect_request, &result),
         DARWIN_ART_FD_BROKER_WRONG_OWNER);
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, file_fd,
                                               &connect_request, &result),
         DARWIN_ART_FD_BROKER_WRONG_KIND);
  const size_t socket_calls_before_invalid = fake.socket_calls;
  DarwinArtFdSocketRequestV1 invalid_request = connect_request;
  invalid_request.address = nullptr;
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &invalid_request, &result),
         DARWIN_ART_FD_BROKER_INVALID_ARGUMENT);
  assert(fake.socket_calls == socket_calls_before_invalid);
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &connect_request, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 0 && result.android_errno == 0);
  DarwinArtFdSocketRequestV1 bind_request = connect_request;
  bind_request.operation = DARWIN_ART_FD_SOCKET_BIND;
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &bind_request, &result),
         DARWIN_ART_FD_BROKER_OK);
  DarwinArtFdSocketRequestV1 listen_request =
      SocketRequest(DARWIN_ART_FD_SOCKET_LISTEN);
  listen_request.argument = 8;
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &listen_request, &result),
         DARWIN_ART_FD_BROKER_OK);
  DarwinArtFdSocketRequestV1 shutdown_request =
      SocketRequest(DARWIN_ART_FD_SOCKET_SHUTDOWN);
  shutdown_request.argument = 2;
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &shutdown_request, &result),
         DARWIN_ART_FD_BROKER_OK);

  const char packet[] = "packet";
  DarwinArtFdSocketRequestV1 send_request =
      SocketRequest(DARWIN_ART_FD_SOCKET_SEND);
  send_request.flags = 0x40;
  send_request.input_bytes = packet;
  send_request.byte_count = sizeof(packet) - 1;
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &send_request, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 6 && result.android_errno == 0);
  char received[8]{};
  DarwinArtFdSocketRequestV1 recv_request =
      SocketRequest(DARWIN_ART_FD_SOCKET_RECV);
  recv_request.output_bytes = received;
  recv_request.byte_count = sizeof(received);
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &recv_request, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 7 && std::memcmp(received, "network", 7) == 0);
  uint8_t accepted_address[16]{};
  uint32_t accepted_address_length = 0;
  DarwinArtFdSocketRequestV1 sendto_request =
      SocketRequest(DARWIN_ART_FD_SOCKET_SENDTO);
  sendto_request.address = loopback_address;
  sendto_request.address_length = sizeof(loopback_address);
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &sendto_request, &result),
         DARWIN_ART_FD_BROKER_OK);
  DarwinArtFdSocketRequestV1 recvfrom_request =
      SocketRequest(DARWIN_ART_FD_SOCKET_RECVFROM);
  recvfrom_request.output_address = accepted_address;
  recvfrom_request.output_address_capacity = sizeof(accepted_address);
  recvfrom_request.output_address_length = &accepted_address_length;
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &recvfrom_request, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(accepted_address_length == 16);
  int option_value = 7;
  DarwinArtFdSocketRequestV1 setsockopt_request =
      SocketRequest(DARWIN_ART_FD_SOCKET_SETSOCKOPT);
  setsockopt_request.option_input = &option_value;
  setsockopt_request.option_input_length = sizeof(option_value);
  setsockopt_request.level = 1;
  setsockopt_request.option = 2;
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &setsockopt_request, &result),
         DARWIN_ART_FD_BROKER_OK);
  uint32_t option_length = 0;
  DarwinArtFdSocketRequestV1 getsockopt_request =
      SocketRequest(DARWIN_ART_FD_SOCKET_GETSOCKOPT);
  getsockopt_request.option_output = &option_value;
  getsockopt_request.option_output_capacity = sizeof(option_value);
  getsockopt_request.option_output_length = &option_length;
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &getsockopt_request, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(option_value == 1 && option_length == sizeof(int));
  for (uint32_t operation :
       {DARWIN_ART_FD_SOCKET_GETPEERNAME, DARWIN_ART_FD_SOCKET_GETSOCKNAME}) {
    DarwinArtFdSocketRequestV1 name_request = SocketRequest(operation);
    accepted_address_length = 0;
    name_request.output_address = accepted_address;
    name_request.output_address_capacity = sizeof(accepted_address);
    name_request.output_address_length = &accepted_address_length;
    Expect(darwin_art_fd_broker_socket_operation(
               broker, socket_owner, socket_fd, &name_request, &result),
           DARWIN_ART_FD_BROKER_OK);
    assert(accepted_address_length == 16);
  }
  connect_request.argument = -777;
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &connect_request, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(result.value == -1 && result.android_errno == 111);
  connect_request.argument = 0;

  DarwinArtFdSocketRequestV1 accept_request =
      SocketRequest(DARWIN_ART_FD_SOCKET_ACCEPT4);
  accept_request.flags = DARWIN_ART_FD_STATUS_NONBLOCK;
  int accepted_length = 0;
  accepted_address_length = 0;
  accept_request.output_address = accepted_address;
  accept_request.output_address_capacity = sizeof(accepted_address);
  accept_request.output_address_length = &accepted_address_length;
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &accept_request, &result),
         DARWIN_ART_FD_BROKER_OK);
  const int accepted_fd = static_cast<int>(result.value);
  assert(accepted_fd >= 0x40000000 && accepted_address_length == 16);
  Expect(darwin_art_fd_broker_get_descriptor_flags(broker, accepted_fd,
                                                   &accepted_length),
         DARWIN_ART_FD_BROKER_OK);
  assert(accepted_length == DARWIN_ART_FD_CLOEXEC);
  Expect(darwin_art_fd_broker_get_status_flags(broker, accepted_fd,
                                               &accepted_length),
         DARWIN_ART_FD_BROKER_OK);
  assert(accepted_length == DARWIN_ART_FD_STATUS_NONBLOCK);
  Expect(darwin_art_fd_broker_close_owned(broker, socket_owner, accepted_fd,
                                          &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_socket_operation(
             broker, socket_owner, accepted_fd, &send_request, &result),
         DARWIN_ART_FD_BROKER_STALE);

  fake.accepted_kind = DARWIN_ART_FD_FS_FILE;
  const uint64_t rejected_child = fake.next_accepted_object;
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &accept_request, &result),
         DARWIN_ART_FD_BROKER_INVALID_ARGUMENT);
  {
    std::lock_guard lock(fake.mutex);
    assert(fake.objects.at(rejected_child).closed);
    assert(std::count(fake.events.begin(), fake.events.end(),
                      std::to_string(rejected_child) + ":close") == 1);
  }
  fake.accepted_kind = DARWIN_ART_FD_SOCKET;

  std::vector<int> saturation_fds;
  for (uint64_t object = 10000; object < 12048; ++object) {
    {
      std::lock_guard lock(fake.mutex);
      fake.objects.emplace(object, ObjectState{});
    }
    int saturation_fd = -1;
    const auto status = darwin_art_fd_broker_publish(broker, socket_owner,
                                                     object, &saturation_fd);
    if (status == DARWIN_ART_FD_BROKER_EXHAUSTED) {
      std::lock_guard lock(fake.mutex);
      fake.objects.erase(object);
      break;
    }
    Expect(status, DARWIN_ART_FD_BROKER_OK);
    saturation_fds.push_back(saturation_fd);
  }
  assert(!saturation_fds.empty());
  const uint64_t exhausted_child = fake.next_accepted_object;
  Expect(darwin_art_fd_broker_socket_operation(broker, socket_owner, socket_fd,
                                               &accept_request, &result),
         DARWIN_ART_FD_BROKER_EXHAUSTED);
  {
    std::lock_guard lock(fake.mutex);
    assert(fake.objects.at(exhausted_child).closed);
    assert(std::count(fake.events.begin(), fake.events.end(),
                      std::to_string(exhausted_child) + ":close") == 1);
  }
  for (int saturation_fd : saturation_fds)
    Expect(darwin_art_fd_broker_close_owned(broker, socket_owner, saturation_fd,
                                            &result),
           DARWIN_ART_FD_BROKER_OK);

  Expect(darwin_art_fd_broker_sendfile(broker, socket_fd, file_fd, 5, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 5);
  {
    std::lock_guard lock(fake.mutex);
    assert(fake.objects.at(40).output == "packethello");
  }
  DarwinArtFdPollEntry poll_entries[] = {
      DarwinArtFdPollEntry{socket_fd, 1, 0},
      DarwinArtFdPollEntry{-1, 1, 7},
      DarwinArtFdPollEntry{123, 1, 0},
  };
  Expect(darwin_art_fd_broker_poll(broker, poll_entries, 3, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 2 && poll_entries[0].revents == 1 &&
         poll_entries[1].revents == 0 && poll_entries[2].revents == 0x20);
  poll_entries[0].revents = 0;
  Expect(darwin_art_fd_broker_poll_wait(broker, poll_entries, 1, 37, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 1 && poll_entries[0].revents == 1 &&
         fake.last_poll_timeout == 37);

  const int stale_file_fd = second_file_fd;
  Expect(darwin_art_fd_broker_close(broker, second_file_fd, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 0);
  Expect(darwin_art_fd_broker_close(broker, stale_file_fd, &result),
         DARWIN_ART_FD_BROKER_STALE);
  Expect(darwin_art_fd_broker_read(broker, stale_file_fd, nullptr, 0, &result),
         DARWIN_ART_FD_BROKER_STALE);
  int reused_fd = -1;
  Expect(
      darwin_art_fd_broker_publish(broker, second_file_owner, 100, &reused_fd),
      DARWIN_ART_FD_BROKER_OK);
  assert(reused_fd != stale_file_fd);
  assert((static_cast<uint32_t>(reused_fd) & 0x3ffU) ==
         (static_cast<uint32_t>(stale_file_fd) & 0x3ffU));

  int blocked_fd = -1;
  Expect(darwin_art_fd_broker_publish(broker, file_owner, kBlockedObject,
                                      &blocked_fd),
         DARWIN_ART_FD_BROKER_OK);
  std::atomic<bool> read_done{false};
  std::atomic<bool> close_done{false};
  std::thread reader([&] {
    char bytes[7]{};
    DarwinArtFdIoResult read_result{};
    Expect(darwin_art_fd_broker_read(broker, blocked_fd, bytes, sizeof(bytes),
                                     &read_result),
           DARWIN_ART_FD_BROKER_OK);
    assert(read_result.value == 7 && std::memcmp(bytes, "blocked", 7) == 0);
    read_done = true;
  });
  {
    std::unique_lock lock(fake.mutex);
    fake.changed.wait(lock, [&] { return fake.blocked_read_entered; });
  }
  int blocked_dup = -1;
  Expect(darwin_art_fd_broker_dup(broker, blocked_fd, &blocked_dup),
         DARWIN_ART_FD_BROKER_OK);
  std::thread closer([&] {
    DarwinArtFdIoResult close_result{};
    Expect(darwin_art_fd_broker_close(broker, blocked_fd, &close_result),
           DARWIN_ART_FD_BROKER_OK);
    close_done = true;
  });
  DarwinArtFdBrokerStatus during_close = DARWIN_ART_FD_BROKER_OK;
  for (size_t attempt = 0; attempt < 100000; ++attempt) {
    int descriptor_flags = 0;
    during_close = darwin_art_fd_broker_get_descriptor_flags(broker, blocked_fd,
                                                             &descriptor_flags);
    if (during_close == DARWIN_ART_FD_BROKER_STALE) {
      break;
    }
    std::this_thread::yield();
  }
  assert(during_close == DARWIN_ART_FD_BROKER_STALE);
  for (size_t attempt = 0; attempt < 100000 && !close_done.load(); ++attempt)
    std::this_thread::yield();
  // Logical close must not wait for the in-flight owner read. The physical
  // owner close remains deferred because blocked_dup still references it.
  assert(close_done.load());
  int rejected_dup = -1;
  Expect(darwin_art_fd_broker_dup(broker, blocked_fd, &rejected_dup),
         DARWIN_ART_FD_BROKER_STALE);
  {
    std::lock_guard lock(fake.mutex);
    fake.release_blocked_read = true;
    fake.changed.notify_all();
  }
  reader.join();
  closer.join();
  assert(read_done.load() && close_done.load());
  {
    std::lock_guard lock(fake.mutex);
    assert(!fake.objects.at(kBlockedObject).closed);
  }
  char eof = 0;
  Expect(darwin_art_fd_broker_read(broker, blocked_dup, &eof, 1, &result),
         DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 0);
  Expect(darwin_art_fd_broker_close(broker, blocked_dup, &result),
         DARWIN_ART_FD_BROKER_OK);
  {
    std::lock_guard lock(fake.mutex);
    const size_t read_exit = FindEvent(fake.events, "99:read-exit");
    const size_t close = FindEvent(fake.events, "99:close");
    assert(read_exit < close);
  }

  // Closing the final inactive alias must recycle its slot even while an
  // earlier alias still owns an active lease. The description close remains
  // queued until that lease releases, and flush must wait for it.
  {
    std::lock_guard lock(fake.mutex);
    fake.objects.emplace(kAliasBlockedObject,
                         ObjectState{"alias-blocked", "", 0, false});
    fake.blocked_read_entered = false;
    fake.release_blocked_read = false;
  }
  int alias_a = -1;
  int alias_b = -1;
  Expect(darwin_art_fd_broker_publish(broker, file_owner, kAliasBlockedObject,
                                      &alias_a),
         DARWIN_ART_FD_BROKER_OK);
  std::atomic<bool> alias_read_done{false};
  std::thread alias_reader([&] {
    char bytes[13]{};
    DarwinArtFdIoResult alias_result{};
    Expect(darwin_art_fd_broker_read(broker, alias_a, bytes, sizeof(bytes),
                                     &alias_result),
           DARWIN_ART_FD_BROKER_OK);
    assert(alias_result.value == 13 &&
           std::memcmp(bytes, "alias-blocked", 13) == 0);
    alias_read_done = true;
  });
  {
    std::unique_lock lock(fake.mutex);
    fake.changed.wait(lock, [&] { return fake.blocked_read_entered; });
  }
  Expect(darwin_art_fd_broker_dup(broker, alias_a, &alias_b),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_close(broker, alias_a, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_close(broker, alias_b, &result),
         DARWIN_ART_FD_BROKER_OK);
  int alias_b_flags = 0;
  Expect(darwin_art_fd_broker_get_descriptor_flags(broker, alias_b,
                                                   &alias_b_flags),
         DARWIN_ART_FD_BROKER_STALE);
  int recycled_alias = -1;
  Expect(darwin_art_fd_broker_publish(broker, file_owner,
                                      kAliasReplacementObject, &recycled_alias),
         DARWIN_ART_FD_BROKER_OK);
  assert(recycled_alias != alias_b &&
         (static_cast<uint32_t>(recycled_alias) & 0x3ffU) ==
             (static_cast<uint32_t>(alias_b) & 0x3ffU));
  std::atomic<bool> flush_done{false};
  std::thread flusher([&] {
    Expect(darwin_art_fd_broker_flush_deferred_closes(broker),
           DARWIN_ART_FD_BROKER_OK);
    flush_done = true;
  });
  for (size_t attempt = 0; attempt < 100000; ++attempt) {
    if (flush_done.load())
      break;
    std::this_thread::yield();
  }
  assert(!flush_done.load() && !alias_read_done.load());
  {
    std::lock_guard lock(fake.mutex);
    fake.release_blocked_read = true;
    fake.changed.notify_all();
  }
  alias_reader.join();
  flusher.join();
  assert(alias_read_done.load() && flush_done.load());
  {
    std::lock_guard lock(fake.mutex);
    assert(fake.objects.at(kAliasBlockedObject).closed);
    assert(std::count(fake.events.begin(), fake.events.end(),
                      std::to_string(kAliasBlockedObject) + ":close") == 1);
  }
  Expect(darwin_art_fd_broker_close(broker, recycled_alias, &result),
         DARWIN_ART_FD_BROKER_OK);

  int epoll_fd = -1;
  Expect(darwin_art_fd_broker_epoll_create1(broker, DARWIN_ART_EPOLL_CLOEXEC,
                                            &epoll_fd),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_get_descriptor_flags(broker, epoll_fd, &flags),
         DARWIN_ART_FD_BROKER_OK);
  assert(flags == DARWIN_ART_FD_CLOEXEC);
  int socket_dup = -1;
  Expect(darwin_art_fd_broker_dup(broker, socket_fd, &socket_dup),
         DARWIN_ART_FD_BROKER_OK);
  DarwinArtFdEpollEvent registration{1, 0x1111};
  Expect(darwin_art_fd_broker_epoll_ctl(broker, epoll_fd,
                                        DARWIN_ART_EPOLL_CTL_ADD, socket_fd,
                                        &registration, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_epoll_ctl(broker, epoll_fd,
                                        DARWIN_ART_EPOLL_CTL_ADD, socket_fd,
                                        &registration, &result),
         DARWIN_ART_FD_BROKER_ALREADY_EXISTS);
  registration.data = 0x2222;
  Expect(darwin_art_fd_broker_epoll_ctl(broker, epoll_fd,
                                        DARWIN_ART_EPOLL_CTL_ADD, socket_dup,
                                        &registration, &result),
         DARWIN_ART_FD_BROKER_OK);
  registration.data = 0x3333;
  Expect(darwin_art_fd_broker_epoll_ctl(broker, epoll_fd,
                                        DARWIN_ART_EPOLL_CTL_MOD, socket_dup,
                                        &registration, &result),
         DARWIN_ART_FD_BROKER_OK);
  DarwinArtFdEpollEvent ready[4]{};
  Expect(
      darwin_art_fd_broker_epoll_wait(broker, epoll_fd, ready, 4, 1, &result),
      DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 2 && fake.last_poll_timeout == 1 &&
         ready[0].data == 0x1111 && ready[1].data == 0x3333);

  int wake_epoll_fd = -1;
  Expect(darwin_art_fd_broker_epoll_create1(broker, 0, &wake_epoll_fd),
         DARWIN_ART_FD_BROKER_OK);
  DarwinArtFdEpollEvent dormant_registration{0, 0x4444};
  Expect(darwin_art_fd_broker_epoll_ctl(broker, wake_epoll_fd,
                                        DARWIN_ART_EPOLL_CTL_ADD, socket_fd,
                                        &dormant_registration, &result),
         DARWIN_ART_FD_BROKER_OK);
  DarwinArtFdIoResult wake_wait_result{};
  DarwinArtFdEpollEvent wake_ready{};
  std::thread wake_waiter([&] {
    Expect(darwin_art_fd_broker_epoll_wait(broker, wake_epoll_fd, &wake_ready,
                                           1, 2000, &wake_wait_result),
           DARWIN_ART_FD_BROKER_OK);
  });
  {
    std::unique_lock lock(fake.mutex);
    fake.changed.wait(lock, [&] { return fake.wake_wait_entered; });
  }
  DarwinArtFdEpollEvent new_registration{1, 0x5555};
  Expect(darwin_art_fd_broker_epoll_ctl(broker, wake_epoll_fd,
                                        DARWIN_ART_EPOLL_CTL_ADD, socket_dup,
                                        &new_registration, &result),
         DARWIN_ART_FD_BROKER_OK);
  wake_waiter.join();
  assert(wake_wait_result.value == 1 && wake_ready.data == 0x5555);
  Expect(darwin_art_fd_broker_close(broker, wake_epoll_fd, &result),
         DARWIN_ART_FD_BROKER_OK);

  Expect(
      darwin_art_fd_broker_epoll_wait(broker, epoll_fd, ready, 4, 0, &result),
      DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 2 && ready[0].data == 0x1111 &&
         ready[1].data == 0x3333);
  registration.events = 0x40000001U;
  registration.data = 0x3333;
  Expect(darwin_art_fd_broker_epoll_ctl(broker, epoll_fd,
                                        DARWIN_ART_EPOLL_CTL_MOD, socket_dup,
                                        &registration, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(
      darwin_art_fd_broker_epoll_wait(broker, epoll_fd, ready, 4, 0, &result),
      DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 2);
  Expect(
      darwin_art_fd_broker_epoll_wait(broker, epoll_fd, ready, 4, 0, &result),
      DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 1 && ready[0].data == 0x1111);
  Expect(darwin_art_fd_broker_epoll_ctl(broker, epoll_fd,
                                        DARWIN_ART_EPOLL_CTL_MOD, socket_dup,
                                        &registration, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_epoll_ctl(broker, epoll_fd,
                                        DARWIN_ART_EPOLL_CTL_DEL, socket_dup,
                                        nullptr, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(
      darwin_art_fd_broker_epoll_wait(broker, epoll_fd, ready, 4, 0, &result),
      DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 1 && ready[0].data == 0x1111);
  registration.events = 1;
  registration.data = 0x2222;
  Expect(darwin_art_fd_broker_epoll_ctl(broker, epoll_fd,
                                        DARWIN_ART_EPOLL_CTL_ADD, socket_dup,
                                        &registration, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_close(broker, socket_fd, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(
      darwin_art_fd_broker_epoll_wait(broker, epoll_fd, ready, 4, 0, &result),
      DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 2);
  Expect(darwin_art_fd_broker_close(broker, socket_dup, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(
      darwin_art_fd_broker_epoll_wait(broker, epoll_fd, ready, 4, 0, &result),
      DARWIN_ART_FD_BROKER_OK);
  assert(result.value == 0);
  Expect(darwin_art_fd_broker_close(broker, epoll_fd, &result),
         DARWIN_ART_FD_BROKER_OK);
  Expect(
      darwin_art_fd_broker_epoll_wait(broker, epoll_fd, ready, 4, 0, &result),
      DARWIN_ART_FD_BROKER_STALE);

  int drain_socket_fd = -1;
  Expect(
      darwin_art_fd_broker_publish(broker, socket_owner, 101, &drain_socket_fd),
      DARWIN_ART_FD_BROKER_OK);
  DarwinArtFdSocketRequestV1 blocking_accept =
      SocketRequest(DARWIN_ART_FD_SOCKET_ACCEPT4);
  const uint64_t draining_child = fake.next_accepted_object;
  std::atomic<bool> socket_operation_done{false};
  std::atomic<bool> socket_close_done{false};
  std::thread socket_user([&] {
    DarwinArtFdIoResult socket_result{};
    Expect(darwin_art_fd_broker_socket_operation(
               broker, socket_owner, drain_socket_fd, &blocking_accept,
               &socket_result),
           DARWIN_ART_FD_BROKER_DRAINING);
    socket_operation_done = true;
  });
  {
    std::unique_lock lock(fake.mutex);
    fake.changed.wait(lock, [&] { return fake.blocked_socket_entered; });
  }
  std::thread socket_closer([&] {
    DarwinArtFdIoResult close_result{};
    Expect(darwin_art_fd_broker_close(broker, drain_socket_fd, &close_result),
           DARWIN_ART_FD_BROKER_OK);
    socket_close_done = true;
  });
  DarwinArtFdBrokerStatus closing_socket = DARWIN_ART_FD_BROKER_OK;
  for (size_t attempt = 0; attempt < 100000; ++attempt) {
    int ignored_flags = 0;
    closing_socket = darwin_art_fd_broker_get_descriptor_flags(
        broker, drain_socket_fd, &ignored_flags);
    if (closing_socket == DARWIN_ART_FD_BROKER_STALE)
      break;
    std::this_thread::yield();
  }
  assert(closing_socket == DARWIN_ART_FD_BROKER_STALE);
  Expect(darwin_art_fd_broker_uninstall_owner(broker, socket_owner),
         DARWIN_ART_FD_BROKER_BUSY);
  int rejected_fd = -1;
  Expect(darwin_art_fd_broker_publish(broker, socket_owner, 40, &rejected_fd),
         DARWIN_ART_FD_BROKER_DRAINING);
  // close() is a logical descriptor close and must return while the socket
  // operation still holds its lease; the owner callback remains deferred.
  for (size_t attempt = 0; attempt < 100000 && !socket_close_done.load();
       ++attempt)
    std::this_thread::yield();
  assert(!socket_operation_done.load() && socket_close_done.load());
  {
    std::lock_guard lock(fake.mutex);
    fake.release_blocked_socket = true;
    fake.changed.notify_all();
  }
  socket_user.join();
  socket_closer.join();
  assert(socket_operation_done.load() && socket_close_done.load());
  {
    std::lock_guard lock(fake.mutex);
    assert(fake.objects.at(draining_child).closed);
    assert(std::count(fake.events.begin(), fake.events.end(),
                      std::to_string(draining_child) + ":close") == 1);
    assert(FindEvent(fake.events, "101:socket-exit") <
           FindEvent(fake.events, "101:close"));
  }
  Expect(darwin_art_fd_broker_uninstall_owner(broker, socket_owner),
         DARWIN_ART_FD_BROKER_OK);
  Expect(darwin_art_fd_broker_publish(broker, socket_owner, 40, &rejected_fd),
         DARWIN_ART_FD_BROKER_STALE);

  std::atomic<bool> owner_close_done{false};
  std::thread owner_closer([&] {
    DarwinArtFdIoResult close_result{};
    Expect(darwin_art_fd_broker_close_owned(broker, second_file_owner,
                                            reused_fd, &close_result),
           DARWIN_ART_FD_BROKER_OK);
    owner_close_done = true;
  });
  {
    std::unique_lock lock(fake.mutex);
    fake.changed.wait(lock, [&] { return fake.blocked_close_entered; });
  }
  Expect(darwin_art_fd_broker_uninstall_owner(broker, second_file_owner),
         DARWIN_ART_FD_BROKER_BUSY);
  Expect(
      darwin_art_fd_broker_publish(broker, second_file_owner, 50, &rejected_fd),
      DARWIN_ART_FD_BROKER_DRAINING);
  assert(!owner_close_done.load());
  {
    std::lock_guard lock(fake.mutex);
    fake.release_blocked_close = true;
    fake.changed.notify_all();
  }
  owner_closer.join();
  Expect(darwin_art_fd_broker_uninstall_owner(broker, second_file_owner),
         DARWIN_ART_FD_BROKER_OK);

  assert(darwin_art_fd_broker_destroy(broker) == DARWIN_ART_FD_BROKER_BUSY);
  const std::pair<DarwinArtFdOwnerHandle, int> remaining[] = {
      {file_owner, file_fd},
      {random_owner, random_fd},
      {stdio_owner, stdio_fd},
  };
  for (const auto &[owner, fd] : remaining) {
    Expect(darwin_art_fd_broker_close_owned(broker, owner, fd, &result),
           DARWIN_ART_FD_BROKER_OK);
    Expect(darwin_art_fd_broker_uninstall_owner(broker, owner),
           DARWIN_ART_FD_BROKER_OK);
  }
  Expect(darwin_art_fd_broker_destroy(broker), DARWIN_ART_FD_BROKER_OK);

  std::cout << "bionic-central-fd-broker: PASS kinds=4 owners=6 "
               "namespace=collision-free foreign=reject\n";
  std::cout << "bionic-central-fd-broker: PASS generation=reuse-safe "
               "stale=reject double-close=reject\n";
  std::cout << "bionic-central-fd-broker: PASS close-vs-use=drained "
               "poll+sendfile+ioctl=typed uninstall=quiescent\n";
  std::cout << "bionic-central-fd-broker: PASS "
               "OFD=dup+duplicate_with_flags+F_DUPFD_CLOEXEC "
               "offset+status=shared "
               "FD_CLOEXEC=independent close=last\n";
  std::cout << "bionic-central-fd-broker: PASS epoll=ADD+MOD+DEL "
               "duplicate=reject socket-fan-in=OFD-stable "
               "timeout=fail-closed\n";
  std::cout
      << "bionic-central-fd-broker: PASS owner-ABI=v1-prefix+v2-prefix+v3 "
         "callbacks=size-checked\n";
  std::cout << "bionic-central-fd-broker: PASS socket=13-typed-operations "
               "owner+kind+generation=checked callback=leased-unlocked "
               "accept=atomic-publish+rollback-once\n";
  return kAndroidEbadf == 9 ? 0 : 1;
}
