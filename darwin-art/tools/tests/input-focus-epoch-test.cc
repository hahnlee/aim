#include "runtime/framework/input/input_focus_epoch.h"

#include <cassert>
#include <cstdio>
#include <memory>

using Evaluator = darwin_art::input::InputFocusEpochEvaluator;
using Route = Evaluator::Route;
using Evaluation = Evaluator::Evaluation;
using Domain = Evaluator::DomainRecord;
using Cache = Evaluator::ChannelCache;
using darwin_art::input::FocusControl;
using darwin_art::input::InputRoutingEndpointHandle;
using darwin_art::input::InputRoutingHandle;
using darwin_art::input::InputRoutingRecipientHandle;
using darwin_art::input::CreateInputRoutingState;
using darwin_art::input::PrepareInputRoutingRecipient;

static InputRoutingRecipientHandle Recipient(const InputRoutingHandle& channel,
                                             std::uint64_t id) {
  return PrepareInputRoutingRecipient(channel, id, InputRoutingEndpointHandle{});
}

static Route LiveRoute(const InputRoutingHandle& channel,
                       const InputRoutingRecipientHandle& recipient,
                       bool eligible = true) {
  return Route{channel, recipient, true, true, eligible};
}

static void Gain(Evaluator* evaluator, Domain* domain, Cache* cache,
                 const Route& route, std::uint64_t epoch,
                 bool mark_notification = true) {
  Evaluation result;
  assert(evaluator->Evaluate(FocusControl{epoch, true}, route, route, *domain,
                             *cache, &result) == Evaluator::Decision::kAcceptedGain);
  assert(result.plan.MutatesCache());
  assert(evaluator->Commit(*domain, *cache, std::move(result.plan)));
  if (mark_notification)
    assert(Evaluator::CommitNotification(*cache, result.notification));
}

int main() {
  auto a = CreateInputRoutingState();
  auto b = CreateInputRoutingState();
  auto c = CreateInputRoutingState();
  const auto a_old = Recipient(a, 1);
  const auto a_new = Recipient(a, 2);
  const auto b_recipient = Recipient(b, 3);
  const auto c_recipient = Recipient(c, 4);
  const Route a_old_route = LiveRoute(a, a_old);
  const Route a_new_route = LiveRoute(a, a_new);
  const Route b_route = LiveRoute(b, b_recipient);
  const Route c_route = LiveRoute(c, c_recipient);

  {
    Evaluator evaluator;
    Domain domain;
    Cache cache;
    Evaluation result;
    assert(evaluator.Evaluate(FocusControl{0, true}, a_old_route, a_old_route,
                              domain, cache, &result) ==
           Evaluator::Decision::kRejectedZeroEpoch);
    assert(!result.plan.Valid());
  }

  {
    Evaluator evaluator;
    Domain domain;
    Cache a_cache;
    Gain(&evaluator, &domain, &a_cache, a_old_route, 8);
    const auto revision = domain.revision;
    Evaluation result;
    assert(evaluator.Evaluate(FocusControl{7, true}, a_old_route, a_old_route,
                              domain, a_cache, &result) ==
           Evaluator::Decision::kRejectedStaleGain);
    assert(evaluator.Evaluate(FocusControl{8, true}, a_old_route, a_old_route,
                              domain, a_cache, &result) ==
           Evaluator::Decision::kDuplicate);
    assert(domain.revision == revision && !result.ShouldNotify());
    assert(evaluator.Evaluate(FocusControl{8, true}, b_route, b_route, domain,
                              Cache{}, &result) ==
           Evaluator::Decision::kRejectedConflictingGain);
  }

  {
    // An expired provenance weak pointer remains an identity and conflicts.
    Evaluator evaluator;
    Domain domain;
    Cache expired_cache;
    auto expired = CreateInputRoutingState();
    auto expired_recipient = Recipient(expired, 5);
    const Route expired_route = LiveRoute(expired, expired_recipient);
    Gain(&evaluator, &domain, &expired_cache, expired_route, 8);
    expired_recipient.reset();
    expired.reset();
    Evaluation result;
    Cache b_cache;
    assert(evaluator.Evaluate(FocusControl{8, true}, b_route, b_route, domain,
                              b_cache, &result) ==
           Evaluator::Decision::kRejectedConflictingGain);
  }

  {
    // Per-channel losses remain effective after newer gains elsewhere.
    Evaluator evaluator;
    Domain domain;
    Cache a_cache;
    Cache b_cache;
    Cache c_cache;
    Gain(&evaluator, &domain, &a_cache, a_old_route, 8);
    Gain(&evaluator, &domain, &b_cache, b_route, 10);
    Evaluation result;
    assert(evaluator.Evaluate(FocusControl{11, false}, b_route, b_route, domain,
                              b_cache, &result) == Evaluator::Decision::kAcceptedLoss);
    assert(evaluator.Commit(domain, b_cache, std::move(result.plan)));
    assert(evaluator.Evaluate(FocusControl{11, true}, c_route, c_route, domain,
                              c_cache, &result) == Evaluator::Decision::kAcceptedGain);
    assert(evaluator.Commit(domain, c_cache, std::move(result.plan)));
    assert(evaluator.Evaluate(FocusControl{10, false}, a_old_route, a_old_route,
                              domain, a_cache, &result) ==
           Evaluator::Decision::kAcceptedLoss);
    assert(evaluator.Commit(domain, a_cache, std::move(result.plan)));
    assert(evaluator.EvaluateReplay(a_old_route, a_old_route, domain, a_cache,
                                    &result) == Evaluator::Decision::kRejectedNoCachedGrant);
  }

  {
    // A newer gain protects its grant from a stale loss.
    Evaluator evaluator;
    Domain domain;
    Cache a_cache;
    Gain(&evaluator, &domain, &a_cache, a_old_route, 8);
    Evaluation result;
    assert(evaluator.Evaluate(FocusControl{10, false}, a_old_route, a_old_route,
                              domain, a_cache, &result) == Evaluator::Decision::kAcceptedLoss);
    assert(evaluator.Commit(domain, a_cache, std::move(result.plan)));
    Gain(&evaluator, &domain, &a_cache, a_old_route, 12);
    assert(evaluator.Evaluate(FocusControl{10, false}, a_old_route, a_old_route,
                              domain, a_cache, &result) ==
           Evaluator::Decision::kRejectedStaleLoss);
    assert(evaluator.EvaluateReplay(a_old_route, a_old_route, domain, a_cache,
                                    &result) == Evaluator::Decision::kDuplicate);
  }

  {
    // Explicit visibility revocation survives hide -> show and old duplicate
    // gains. A newer loss still gets evaluated after that revocation.
    Evaluator evaluator;
    Domain domain;
    Cache a_cache;
    Gain(&evaluator, &domain, &a_cache, a_old_route, 8);
    Evaluator::Revoke(a_cache, 9);
    Evaluation result;
    const Route hidden = LiveRoute(a, a_old, false);
    assert(evaluator.Evaluate(FocusControl{10, false}, hidden, hidden, domain,
                              a_cache, &result) == Evaluator::Decision::kAcceptedLoss);
    assert(result.ShouldNotify() && !result.notification.focused);
    assert(evaluator.Commit(domain, a_cache, std::move(result.plan)));
    assert(Evaluator::CommitNotification(a_cache, result.notification));
    assert(evaluator.Evaluate(FocusControl{10, false}, hidden, hidden, domain,
                              a_cache, &result) == Evaluator::Decision::kDuplicate);
    assert(!result.ShouldNotify());
    const Route shown = LiveRoute(a, a_old, true);
    assert(evaluator.EvaluateReplay(shown, shown, domain, a_cache, &result) ==
           Evaluator::Decision::kRejectedNoCachedGrant);
    assert(evaluator.Evaluate(FocusControl{8, true}, shown, shown, domain,
                              a_cache, &result) == Evaluator::Decision::kRejectedStaleGain);
    Gain(&evaluator, &domain, &a_cache, shown, 12);
    assert(evaluator.EvaluateReplay(shown, shown, domain, a_cache, &result) ==
           Evaluator::Decision::kDuplicate);
  }

  {
    // Even without a preceding gain, the first loss is cached and deduplicated.
    Evaluator evaluator;
    Domain domain;
    Cache a_cache;
    Evaluation result;
    assert(evaluator.Evaluate(FocusControl{3, false}, a_old_route, a_old_route,
                              domain, a_cache, &result) == Evaluator::Decision::kAcceptedLoss);
    assert(evaluator.Commit(domain, a_cache, std::move(result.plan)));
    assert(evaluator.Evaluate(FocusControl{3, false}, a_old_route, a_old_route,
                              domain, a_cache, &result) == Evaluator::Decision::kDuplicate);
    assert(!result.ShouldNotify());
  }

  {
    // Replacement retains channel authority but changes notification identity.
    Evaluator evaluator;
    Domain domain;
    Cache a_cache;
    Gain(&evaluator, &domain, &a_cache, a_old_route, 8);
    Evaluation result;
    assert(evaluator.EvaluateReplay(a_new_route, a_new_route, domain, a_cache,
                                    &result) == Evaluator::Decision::kAcceptedReplay);
    assert(result.ShouldNotify());
    assert(evaluator.Commit(domain, a_cache, std::move(result.plan)));
    assert(Evaluator::CommitNotification(a_cache, result.notification));
    assert(evaluator.EvaluateReplay(a_old_route, a_new_route, domain, a_cache,
                                    &result) == Evaluator::Decision::kRejectedNotCurrentRecipient);
    assert(evaluator.Evaluate(FocusControl{8, true}, a_old_route, a_new_route,
                              domain, a_cache, &result) ==
           Evaluator::Decision::kRejectedNotCurrentRecipient);
  }

  {
    // Ineligible input is consumed immediately; no deferred epoch is stored.
    Evaluator evaluator;
    Domain domain;
    Cache a_cache;
    const Route ineligible = LiveRoute(a, a_old, false);
    Evaluation result;
    Route dead_endpoint = a_old_route;
    dead_endpoint.live_endpoint = false;
    assert(evaluator.Evaluate(FocusControl{20, true}, dead_endpoint,
                              dead_endpoint, domain, a_cache, &result) ==
           Evaluator::Decision::kRejectedNoLiveEndpoint);
    assert(evaluator.Evaluate(FocusControl{20, true}, ineligible, ineligible,
                              domain, a_cache, &result) ==
           Evaluator::Decision::kRejectedIneligible);
    assert(!result.plan.Valid());
    assert(evaluator.Evaluate(FocusControl{20, true}, a_old_route, a_old_route,
                              domain, a_cache, &result) == Evaluator::Decision::kAcceptedGain);
  }

  std::puts("input-focus-epoch: PASS malformed/stale/duplicate/conflict/expired-provenance/cross-channel-loss/newer-gain/revocation/hide-loss/replacement-replay/predecessor/ineligible");
}
