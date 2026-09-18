#pragma once

#include "input_routing.h"
#include "root_key_authority.h"

#include <cstdint>
#include <memory>

namespace darwin_art::input {

struct PreparedRootKeyIngress;

// WMS owns routing policy; this ingress owns only the bounded physical-key
// queue and the exact focus fence captured for each queued key.
class RootKeyIngress final {
 public:
  struct State;
  using SubmitPort = darwin_art::DarwinArtInputEnqueueResult (*)(
      InputRoutingAdmission);

  static std::shared_ptr<RootKeyIngress> Create(
      const RootKeyAuthorityHandle& authority, void* owner_looper,
      SubmitPort submit_port) noexcept;

  RootKeyIngress(const RootKeyIngress&) = delete;
  RootKeyIngress& operator=(const RootKeyIngress&) = delete;
  ~RootKeyIngress();

  // Captures the current root/decision interval synchronously at arrival.
  // Queued transfers one bounded copy to this owner. Backpressure accepts no
  // copy; the caller still owns the event. Internal channel backpressure keeps
  // an already accepted copy and its original fence for a retry.
  darwin_art::DarwinArtInputEnqueueResult Submit(
      const DarwinArtKeyEventV1& key) noexcept;
  void Close() noexcept;
  bool IsQuiescent() const noexcept;

 private:
  friend PreparedRootKeyIngress PrepareRootKeyIngress(
      const RootKeyAuthorityHandle&, void*,
      darwin_art::DarwinArtInputEnqueueResult (*)(InputRoutingAdmission)) noexcept;
  explicit RootKeyIngress(std::shared_ptr<State> state) noexcept;
  std::shared_ptr<State> state_;
};

using RootKeyIngressHandle = std::shared_ptr<RootKeyIngress>;

// Process teardown closes creation and all live/retired ingress states. Poll
// includes detached timer callback/release tails after surface facade release.
bool CloseRootKeyIngressAdmission() noexcept;
bool PollRootKeyIngressQuiesced() noexcept;

inline RootKeyIngressHandle AcquireRootKeyIngress(
    const RootKeyAuthorityHandle& authority, void* owner_looper,
    RootKeyIngress::SubmitPort submit_port) noexcept {
  return RootKeyIngress::Create(authority, owner_looper, submit_port);
}

}  // namespace darwin_art::input
