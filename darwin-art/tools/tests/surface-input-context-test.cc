#include "../../compat/input/surface_input_sink.h"
#include "../../runtime/framework/input/surface_input_context.h"

#include <atomic>
#include <cassert>
#include <cstddef>
#include <cstdlib>
#include <memory>
#include <new>
#include <thread>
#include <utility>
#include <vector>

namespace {

std::atomic<bool> fail_next_allocation{false};

}  // namespace

// A test-only definition deliberately follows the production header's opaque
// forward declaration.  The context must be usable while DesktopRootEvents
// is incomplete and must retain/release the exact shared owner when complete.
namespace darwin_art::window {
class DesktopRootEvents final {
 public:
  explicit DesktopRootEvents(std::atomic<int>* destructions) noexcept
      : destructions_(destructions) {}
  ~DesktopRootEvents() { destructions_->fetch_add(1); }

 private:
  std::atomic<int>* const destructions_;
};
}  // namespace darwin_art::window

namespace darwin_art::input {
// An opaque lifetime fixture, not an authority-policy replacement.
class RootKeyAuthority final {
 public:
  explicit RootKeyAuthority(std::atomic<int>* destructions) noexcept
      : destructions_(destructions) {}
  ~RootKeyAuthority() { destructions_->fetch_add(1); }
 private:
  std::atomic<int>* const destructions_;
};
}  // namespace darwin_art::input

void* operator new(std::size_t size) {
  if (fail_next_allocation.exchange(false)) throw std::bad_alloc();
  if (void* result = std::malloc(size)) return result;
  throw std::bad_alloc();
}

void* operator new[](std::size_t size) {
  if (fail_next_allocation.exchange(false)) throw std::bad_alloc();
  if (void* result = std::malloc(size)) return result;
  throw std::bad_alloc();
}

void* operator new(std::size_t size, const std::nothrow_t&) noexcept {
  if (fail_next_allocation.exchange(false)) return nullptr;
  return std::malloc(size);
}

void* operator new[](std::size_t size, const std::nothrow_t&) noexcept {
  if (fail_next_allocation.exchange(false)) return nullptr;
  return std::malloc(size);
}

void operator delete(void* pointer) noexcept { std::free(pointer); }
void operator delete[](void* pointer) noexcept { std::free(pointer); }
void operator delete(void* pointer, std::size_t) noexcept {
  std::free(pointer);
}
void operator delete[](void* pointer, std::size_t) noexcept {
  std::free(pointer);
}
void operator delete(void* pointer, const std::nothrow_t&) noexcept {
  std::free(pointer);
}
void operator delete[](void* pointer, const std::nothrow_t&) noexcept {
  std::free(pointer);
}

namespace {

using Context = darwin_art::input::SurfaceInputContext;
using Root = darwin_art::window::DesktopRootEvents;
using Binding = DarwinArtSurfaceInputSinkBinding;

DarwinArtSurfaceInputResult PointerSink(
    void*, const DarwinArtPointerEventV2*) {
  return DARWIN_ART_SURFACE_INPUT_QUEUED;
}

DarwinArtSurfaceInputResult KeySink(void*, const DarwinArtKeyEventV1*) {
  return DARWIN_ART_SURFACE_INPUT_QUEUED;
}

DarwinArtSurfaceInputSink MakeSink(void* context) {
  return DarwinArtSurfaceInputSink{
      .version = 1,
      .size = sizeof(DarwinArtSurfaceInputSink),
      .context = context,
      .retain_context = &Context::Retain,
      .release_context = &Context::Release,
      .pointer = &PointerSink,
      .key = &KeySink,
  };
}

void TestNullAndAllocationFailureLeaveRootWithCaller() {
  assert(Context::Create(std::shared_ptr<Root>()) == nullptr);

  std::atomic<int> destructions{0};
  auto root = std::make_shared<Root>(&destructions);
  fail_next_allocation.store(true);
  Context* failed = Context::Create(root);
  assert(failed == nullptr);
  assert(root != nullptr);
  assert(destructions.load() == 0);
  root.reset();
  assert(destructions.load() == 1);
}

void TestUnpublishedCreatorReleaseCleansContext() {
  std::atomic<int> destructions{0};
  auto root = std::make_shared<Root>(&destructions);
  Context* context = Context::Create(root);
  assert(context != nullptr);
  root.reset();
  assert(context->root() != nullptr);
  Context::Release(context);  // The creator never published a sink.
  assert(destructions.load() == 1);
}

void TestExactAuthorityRetentionAndFailedPublication() {
  std::atomic<int> root_destructions{0};
  std::atomic<int> authority_destructions{0};
  auto root = std::make_shared<Root>(&root_destructions);
  auto authority = std::make_shared<darwin_art::input::RootKeyAuthority>(
      &authority_destructions);
  fail_next_allocation.store(true);
  assert(Context::Create(root, authority) == nullptr);
  assert(root_destructions.load() == 0);
  assert(authority_destructions.load() == 0);
  auto* context = Context::Create(root, authority);
  assert(context != nullptr);
  assert(context->key_authority() == authority);
  auto binding = MakeDarwinArtSurfaceInputSinkBinding(MakeSink(context));
  Context::Release(context);
  root.reset();
  authority.reset();
  assert(authority_destructions.load() == 0);
  binding.reset();
  assert(authority_destructions.load() == 1);
  assert(root_destructions.load() == 1);
}

void TestBindingRetainsContextAcrossSnapshot() {
  std::atomic<int> destructions{0};
  auto root = std::make_shared<Root>(&destructions);
  Context* context = Context::Create(root);
  assert(context != nullptr);
  root.reset();

  auto binding = MakeDarwinArtSurfaceInputSinkBinding(MakeSink(context));
  assert(binding != nullptr);
  Context::Release(context);  // Drop the creator reference.
  assert(destructions.load() == 0);

  auto snapshot = binding;
  binding.reset();
  assert(destructions.load() == 0);
  assert(snapshot->sink.context != nullptr);
  snapshot.reset();
  assert(destructions.load() == 1);
}

void TestBindingAllocationFailureDoesNotRetainOrPublish() {
  std::atomic<int> destructions{0};
  auto root = std::make_shared<Root>(&destructions);
  Context* context = Context::Create(root);
  assert(context != nullptr);
  root.reset();

  auto published = MakeDarwinArtSurfaceInputSinkBinding(MakeSink(context));
  assert(published != nullptr);
  fail_next_allocation.store(true);
  bool failed = false;
  try {
    auto not_published = MakeDarwinArtSurfaceInputSinkBinding(MakeSink(context));
    (void)not_published;
  } catch (const std::bad_alloc&) {
    failed = true;
  }
  assert(failed);

  // The failed replacement allocated no binding, so it could not retain or
  // release the context and the published snapshot remains valid.
  assert(published->sink.context == context);
  Context::Release(context);
  assert(destructions.load() == 0);
  published.reset();
  assert(destructions.load() == 1);
}

void TestConcurrentIndependentSnapshotTails() {
  std::atomic<int> destructions{0};
  auto root = std::make_shared<Root>(&destructions);
  Context* context = Context::Create(root);
  assert(context != nullptr);
  root.reset();

  auto binding = MakeDarwinArtSurfaceInputSinkBinding(MakeSink(context));
  constexpr std::size_t kTailCount = 32;
  std::vector<std::shared_ptr<Binding>> tails;
  tails.reserve(kTailCount);
  for (std::size_t i = 0; i < kTailCount; ++i) tails.push_back(binding);
  binding.reset();
  Context::Release(context);
  assert(destructions.load() == 0);

  std::vector<std::thread> workers;
  workers.reserve(kTailCount);
  for (std::size_t i = 0; i < kTailCount; ++i) {
    workers.emplace_back([&tails, i] { tails[i].reset(); });
  }
  for (auto& worker : workers) worker.join();
  assert(destructions.load() == 1);
}

}  // namespace

int main() {
  TestNullAndAllocationFailureLeaveRootWithCaller();
  TestUnpublishedCreatorReleaseCleansContext();
  TestExactAuthorityRetentionAndFailedPublication();
  TestBindingRetainsContextAcrossSnapshot();
  TestBindingAllocationFailureDoesNotRetainOrPublish();
  TestConcurrentIndependentSnapshotTails();
  return 0;
}
