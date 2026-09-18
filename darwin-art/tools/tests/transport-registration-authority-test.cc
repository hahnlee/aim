#include "runtime/framework/input/transport_registration_authority.h"

#include <atomic>
#include <cassert>
#include <cstdlib>
#include <new>
#include <thread>
#include <vector>

using namespace darwin_art::input;
using Authority = TransportRegistrationAuthority;
using Role = TransportRegistrationRole;
using Result = TransportRegistrationResult;

namespace {

// The failpoint is armed only after all test fixtures are constructed. It is
// allocation-count based so Reserve's entry preallocation and lane
// publication can be tested independently without a production hook.
std::atomic<int> g_fail_at{-1};
std::atomic<int> g_allocation_count{0};

void* Allocate(std::size_t size) {
  const int allocation = g_allocation_count.fetch_add(1) + 1;
  if (allocation == g_fail_at.load()) throw std::bad_alloc();
  if (void* result = std::malloc(size == 0 ? 1 : size)) return result;
  throw std::bad_alloc();
}

}  // namespace

void* operator new(std::size_t size) { return Allocate(size); }
void* operator new[](std::size_t size) { return Allocate(size); }
void operator delete(void* pointer) noexcept { std::free(pointer); }
void operator delete[](void* pointer) noexcept { std::free(pointer); }
void operator delete(void* pointer, std::size_t) noexcept {
  std::free(pointer);
}
void operator delete[](void* pointer, std::size_t) noexcept {
  std::free(pointer);
}

namespace {
struct Observer {
  int calls = 0;
  Authority::Claim* activate = nullptr;
  std::unique_ptr<Authority::Claim>* destroy_old = nullptr;
};
void Available(void* opaque) noexcept {
  auto* observer = static_cast<Observer*>(opaque);
  ++observer->calls;
  if (observer->activate != nullptr)
    assert(observer->activate->BeginRegistration() == Result::kAcquired);
  if (observer->destroy_old != nullptr) observer->destroy_old->reset();
}

struct YieldObserver {
  Authority* authority;
  Authority::Claim* orphan;
  Authority::Claim* receiver;
  int fd;
  int calls = 0;
  bool release = false;
};
void ReceiverWaiting(void* opaque) noexcept {
  auto* observer = static_cast<YieldObserver*>(opaque);
  ++observer->calls;
  // Reentry queries prove the hint runs unlocked and intent is published.
  assert(observer->authority->HasLiveIntent(observer->fd));
  assert(observer->authority->HasAdmittedRegistration(observer->fd));
  assert(*observer->receiver);
  assert(observer->receiver->BeginRegistration() == Result::kDeferred);
  if (observer->release) assert(observer->orphan->ReleaseAfterQuiescence());
}
}

int main() {
  void* looper = reinterpret_cast<void*>(1);
  Authority authority;

  // Failure before Entry construction must not publish an FD intent.
  Authority preallocation_failure;
  Authority::Claim failed_entry, retry_entry;
  g_allocation_count = 0;
  g_fail_at = 1;
  assert(preallocation_failure.Reserve(20, Role::kReceiver, 100, looper,
                                       &failed_entry) == Result::kOutOfMemory);
  g_fail_at = -1;
  assert(!failed_entry);
  assert(!preallocation_failure.HasLiveIntent(20));
  assert(!preallocation_failure.HasAdmittedRegistration(20));
  assert(preallocation_failure.Reserve(20, Role::kReceiver, 101, looper,
                                       &retry_entry) == Result::kAcquired);
  assert(retry_entry.BeginRegistration() == Result::kAcquired);
  assert(retry_entry.ReleaseAfterQuiescence());
  assert(!preallocation_failure.HasLiveIntent(20));

  // Failure after Entry construction, while publishing the lane, must roll
  // back the same way. The next reservation is a clean acquisition.
  Authority publication_failure;
  Authority::Claim failed_publication, retry_publication;
  g_allocation_count = 0;
  g_fail_at = 2;
  assert(publication_failure.Reserve(21, Role::kReceiver, 110, looper,
                                     &failed_publication) == Result::kOutOfMemory);
  g_fail_at = -1;
  assert(!failed_publication);
  assert(!publication_failure.HasLiveIntent(21));
  assert(!publication_failure.HasAdmittedRegistration(21));
  assert(publication_failure.Reserve(21, Role::kReceiver, 111, looper,
                                     &retry_publication) == Result::kAcquired);
  assert(retry_publication.BeginRegistration() == Result::kAcquired);
  assert(retry_publication.ReleaseAfterQuiescence());
  assert(!publication_failure.HasLiveIntent(21));

  auto old = std::make_unique<Authority::Claim>();
  Authority::Claim successor, retired, other_fd;
  assert(authority.Reserve(10, Role::kReceiver, 1, looper, old.get()) == Result::kAcquired);
  assert(old->BeginRegistration() == Result::kAcquired);
  assert(authority.Reserve(10, Role::kRetiredOutput, 2, looper, &retired) == Result::kDeferred);
  assert(authority.Reserve(10, Role::kReceiver, 3, looper, &successor) == Result::kDeferred);
  assert(authority.Reserve(11, Role::kReceiver, 4, looper, &other_fd) == Result::kAcquired);
  assert(other_fd.BeginRegistration() == Result::kAcquired);
  assert(successor.BeginRegistration() == Result::kDeferred);
  Authority::Claim duplicate;
  assert(authority.Reserve(10, Role::kReceiver, 3, looper, &duplicate) == Result::kConflict);
  assert(!duplicate);
  auto observer = std::make_shared<Observer>();
  observer->activate = &successor;
  observer->destroy_old = &old;
  assert(successor.SubscribeAvailable(Available, observer));
  assert(old->ReleaseAfterQuiescence());
  assert(old == nullptr && observer->calls == 1);
  assert(authority.HasAdmittedRegistration(10));
  assert(retired.BeginRegistration() == Result::kDeferred);
  assert(successor.ReleaseAfterQuiescence());
  assert(!authority.HasLiveIntent(10));
  assert(retired.BeginRegistration() == Result::kAcquired);
  assert(retired.ReleaseAfterQuiescence());
  assert(!authority.HasAdmittedRegistration(10));
  assert(other_fd.ReleaseAfterQuiescence());

  // Successor intent invalidates a retired proposal before any AddFd admission.
  Authority::Claim proposed, live;
  assert(authority.Reserve(12, Role::kRetiredOutput, 5, looper, &proposed) == Result::kAcquired);
  auto proposal_observer = std::make_shared<Observer>();
  assert(proposed.SubscribeAvailable(Available, proposal_observer));
  assert(proposal_observer->calls == 1);
  assert(authority.Reserve(12, Role::kReceiver, 6, looper, &live) == Result::kAcquired);
  assert(proposed.BeginRegistration() == Result::kDeferred);
  assert(live.BeginRegistration() == Result::kAcquired);
  assert(live.ReleaseAfterQuiescence());
  assert(proposal_observer->calls == 2);
  assert(proposed.BeginRegistration() == Result::kAcquired);
  assert(proposed.ReleaseAfterQuiescence());

  // An admitted orphan must learn about receiver intent, not just notify the
  // waiting receiver after someone else happens to release the orphan.
  for (const bool release_in_hint : {false, true}) {
    Authority handoff;
    Authority::Claim orphan, receiver;
    assert(handoff.Reserve(22, Role::kRetiredOutput, 120, looper, &orphan) ==
           Result::kAcquired);
    auto yield = std::make_shared<YieldObserver>(
        YieldObserver{&handoff, &orphan, &receiver, 22, 0, release_in_hint});
    assert(orphan.SubscribeReceiverWaiting(ReceiverWaiting, yield));
    assert(orphan.BeginRegistration() == Result::kAcquired && yield->calls == 0);
    assert(handoff.Reserve(22, Role::kReceiver, 121, looper, &receiver) ==
           Result::kDeferred);
    assert(yield->calls == 1);
    if (!release_in_hint) {
      assert(receiver.BeginRegistration() == Result::kDeferred);
      // Hint alone never revokes an admitted claim or permits duplicate AddFd.
      assert(handoff.HasAdmittedRegistration(22));
      assert(orphan.ReleaseAfterQuiescence());
    }
    assert(receiver.BeginRegistration() == Result::kAcquired);
    assert(!receiver.SubscribeReceiverWaiting(ReceiverWaiting, yield));
    assert(receiver.ReleaseAfterQuiescence());
  }

  // Late subscription observes existing intent. Once that intent is canceled,
  // a resubscription must not invent a yield request.
  {
    Authority handoff;
    Authority::Claim orphan, receiver;
    assert(handoff.Reserve(23, Role::kRetiredOutput, 130, looper, &orphan) == Result::kAcquired);
    assert(orphan.BeginRegistration() == Result::kAcquired);
    assert(handoff.Reserve(23, Role::kReceiver, 131, looper, &receiver) == Result::kDeferred);
    auto yield = std::make_shared<YieldObserver>(YieldObserver{&handoff, &orphan, &receiver, 23});
    g_allocation_count = 0;
    g_fail_at = 1;
    assert(orphan.SubscribeReceiverWaiting(ReceiverWaiting, yield) && yield->calls == 1);
    assert(g_allocation_count == 0);
    g_fail_at = -1;
    assert(receiver.ReleaseAfterQuiescence());
    assert(orphan.SubscribeReceiverWaiting(ReceiverWaiting, yield) && yield->calls == 1);
    assert(orphan.ReleaseAfterQuiescence());
  }

  // The authority never keeps a notification target alive through a cycle.
  {
    Authority handoff;
    Authority::Claim orphan, receiver;
    assert(handoff.Reserve(24, Role::kRetiredOutput, 140, looper, &orphan) == Result::kAcquired);
    assert(orphan.BeginRegistration() == Result::kAcquired);
    auto token = std::make_shared<int>(0);
    std::weak_ptr<int> weak = token;
    assert(orphan.SubscribeReceiverWaiting([](void*) noexcept { assert(false); }, token));
    token.reset();
    assert(weak.expired());
    assert(handoff.Reserve(24, Role::kReceiver, 141, looper, &receiver) == Result::kDeferred);
    assert(receiver.BeginRegistration() == Result::kDeferred);
    assert(orphan.ReleaseAfterQuiescence());
    assert(receiver.BeginRegistration() == Result::kAcquired);
    assert(receiver.ReleaseAfterQuiescence());
  }

  // Losing an admitted lease is not evidence of FD quiescence. Quarantine it.
  {
    Authority::Claim abandoned;
    assert(authority.Reserve(13, Role::kReceiver, 7, looper, &abandoned) == Result::kAcquired);
    assert(abandoned.BeginRegistration() == Result::kAcquired);
  }
  Authority::Claim blocked;
  assert(authority.Reserve(13, Role::kReceiver, 8, looper, &blocked) == Result::kDeferred);
  assert(blocked.BeginRegistration() == Result::kDeferred);
  assert(authority.HasAdmittedRegistration(13));
  assert(blocked.ReleaseAfterQuiescence());

  // Concurrent construction may reserve many intents, but only one admission.
  Authority concurrent;
  std::vector<Authority::Claim> claims(8);
  std::vector<std::thread> threads;
  std::atomic<int> admitted{0};
  for (std::size_t i = 0; i != claims.size(); ++i) {
    threads.emplace_back([&, i] {
      const auto reserved = concurrent.Reserve(14, Role::kReceiver, i + 1,
                                               looper, &claims[i]);
      assert(reserved == Result::kAcquired || reserved == Result::kDeferred);
      if (claims[i].BeginRegistration() == Result::kAcquired) ++admitted;
    });
  }
  for (auto& thread : threads) thread.join();
  assert(admitted == 1);
  for (auto& claim : claims) assert(claim.ReleaseAfterQuiescence());
  assert(!concurrent.HasLiveIntent(14));
}
