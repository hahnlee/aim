#pragma once

#include <cstdint>
#include <memory>

#include "input_routing.h"
#include "input_transport_wire.h"

namespace darwin_art::input {

// FocusControl is transport data. This component supplies the small policy
// ledger at its Android owner: no window selection, routing mutation,
// endpoint I/O, JNI, or allocation is performed here.
class InputFocusEpochEvaluator final {
 public:
  enum class Decision : std::uint8_t {
    kAcceptedGain,
    kAcceptedLoss,
    kAcceptedReplay,
    kDuplicate,
    kRejectedZeroEpoch,
    kRejectedNotCurrentRecipient,
    kRejectedNoLiveChannel,
    kRejectedNoLiveEndpoint,
    kRejectedIneligible,
    kRejectedStaleGain,
    kRejectedConflictingGain,
    kRejectedStaleLoss,
    kRejectedNoCachedGrant,
    kRejectedCacheIdentity,
    kRejectedLedgerPreparation,
    // The owner-Looper must drain an earlier local packet before applying a
    // focus control.  This is a real admission result, not a consumed event.
    kDeferredPendingLocalInput,
  };

  // The policy owner supplies these facts while pinning its exact channel and
  // endpoint owners. The evaluator uses weak identities and owner-verified
  // booleans; it never promotes a weak pointer itself.
  struct Route final {
    std::weak_ptr<InputRoutingState> channel;
    InputRoutingRecipientHandle recipient;
    bool live_channel = false;
    bool live_endpoint = false;
    bool eligible = false;
  };

  // One domain record is shared by all channel caches and is owned by the WMS
  // input policy owner. gain_provenance_present is explicit: an expired weak
  // identity remains a provenance identity and still conflicts with a
  // different channel at the same epoch.
  struct DomainRecord final {
    std::uint64_t highest_observed_epoch = 0;
    bool gain_provenance_present = false;
    std::weak_ptr<InputRoutingState> gain_channel;
    std::uint64_t revision = 0;
  };

  // This record is caller-owned per channel, so the evaluator imposes no
  // arbitrary number-of-windows capacity. It survives exact recipient
  // replacement. Notification identity is weak and separate from the
  // authoritative grant, avoiding a retired recipient/transport lifetime.
  struct ChannelCache final {
    bool present = false;
    std::weak_ptr<InputRoutingState> channel;
    bool grant = false;
    bool revoked = false;
    std::uint64_t focus_epoch = 0;
    std::uint64_t last_gain_epoch = 0;
    std::uint64_t revoked_epoch = 0;
    bool notified = false;
    std::weak_ptr<const InputRoutingRecipient> notified_recipient;
    std::uint64_t notified_epoch = 0;
    bool notified_focused = false;
    std::uint64_t revision = 0;
    // Successful Java focus delivery is a separate readiness fact.  It is
    // never inferred from notified/invoked: Java may have thrown after the
    // callback was invoked.
    bool focus_ready = false;
    std::weak_ptr<const InputRoutingRecipient> ready_recipient;
    std::uint64_t ready_epoch = 0;
    std::uint64_t ready_cache_revision = 0;

    void Bind(const std::weak_ptr<InputRoutingState>& owner) noexcept {
      channel = owner;
      present = true;
    }
    void Reset() noexcept { *this = ChannelCache{}; }
  };

  struct Notification final {
    bool present = false;
    std::weak_ptr<InputRoutingState> channel;
    std::weak_ptr<const InputRoutingRecipient> recipient;
    std::uint64_t epoch = 0;
    bool focused = false;
  };

  struct Plan final {
    enum class Mutation : std::uint8_t { kNone, kGain, kLoss, kReplay };
    Mutation mutation = Mutation::kNone;
    std::weak_ptr<InputRoutingState> channel;
    InputRoutingRecipientHandle recipient;
    std::uint64_t epoch = 0;
    bool focused = false;
    std::uint64_t domain_revision = 0;
    std::uint64_t cache_revision = 0;

    bool Valid() const noexcept { return mutation != Mutation::kNone; }
    bool MutatesCache() const noexcept {
      return mutation == Mutation::kGain || mutation == Mutation::kLoss;
    }
  };

  struct Evaluation final {
    Decision decision = Decision::kRejectedZeroEpoch;
    Plan plan;
    Notification notification;

    bool Accepted() const noexcept {
      return decision == Decision::kAcceptedGain ||
             decision == Decision::kAcceptedLoss ||
             decision == Decision::kAcceptedReplay ||
             decision == Decision::kDuplicate;
    }
    bool ShouldNotify() const noexcept { return notification.present; }
  };

  // Source and current are separate so an old predecessor cannot deliver a
  // late control after exact recipient replacement. Losses intentionally do
  // not require eligibility: WMS hide/revocation must reach the old grant.
  Decision Evaluate(const FocusControl& control, const Route& source,
                    const Route& current, const DomainRecord& domain,
                    const ChannelCache& cache, Evaluation* out) const noexcept {
    if (out == nullptr) return Decision::kRejectedNoLiveChannel;
    *out = Evaluation{};
    if (control.epoch == 0) {
      out->decision = Decision::kRejectedZeroEpoch;
      return out->decision;
    }
    const Decision route_result =
        ValidateRoute(source, current, control.focused);
    if (route_result != Decision::kAcceptedGain) {
      out->decision = route_result;
      return out->decision;
    }
    if (cache.present && !SameOwner(cache.channel, current.channel)) {
      out->decision = Decision::kRejectedCacheIdentity;
      return out->decision;
    }

    if (!control.focused)
      return EvaluateLoss(control.epoch, current, domain, cache, out);

    if (control.epoch < domain.highest_observed_epoch) {
      out->decision = Decision::kRejectedStaleGain;
      return out->decision;
    }
    if (control.epoch == domain.highest_observed_epoch &&
        domain.gain_provenance_present &&
        !SameOwner(domain.gain_channel, current.channel)) {
      out->decision = Decision::kRejectedConflictingGain;
      return out->decision;
    }
    // A duplicate never advances domain/cache revisions or emits a second
    // Java notification. A revoked grant cannot be renewed by its old gain.
    if (cache.present && cache.grant && !cache.revoked &&
        cache.focus_epoch == control.epoch) {
      out->decision = Decision::kDuplicate;
      return out->decision;
    }
    if (cache.present && cache.revoked &&
        control.epoch <= cache.revoked_epoch) {
      out->decision = Decision::kRejectedStaleGain;
      return out->decision;
    }
    PreparePlan(Plan::Mutation::kGain, current, control.epoch, true, domain,
                cache, out);
    out->decision = Decision::kAcceptedGain;
    PrepareNotification(current, control.epoch, true, cache, out);
    return out->decision;
  }

  // Replay receives no epoch or state from its caller. It reads the
  // caller-owned channel cache and rechecks domain epoch/provenance, exact
  // recipient, endpoint lifetime, and eligibility.
  Decision EvaluateReplay(const Route& source, const Route& current,
                          const DomainRecord& domain,
                          const ChannelCache& cache,
                          Evaluation* out) const noexcept {
    if (out == nullptr) return Decision::kRejectedNoLiveChannel;
    *out = Evaluation{};
    const Decision route_result = ValidateRoute(source, current, true);
    if (route_result != Decision::kAcceptedGain) {
      out->decision = route_result;
      return out->decision;
    }
    if (!cache.present || !SameOwner(cache.channel, current.channel) ||
        !cache.grant || cache.revoked || cache.focus_epoch == 0 ||
        cache.focus_epoch != domain.highest_observed_epoch ||
        !domain.gain_provenance_present ||
        !SameOwner(domain.gain_channel, current.channel)) {
      out->decision = cache.present &&
                              !SameOwner(cache.channel, current.channel)
                          ? Decision::kRejectedCacheIdentity
                          : Decision::kRejectedNoCachedGrant;
      return out->decision;
    }
    if (cache.notified &&
        SameOwner(cache.notified_recipient, current.recipient) &&
        cache.notified_epoch == cache.focus_epoch &&
        cache.notified_focused) {
      out->decision = Decision::kDuplicate;
      return out->decision;
    }
    PreparePlan(Plan::Mutation::kReplay, current, cache.focus_epoch, true,
                domain, cache, out);
    out->decision = Decision::kAcceptedReplay;
    PrepareNotification(current, cache.focus_epoch, true, cache, out);
    return out->decision;
  }

  // This is the only authoritative mutation. The caller must first prepare
  // any fallible routing ledger and then call this allocation-free commit.
  bool Commit(DomainRecord& domain, ChannelCache& cache,
              Plan&& plan) const noexcept {
    if (!plan.Valid() || plan.domain_revision != domain.revision ||
        plan.cache_revision != cache.revision)
      return false;
    if (plan.mutation == Plan::Mutation::kReplay) {
      // Exact recipient replacement keeps the cached grant, but readiness is
      // tied to the recipient that Java actually observed.  Replay must
      // establish a fresh successful-delivery fence for the successor.
      if (!cache.focus_ready ||
          !SameOwner(cache.ready_recipient, plan.recipient)) {
        cache.focus_ready = false;
        cache.ready_recipient.reset();
        cache.ready_epoch = 0;
        cache.ready_cache_revision = 0;
      }
      return true;
    }
    if (plan.mutation == Plan::Mutation::kGain) {
      if (cache.present && !SameOwner(cache.channel, plan.channel)) return false;
      if (!cache.present) {
        cache.Bind(plan.channel);
        cache.notified = false;
        cache.notified_recipient.reset();
      }
      cache.grant = true;
      cache.revoked = false;
      cache.focus_ready = false;
      cache.ready_recipient.reset();
      cache.ready_epoch = 0;
      cache.ready_cache_revision = 0;
      cache.focus_epoch = plan.epoch;
      cache.last_gain_epoch = plan.epoch;
      cache.revoked_epoch = 0;
      if (plan.epoch > domain.highest_observed_epoch) {
        domain.highest_observed_epoch = plan.epoch;
        domain.gain_provenance_present = true;
        domain.gain_channel = plan.channel;
      } else if (plan.epoch == domain.highest_observed_epoch &&
                 !domain.gain_provenance_present) {
        domain.gain_provenance_present = true;
        domain.gain_channel = plan.channel;
      }
    } else {
      if (cache.present && !SameOwner(cache.channel, plan.channel)) return false;
      if (!cache.present) {
        // A first loss is still authoritative for this channel. Binding the
        // cache here makes the same loss a duplicate and establishes the
        // barrier that prevents an old gain from being replayed.
        cache.Bind(plan.channel);
        cache.grant = false;
        cache.revoked = true;
        cache.focus_epoch = plan.epoch;
        cache.revoked_epoch = plan.epoch;
      } else if (plan.epoch >= cache.last_gain_epoch) {
        // Apply the loss even when Revoke already cleared grant. The loss
        // epoch remains the channel's authoritative freshness barrier.
        cache.grant = false;
        cache.revoked = true;
        if (plan.epoch > cache.focus_epoch) cache.focus_epoch = plan.epoch;
        if (plan.epoch > cache.revoked_epoch) cache.revoked_epoch = plan.epoch;
      }
      cache.focus_ready = false;
      cache.ready_recipient.reset();
      cache.ready_epoch = 0;
      cache.ready_cache_revision = 0;
      if (plan.epoch > domain.highest_observed_epoch) {
        domain.highest_observed_epoch = plan.epoch;
        domain.gain_provenance_present = false;
        domain.gain_channel.reset();
      }
    }
    ++domain.revision;
    ++cache.revision;
    return true;
  }

  // Hide/geometry/terminal owners use this explicit invalidation. It keeps
  // the old gain epoch as a barrier, so visibility becoming true later cannot
  // resurrect it and a duplicate old gain is rejected. A later loss still
  // evaluates and advances the domain when its epoch is newer.
  static bool Revoke(ChannelCache& cache,
                     std::uint64_t revocation_epoch = 0) noexcept {
    if (!cache.present) return false;
    const std::uint64_t barrier = revocation_epoch > cache.last_gain_epoch
                                      ? revocation_epoch
                                      : cache.last_gain_epoch;
    if (!cache.grant && cache.revoked && barrier <= cache.revoked_epoch &&
        barrier <= cache.focus_epoch)
      return false;
    cache.grant = false;
    cache.revoked = true;
    cache.focus_ready = false;
    cache.ready_recipient.reset();
    cache.ready_epoch = 0;
    cache.ready_cache_revision = 0;
    if (barrier > cache.revoked_epoch) cache.revoked_epoch = barrier;
    if (barrier > cache.focus_epoch) cache.focus_epoch = barrier;
    ++cache.revision;
    return true;
  }

  // Mark notification freshness only after the exact recipient callback has
  // been invoked, regardless of Java delivery/exception result. This method
  // does not retain that recipient and does not advance routing generation.
  static bool CommitNotification(ChannelCache& cache,
                                 const Notification& notification) noexcept {
    if (!notification.present) return true;
    if (!cache.present || !SameOwner(cache.channel, notification.channel) ||
        cache.grant != notification.focused ||
        (notification.focused && cache.focus_epoch != notification.epoch) ||
        (!notification.focused && notification.epoch > cache.focus_epoch))
      return false;
    cache.notified = true;
    cache.notified_recipient = notification.recipient;
    cache.notified_epoch = notification.epoch;
    cache.notified_focused = notification.focused;
    return true;
  }

  // Validate the independent successful-Java-delivery fence.  The caller
  // holds the domain -> channel locks, so this predicate is allocation-free
  // and cannot observe a newer same-channel epoch halfway through a check.
  static bool IsFocusReady(
      const DomainRecord& domain, const ChannelCache& cache,
      const InputRoutingRecipientHandle& recipient, std::uint64_t epoch,
      std::uint64_t cache_revision) noexcept {
    return IsFocusReady(
        domain, cache.channel, recipient, cache.focus_ready, cache.grant,
        cache.revoked, cache.focus_epoch, cache.revision,
        cache.ready_recipient, cache.ready_epoch, cache.ready_cache_revision,
        epoch, cache_revision);
  }

  // Compatibility with the still-present Init owner is strictly pre-authority.
  // An authoritative pending/failed grant must never fall back to this path.
  static bool IsLegacyKeyAdmission(const DomainRecord& domain,
                                  bool authoritative_cache) noexcept {
    return domain.highest_observed_epoch == 0 && !authoritative_cache;
  }

  static bool IsFocusReady(
      const DomainRecord& domain,
      const std::weak_ptr<InputRoutingState>& channel,
      const InputRoutingRecipientHandle& recipient, bool ready, bool grant,
      bool revoked, std::uint64_t focus_epoch,
      std::uint64_t cache_revision,
      const std::weak_ptr<const InputRoutingRecipient>& ready_recipient,
      std::uint64_t ready_epoch, std::uint64_t ready_cache_revision,
      std::uint64_t epoch, std::uint64_t expected_cache_revision) noexcept {
    return recipient != nullptr && epoch != 0 && ready && grant && !revoked && focus_epoch == epoch &&
           cache_revision == expected_cache_revision && ready_epoch == epoch &&
           ready_cache_revision == expected_cache_revision &&
           SameOwner(ready_recipient, recipient) &&
           domain.highest_observed_epoch == epoch &&
           domain.gain_provenance_present &&
           SameOwner(domain.gain_channel, channel);
  }

  // Commit readiness only after the JNI owner has returned a successful
  // delivery result.  A false callback clears readiness; an undelivered
  // callback leaves no new readiness and never promotes invoked to success.
  static bool CommitFocusReadiness(
      DomainRecord& domain, ChannelCache& cache,
      const Notification& notification, bool delivered,
      std::uint64_t expected_cache_revision) noexcept {
    if (!notification.present || notification.epoch == 0 ||
        !cache.present || cache.revision != expected_cache_revision ||
        !SameOwner(cache.channel, notification.channel) ||
        (!SameOwner(cache.ready_recipient, notification.recipient) &&
         cache.focus_ready))
      return false;
    if (!delivered) return true;
    if (!notification.focused) {
      cache.focus_ready = false;
      cache.ready_recipient.reset();
      cache.ready_epoch = 0;
      cache.ready_cache_revision = 0;
      return true;
    }
    if (!cache.grant || cache.revoked || cache.focus_epoch != notification.epoch ||
        !domain.gain_provenance_present ||
        domain.highest_observed_epoch != notification.epoch ||
        !SameOwner(domain.gain_channel, cache.channel))
      return false;
    cache.focus_ready = true;
    cache.ready_recipient = notification.recipient;
    cache.ready_epoch = notification.epoch;
    cache.ready_cache_revision = expected_cache_revision;
    return true;
  }

 private:
  static bool SameOwner(const std::weak_ptr<const InputRoutingRecipient>& left,
                        const std::weak_ptr<const InputRoutingRecipient>& right) noexcept {
    return !left.owner_before(right) && !right.owner_before(left);
  }
  static bool SameOwner(const std::weak_ptr<InputRoutingState>& left,
                        const std::weak_ptr<InputRoutingState>& right) noexcept {
    return !left.owner_before(right) && !right.owner_before(left);
  }
  static bool SameOwner(const std::weak_ptr<const InputRoutingRecipient>& left,
                        const InputRoutingRecipientHandle& right) noexcept {
    const std::weak_ptr<const InputRoutingRecipient> weak_right = right;
    return !left.owner_before(weak_right) && !weak_right.owner_before(left);
  }

  static Decision ValidateRoute(const Route& source, const Route& current,
                                bool require_eligibility) noexcept {
    const std::weak_ptr<const InputRoutingRecipient> source_recipient =
        source.recipient;
    if (!source.recipient || !current.recipient ||
        !SameOwner(source_recipient, current.recipient))
      return Decision::kRejectedNotCurrentRecipient;
    if (!source.live_channel || !current.live_channel)
      return Decision::kRejectedNoLiveChannel;
    if (!SameOwner(source.channel, current.channel))
      return Decision::kRejectedNotCurrentRecipient;
    if (!source.live_endpoint || !current.live_endpoint)
      return Decision::kRejectedNoLiveEndpoint;
    if (require_eligibility && (!source.eligible || !current.eligible))
      return Decision::kRejectedIneligible;
    return Decision::kAcceptedGain;
  }

  static void PreparePlan(Plan::Mutation mutation, const Route& current,
                          std::uint64_t epoch, bool focused,
                          const DomainRecord& domain, const ChannelCache& cache,
                          Evaluation* out) noexcept {
    out->plan.mutation = mutation;
    out->plan.channel = current.channel;
    out->plan.recipient = current.recipient;
    out->plan.epoch = epoch;
    out->plan.focused = focused;
    out->plan.domain_revision = domain.revision;
    out->plan.cache_revision = cache.revision;
  }

  static void PrepareNotification(const Route& current, std::uint64_t epoch,
                                  bool focused, const ChannelCache& cache,
                                  Evaluation* out) noexcept {
    if (cache.notified &&
        SameOwner(cache.notified_recipient, current.recipient) &&
        cache.notified_epoch == epoch && cache.notified_focused == focused)
      return;
    out->notification.present = true;
    out->notification.channel = current.channel;
    out->notification.recipient = current.recipient;
    out->notification.epoch = epoch;
    out->notification.focused = focused;
  }

  static Decision EvaluateLoss(std::uint64_t epoch, const Route& current,
                               const DomainRecord& domain,
                               const ChannelCache& cache,
                               Evaluation* out) noexcept {
    if (cache.present && epoch < cache.last_gain_epoch) {
      out->decision = Decision::kRejectedStaleLoss;
      return out->decision;
    }
    const bool pending_false_notification =
        cache.present && cache.notified && cache.notified_focused &&
        SameOwner(cache.notified_recipient, current.recipient);
    if (cache.present && !cache.grant && epoch <= cache.revoked_epoch &&
        epoch <= domain.highest_observed_epoch && !pending_false_notification) {
      out->decision = Decision::kDuplicate;
      return out->decision;
    }
    PreparePlan(Plan::Mutation::kLoss, current, epoch, false, domain, cache, out);
    out->decision = Decision::kAcceptedLoss;
    // A visibility/terminal revoke may already have cleared grant. The exact
    // recipient still needs one false callback if Java previously observed a
    // true callback; otherwise ViewRoot would retain focus indefinitely.
    if (cache.present && cache.notified && cache.notified_focused)
      PrepareNotification(current, epoch, false, cache, out);
    return out->decision;
  }
};

}  // namespace darwin_art::input
