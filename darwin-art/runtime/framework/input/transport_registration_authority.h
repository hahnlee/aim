#pragma once

#include <cstdint>
#include <memory>

namespace darwin_art::input {

enum class TransportRegistrationRole : std::uint8_t { kReceiver, kRetiredOutput };
enum class TransportRegistrationResult : std::uint8_t {
  kAcquired, kDeferred, kConflict, kInvalid, kOutOfMemory,
};

// One authority is attached to one shared InputTransport resource. It owns
// permission to register each FD, not receiver policy or Looper registrations.
class TransportRegistrationAuthority final {
  struct Control;
  struct Entry;
 public:
  class Claim final {
   public:
    // Each Claim's operations, moves, destruction and Reserve output publication
    // must be externally serialized. The authority mutex protects lane state,
    // not these shared_ptr members. Destruction/release may notify synchronously;
    // callers must not hold coordinator locks across them.
    Claim() = default;
    ~Claim();
    Claim(const Claim&) = delete;
    Claim& operator=(const Claim&) = delete;
    Claim(Claim&& other) noexcept;
    Claim& operator=(Claim&& other) noexcept;
    explicit operator bool() const { return entry_ != nullptr; }
    TransportRegistrationResult BeginRegistration();
    // Caller must first prove exact removal and callback/operation quiescence.
    // Dropping an admitted claim never silently permits another AddFd owner.
    bool ReleaseAfterQuiescence();
    // Notifications are possibly stale/overlapping hints. Schedule coalesced
    // owner work and re-run BeginRegistration before AddFd; kAcquired alone
    // does not authorize registration.
    bool SubscribeAvailable(void (*notify)(void*) noexcept,
                            std::weak_ptr<void> context);
    // Retired-output owners subscribe before admission. A queued receiver
    // intent requests yielding, NOT revocation: retire the exact pump, prove
    // quiescence, then release this claim. Weak, stale/overlapping hints run
    // outside authority locks; recheck HasLiveIntent on the owner Looper.
    bool SubscribeReceiverWaiting(void (*notify)(void*) noexcept,
                                  std::weak_ptr<void> context);
   private:
    friend class TransportRegistrationAuthority;
    std::shared_ptr<Control> control_;
    std::shared_ptr<Entry> entry_;
    void Abandon() noexcept;
  };

  TransportRegistrationAuthority();
  TransportRegistrationAuthority(const TransportRegistrationAuthority&) = delete;
  TransportRegistrationAuthority& operator=(const TransportRegistrationAuthority&) = delete;
  TransportRegistrationResult Reserve(int fd, TransportRegistrationRole role,
      std::uint64_t identity, void* looper, Claim* claim) const;
  bool HasLiveIntent(int fd) const;
  bool HasAdmittedRegistration(int fd) const;

 private:
  static bool Release(const std::shared_ptr<Control>&,
                      const std::shared_ptr<Entry>&, bool quiescent);
  static void Notify(const std::shared_ptr<Control>&,
                     const std::shared_ptr<Entry>&);
  static void NotifyReceiverWaiting(const std::shared_ptr<Control>&,
                                   const std::shared_ptr<Entry>&);
  const std::shared_ptr<Control> control_;
};

}  // namespace darwin_art::input
