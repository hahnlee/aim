// Actual CompositionQueue + TransactionReply boundary fixture. The endpoint
// symbols below are test-only FFI stubs; no production service is started.
#include "../../compat/process/service_endpoint.h"
#include "../../compat/surfaceflinger/composition_queue.h"
#include "../../compat/surfaceflinger/composition_protocol.h"
#include "../../compat/surfaceflinger/transaction_reply.h"

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <memory>
#include <mutex>

namespace {

std::atomic<uint64_t> next_handle{1};
std::atomic<int> endpoint_close_calls{0};

}  // namespace

extern "C" int32_t darwin_art_service_endpoint_open(
    const uint8_t* path, size_t length, uint64_t* handle, int32_t* descriptor) {
  assert(path != nullptr && length != 0 && handle != nullptr && descriptor != nullptr);
  *handle = next_handle.fetch_add(1, std::memory_order_relaxed);
  *descriptor = -1;
  return 0;
}

extern "C" int32_t darwin_art_service_endpoint_close(uint64_t handle) {
  assert(handle != 0);
  endpoint_close_calls.fetch_add(1, std::memory_order_relaxed);
  return 0;
}

extern "C" int32_t darwin_art_service_endpoint_ready(uint64_t handle,
                                                       uint32_t bits) {
  assert(handle != 0 && bits != 0);
  return 0;
}

extern "C" int32_t darwin_art_service_endpoint_lost(uint64_t handle,
                                                      uint32_t bits) {
  assert(handle != 0 && bits != 0);
  return 0;
}

namespace {

using darwin_art::process::ServiceEndpoint;
using darwin_art::surfaceflinger::CommitDisposition;
using darwin_art::surfaceflinger::CompositionJob;
using darwin_art::surfaceflinger::CompositionQueue;
using darwin_art::surfaceflinger::ResponseHeader;
using darwin_art::surfaceflinger::TransactionReply;
using darwin_art::surfaceflinger::kProtocolVersion;

struct SocketPair {
  int client = -1;
  int server = -1;

  SocketPair() {
    int descriptors[2] = {-1, -1};
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, descriptors) == 0);
    client = descriptors[0];
    server = descriptors[1];
  }
  ~SocketPair() {
    if (client >= 0) close(client);
    if (server >= 0) close(server);
  }
  void KeepServerOnly() {
    close(client);
    client = -1;
  }
};

struct FencePipe {
  int read = -1;
  int write = -1;

  FencePipe() {
    int descriptors[2] = {-1, -1};
    assert(pipe(descriptors) == 0);
    read = descriptors[0];
    write = descriptors[1];
  }
  ~FencePipe() {
    if (read >= 0) close(read);
    if (write >= 0) close(write);
  }
  void DetachRead() { read = -1; }
  void DetachWrite() { write = -1; }
};

std::shared_ptr<ServiceEndpoint> Endpoint() {
  return std::make_shared<ServiceEndpoint>("/composition-queue-reply-fixture");
}

void ReadExact(int descriptor, void* data, size_t size) {
  auto* bytes = static_cast<unsigned char*>(data);
  while (size != 0) {
    const ssize_t count = read(descriptor, bytes, size);
    assert(count > 0);
    bytes += count;
    size -= static_cast<size_t>(count);
  }
}

void ReadRejected(SocketPair& connection, int32_t status) {
  ResponseHeader response{};
  ReadExact(connection.server, &response, sizeof(response));
  assert(response.version == kProtocolVersion);
  assert(response.status == status);
  assert(response.commit == CommitDisposition::RejectedBeforeCommit);
  assert(response.has_completion_fence == 0);
  char marker = -1;
  ReadExact(connection.server, &marker, sizeof(marker));
  assert(marker == 0);
}

void AssertEofWithoutResponse(int descriptor) {
  pollfd waiter{descriptor, POLLIN | POLLHUP, 0};
  assert(poll(&waiter, 1, 2000) == 1);
  char byte = 0;
  assert(read(descriptor, &byte, sizeof(byte)) == 0);
}

bool IsClosed(int descriptor) {
  errno = 0;
  return fcntl(descriptor, F_GETFD) == -1 && errno == EBADF;
}

struct FullQueueComposeState {
  std::mutex mutex;
  std::condition_variable changed;
  bool entered = false;
  bool release = false;
  int calls = 0;
};

FullQueueComposeState* full_queue_state = nullptr;

void BlockFirstComposition(CompositionJob&) {
  assert(full_queue_state != nullptr);
  std::unique_lock<std::mutex> lock(full_queue_state->mutex);
  ++full_queue_state->calls;
  full_queue_state->entered = true;
  full_queue_state->changed.notify_all();
  full_queue_state->changed.wait(lock, [] {
    return full_queue_state->release;
  });
}

bool ConsumeFence(int descriptor) noexcept {
  char marker = 0;
  assert(read(descriptor, &marker, sizeof(marker)) >= 0);
  close(descriptor);
  return true;
}

void WaitForFirstComposition(FullQueueComposeState& state) {
  std::unique_lock<std::mutex> lock(state.mutex);
  assert(state.changed.wait_for(lock, std::chrono::seconds(2), [&state] {
    return state.entered;
  }));
}

void ReleaseFirstComposition(FullQueueComposeState& state) {
  {
    std::lock_guard<std::mutex> lock(state.mutex);
    state.release = true;
  }
  state.changed.notify_all();
}

void TestFullTryEnqueueRetainsCallerReply() {
  FullQueueComposeState state;
  full_queue_state = &state;
  SocketPair reply_socket;
  FencePipe reply_completion;
  auto reply = TransactionReply::Create(reply_socket.client, reply_completion.read);
  assert(reply);
  reply_socket.KeepServerOnly();

  {
    CompositionQueue queue;
    assert(queue.Start(Endpoint(), &BlockFirstComposition, &ConsumeFence));
    assert(queue.Enqueue(CompositionJob{}));
    WaitForFirstComposition(state);
    for (size_t index = 0; index < CompositionQueue::kCapacity; ++index)
      assert(queue.Enqueue(CompositionJob{}));

    CompositionJob rejected;
    rejected.reply = reply;
    assert(!queue.TryEnqueue(rejected));
    // TryEnqueue takes a by-value job. The caller's retained shared_ptr is the
    // only owner that may send the one explicit admission rejection.
    assert(reply->Rejected(EAGAIN));
    assert(!reply->Rejected(ECANCELED));
    ReadRejected(reply_socket, EAGAIN);

    queue.Abort();
    ReleaseFirstComposition(state);
  }
  full_queue_state = nullptr;
  assert(state.calls == 1);
}

void TestAbortClosesQueuedReplyWithoutSuccess() {
  SocketPair reply_socket;
  FencePipe reply_completion;
  auto reply = TransactionReply::Create(reply_socket.client, reply_completion.read);
  assert(reply);
  reply_socket.KeepServerOnly();
  close(reply_completion.read);
  reply_completion.read = -1;

  // Start a real queue with a callback that would report success if the
  // producer fence became ready. Keeping the write end open prevents that.
  auto compose = +[](CompositionJob&) {
    assert(false && "aborted producer-fence job must never compose");
  };

  FencePipe blocked_producer;
  FencePipe queued_completion;
  const int queued_producer_descriptor = blocked_producer.read;
  const int queued_completion_descriptor = queued_completion.write;
  blocked_producer.DetachRead();
  queued_completion.DetachWrite();
  {
    CompositionQueue queue;
    assert(queue.Start(Endpoint(), compose, &ConsumeFence));
    CompositionJob job{};
    job.producer_descriptor = queued_producer_descriptor;
    job.completion_descriptor = queued_completion_descriptor;
    job.reply = reply;
    assert(queue.TryEnqueue(std::move(job)));
    close(queued_completion.read);
    queued_completion.read = -1;
    reply.reset();

    queue.Abort();
    close(blocked_producer.write);
    blocked_producer.write = -1;
    assert(IsClosed(queued_producer_descriptor));
    assert(IsClosed(queued_completion_descriptor));
    AssertEofWithoutResponse(reply_socket.server);

    // Reusing both queue-owned descriptor numbers catches a second close from
    // the queue destructor or a worker tail after Abort has drained the job.
    int sentinel = open("/dev/null", O_WRONLY);
    assert(sentinel >= 0);
    assert(dup2(sentinel, queued_producer_descriptor) ==
           queued_producer_descriptor);
    assert(dup2(sentinel, queued_completion_descriptor) ==
           queued_completion_descriptor);
    if (sentinel != queued_producer_descriptor &&
        sentinel != queued_completion_descriptor)
      close(sentinel);
  }
  assert(fcntl(queued_producer_descriptor, F_GETFD) >= 0);
  assert(fcntl(queued_completion_descriptor, F_GETFD) >= 0);
  close(queued_producer_descriptor);
  close(queued_completion_descriptor);
}

}  // namespace

int main() {
  alarm(12);
  TestFullTryEnqueueRetainsCallerReply();
  TestAbortClosesQueuedReplyWithoutSuccess();
  alarm(0);
  std::puts("composition queue + transaction reply: rejection ownership, queued abort EOF, and descriptor reuse PASS");
}
