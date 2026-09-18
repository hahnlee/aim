#include "../../compat/surfaceflinger/output_owner.h"

#include <assert.h>
#include <fcntl.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sys/un.h>
#include <unistd.h>

#include <array>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace {

using darwin_art::surfaceflinger::OutputOperation;
using darwin_art::surfaceflinger::OutputRequest;
using darwin_art::surfaceflinger::OutputResponse;

enum class Behavior {
  RegisterOk,
  ReplaceOk,
  Reject,
  WrongMagic,
  WrongVersion,
  WrongToken,
  WrongGeneration,
};

bool ReadExact(int fd, void* target, size_t size) {
  auto* bytes = static_cast<unsigned char*>(target);
  while (size != 0) {
    const ssize_t count = recv(fd, bytes, size, 0);
    if (count <= 0) return false;
    bytes += count;
    size -= static_cast<size_t>(count);
  }
  return true;
}

class FixtureServer {
 public:
  explicit FixtureServer(std::vector<Behavior> behaviors)
      : behaviors_(std::move(behaviors)) {
    char directory[] = "/tmp/darwin-art-output-owner.XXXXXX";
    const char* created = mkdtemp(directory);
    assert(created != nullptr);
    directory_ = created;
    path_ = directory_ + "/control";
    listener_ = socket(AF_UNIX, SOCK_STREAM, 0);
    assert(listener_ >= 0);
    const int flags = fcntl(listener_, F_GETFD);
    assert(flags >= 0 && fcntl(listener_, F_SETFD, flags | FD_CLOEXEC) == 0);
    sockaddr_un address{};
    address.sun_family = AF_UNIX;
    assert(path_.size() < sizeof(address.sun_path));
    std::memcpy(address.sun_path, path_.c_str(), path_.size() + 1);
    assert(bind(listener_, reinterpret_cast<sockaddr*>(&address),
                sizeof(address)) == 0);
    assert(listen(listener_, 1) == 0);
    thread_ = std::thread([this] { Run(); });
  }

  ~FixtureServer() { Stop(); }

  const char* endpoint() const { return path_.c_str(); }
  const std::string& directory() const { return directory_; }

  bool WaitAccepted() {
    std::unique_lock<std::mutex> lock(mutex_);
    return changed_.wait_for(lock, std::chrono::milliseconds(500),
                             [this] { return accepted_; });
  }

  bool WaitEof() {
    std::unique_lock<std::mutex> lock(mutex_);
    return changed_.wait_for(lock, std::chrono::milliseconds(500),
                             [this] { return eof_; });
  }

  void Stop() {
    if (stopped_) return;
    stopped_ = true;
    if (listener_ >= 0) {
      shutdown(listener_, SHUT_RDWR);
      close(listener_);
      listener_ = -1;
    }
    if (thread_.joinable()) thread_.join();
    unlink(path_.c_str());
    rmdir(directory_.c_str());
  }

 private:
  void Run() {
    const int client = accept(listener_, nullptr, nullptr);
    if (client < 0) return;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      accepted_ = true;
      changed_.notify_all();
    }
    for (Behavior behavior : behaviors_) {
      OutputRequest request{};
      if (!ReadExact(client, &request, sizeof(request))) break;
      requests_.push_back(request);
      if (behavior == Behavior::WrongMagic ||
          behavior == Behavior::WrongVersion) {
        OutputResponse response{};
        if (behavior == Behavior::WrongMagic) response.magic[0] = 'X';
        if (behavior == Behavior::WrongVersion) response.version++;
        SendFragmented(client, response);
        break;
      }
      if (behavior == Behavior::Reject) {
        OutputResponse response{};
        response.status = -7;
        SendFragmented(client, response);
        continue;
      }
      OutputResponse response{};
      response.token.server_instance = 0x51525354;
      response.token.serial = 0x101;
      response.generation = request.operation == OutputOperation::Register
                                ? 1
                                : request.generation + 1;
      if (behavior == Behavior::WrongToken) response.token.serial++;
      if (behavior == Behavior::WrongGeneration) response.generation++;
      SendFragmented(client, response);
    }
    // A malformed response must be followed by client-side retirement. Keep
    // the fixture connection open until the owner closes it; otherwise a
    // server-side close would make the retirement assertion vacuous.
    OutputRequest trailing_request{};
    while (ReadExact(client, &trailing_request, sizeof(trailing_request))) {
    }
    shutdown(client, SHUT_RDWR);
    close(client);
    {
      std::lock_guard<std::mutex> lock(mutex_);
      eof_ = true;
      changed_.notify_all();
    }
  }

  template <typename T>
  void SendFragmented(int fd, const T& value) {
    const auto* bytes = reinterpret_cast<const unsigned char*>(&value);
    size_t remaining = sizeof(value);
    while (remaining != 0) {
      const size_t chunk = remaining > 3 ? 3 : remaining;
      assert(send(fd, bytes, chunk, 0) == static_cast<ssize_t>(chunk));
      bytes += chunk;
      remaining -= chunk;
      usleep(1000);
    }
  }

  std::vector<Behavior> behaviors_;
  std::vector<OutputRequest> requests_;
  std::string directory_;
  std::string path_;
  int listener_ = -1;
  std::thread thread_;
  std::mutex mutex_;
  std::condition_variable changed_;
  bool accepted_ = false;
  bool eof_ = false;
  bool stopped_ = false;
};

OutputRequest Request(uint32_t id, uint32_t width = 640,
                      uint32_t height = 480) {
  OutputRequest request{};
  request.iosurface_id = id;
  request.physical_width = width;
  request.physical_height = height;
  request.logical_width = width;
  request.logical_height = height;
  return request;
}

void AssertCleaned(FixtureServer& server) {
  const std::string directory = server.directory();
  server.Stop();
  assert(access(directory.c_str(), F_OK) == -1);
}

void TestRegisterReplaceRejectThenRecover() {
  FixtureServer server(
      {Behavior::RegisterOk, Behavior::Reject, Behavior::ReplaceOk});
  auto owner = darwin_art::surfaceflinger::OutputOwner::Register(
      server.endpoint(), Request(11));
  assert(owner != nullptr && server.WaitAccepted());
  const auto token = owner->token();
  assert(token.server_instance != 0 && owner->generation() == 1);
  assert(!owner->Replace(Request(12)));
  assert(owner->locally_open());
  assert(owner->token() == token && owner->generation() == 1);
  assert(owner->Replace(Request(13)));
  assert(owner->token() == token && owner->generation() == 2);
  owner.reset();
  assert(server.WaitEof());
  AssertCleaned(server);
}

void TestMalformedResponsesRetire() {
  {
    FixtureServer server({Behavior::WrongMagic});
    auto owner = darwin_art::surfaceflinger::OutputOwner::Register(
        server.endpoint(), Request(21));
    assert(owner == nullptr && server.WaitAccepted() && server.WaitEof());
    AssertCleaned(server);
  }
  {
    FixtureServer server({Behavior::WrongVersion});
    auto owner = darwin_art::surfaceflinger::OutputOwner::Register(
        server.endpoint(), Request(22));
    assert(owner == nullptr && server.WaitAccepted() && server.WaitEof());
    AssertCleaned(server);
  }
  for (Behavior behavior : {Behavior::WrongToken, Behavior::WrongGeneration}) {
    FixtureServer server({Behavior::RegisterOk, behavior});
    auto owner = darwin_art::surfaceflinger::OutputOwner::Register(
        server.endpoint(), Request(23));
    assert(owner != nullptr && server.WaitAccepted());
    assert(owner->generation() == 1);
    assert(!owner->Replace(Request(24)));
    assert(!owner->locally_open());
    assert(owner->token() == darwin_art::surfaceflinger::OutputToken{});
    assert(owner->generation() == 0);
    assert(server.WaitEof());
    // Retirement is terminal; a later request cannot reuse the old connection.
    assert(!owner->Replace(Request(25)));
    AssertCleaned(server);
  }
}

void TestRetireAndDestructorAreIdempotent() {
  FixtureServer server({Behavior::RegisterOk});
  {
    auto owner = darwin_art::surfaceflinger::OutputOwner::Register(
        server.endpoint(), Request(31));
    assert(owner != nullptr && server.WaitAccepted());
    owner->Retire();
    owner->Retire();
    assert(server.WaitEof());
  }
  AssertCleaned(server);
}

void TestCloexecDoesNotLeakAcrossExec(const char* executable) {
  FixtureServer server({Behavior::RegisterOk});
  auto owner = darwin_art::surfaceflinger::OutputOwner::Register(
      server.endpoint(), Request(41));
  assert(owner != nullptr && server.WaitAccepted());
  const pid_t child = fork();
  assert(child >= 0);
  if (child == 0) {
    execl(executable, executable, "--hold-output-owner-fd", nullptr);
    _exit(127);
  }
  // If OutputOwner's socket were inherited across exec, this EOF would be
  // delayed until the child exits. No production descriptor getter is used.
  owner->Retire();
  assert(server.WaitEof());
  kill(child, SIGTERM);
  int status = 0;
  assert(waitpid(child, &status, 0) == child);
  AssertCleaned(server);
}

}  // namespace

int main(int argc, char** argv) {
  if (argc == 2 && std::strcmp(argv[1], "--hold-output-owner-fd") == 0) {
    sleep(5);
    return 0;
  }
  assert(argc >= 1 && argv[0] != nullptr);
  TestRegisterReplaceRejectThenRecover();
  TestMalformedResponsesRetire();
  TestRetireAndDestructorAreIdempotent();
  TestCloexecDoesNotLeakAcrossExec(argv[0]);
  std::puts("output owner fixture: fragmented register/gen2 replacement, explicit "
            "reject recovery, malformed retirement, idempotent retire, CLOEXEC "
            "exec isolation PASS");
}
