#pragma once

#include "output_lifetime_protocol.h"

#include <chrono>
#include <map>
#include <poll.h>
#include <vector>

namespace darwin_art::surfaceflinger {

// Bounded socket ingress only. Output/Android transaction state is owned by
// the callbacks; descriptors and fragmented wire frames stay private here.
class ServiceIngress {
 public:
  using ApplyOutput = OutputResponse (*)(int, const OutputRequest&);
  using DropOutput = void (*)(int) noexcept;
  using Compose = void (*)(int, const std::array<char, 8>&);
  ServiceIngress(ApplyOutput apply, DropOutput drop, Compose compose)
      : apply_(apply), drop_(drop), compose_(compose) {}
  ~ServiceIngress();
  ServiceIngress(const ServiceIngress&) = delete;
  ServiceIngress& operator=(const ServiceIngress&) = delete;
  // Consumes the accepted FD on every return path. No FD can be reused before
  // its output registration is dropped by Close.
  bool Add(int descriptor);
  void AppendPoll(std::vector<pollfd>* waiters) const;
  void Ready(int descriptor, short events);
  void ExpirePending();

 private:
  struct Peer {
    std::array<unsigned char, sizeof(OutputRequest)> input{};
    size_t received = 0;
    std::array<unsigned char, sizeof(OutputResponse)> output{};
    size_t sent = 0;
    bool replying = false;
    bool classified = false;
    bool registered = false;
    std::chrono::steady_clock::time_point deadline;
  };
  void Close(int descriptor);
  std::map<int, Peer> peers_;
  ApplyOutput apply_;
  DropOutput drop_;
  Compose compose_;
};

}  // namespace darwin_art::surfaceflinger
