#include "window_input_endpoint_lease.h"

#include <atomic>
#include <limits>
#include <mutex>
#include <new>
#include <unordered_map>
#include <utility>

namespace darwin_art::framework::wm {
namespace {

std::atomic<WindowInputEndpointLeaseToken> g_next_token{1};

bool AllocateToken(WindowInputEndpointLeaseToken* token) noexcept {
  auto candidate = g_next_token.load(std::memory_order_relaxed);
  for (;;) {
    if (candidate == 0 || candidate == std::numeric_limits<WindowInputEndpointLeaseToken>::max())
      return false;
    if (g_next_token.compare_exchange_weak(candidate, candidate + 1,
                                            std::memory_order_relaxed,
                                            std::memory_order_relaxed)) {
      *token = candidate;
      return true;
    }
  }
}

}  // namespace

struct WindowInputEndpointLease::State {
  mutable std::mutex mutex;
  std::unordered_map<WindowInputEndpointLeaseToken,
                     std::shared_ptr<darwin_art::input::InputChannelResources>>
      resources;
  bool closed = false;
};

std::shared_ptr<WindowInputEndpointLease> WindowInputEndpointLease::Create() noexcept {
  try {
    return std::shared_ptr<WindowInputEndpointLease>(new WindowInputEndpointLease());
  } catch (const std::bad_alloc&) {
    return nullptr;
  }
}

WindowInputEndpointLease::WindowInputEndpointLease()
    : state_(std::make_shared<State>()) {}

WindowInputEndpointLease::~WindowInputEndpointLease() { (void)Close(); }

WindowInputEndpointLeaseAcquireResult WindowInputEndpointLease::Acquire(
    const std::shared_ptr<darwin_art::input::InputChannelResources>& resources) {
  if (resources == nullptr || resources->Endpoint() == nullptr ||
      resources->Endpoint()->Transport() == nullptr)
    return {WindowInputEndpointLeaseAcquireStatus::kInvalidArgument, 0};
  auto state = state_;
  std::unordered_map<WindowInputEndpointLeaseToken,
                     std::shared_ptr<darwin_art::input::InputChannelResources>> detached;
  WindowInputEndpointLeaseAcquireStatus status =
      WindowInputEndpointLeaseAcquireStatus::kAcquired;
  WindowInputEndpointLeaseToken token = 0;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->closed)
      return {WindowInputEndpointLeaseAcquireStatus::kClosed, 0};
    if (!AllocateToken(&token)) {
      state->closed = true;
      detached.swap(state->resources);
      status = WindowInputEndpointLeaseAcquireStatus::kExhausted;
    }
    if (status == WindowInputEndpointLeaseAcquireStatus::kExhausted) {
      // The detached map is cleared after the mutex is released.
    } else {
      try {
        state->resources.emplace(token, resources);
      } catch (const std::bad_alloc&) {
        status = WindowInputEndpointLeaseAcquireStatus::kOutOfMemory;
      }
    }
  }
  if (status == WindowInputEndpointLeaseAcquireStatus::kExhausted) {
    detached.clear();
    return {status, 0};
  }
  if (status == WindowInputEndpointLeaseAcquireStatus::kOutOfMemory)
    return {status, 0};
  return {status, token};
}

darwin_art::input::InputTransportStatus WindowInputEndpointLease::PublishWindow(
    WindowInputEndpointLeaseToken token, std::int32_t left, std::int32_t top,
    std::int32_t right, std::int32_t bottom, bool visible,
    std::uint32_t input_flags) {
  auto state = state_;
  std::shared_ptr<darwin_art::input::InputChannelResources> resources;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    const auto found = state->resources.find(token);
    if (token == 0 || found == state->resources.end())
      return darwin_art::input::InputTransportStatus::kTerminal;
    resources = found->second;
  }
  const auto endpoint = resources->Endpoint();
  return endpoint == nullptr
      ? darwin_art::input::InputTransportStatus::kTerminal
      : endpoint->PublishWindow(left, top, right, bottom, visible, input_flags);
}

darwin_art::input::InputTransportStatus WindowInputEndpointLease::PublishFocus(
    WindowInputEndpointLeaseToken token, std::uint64_t epoch, bool focused) {
  if (epoch == 0)
    return darwin_art::input::InputTransportStatus::kTerminal;
  auto state = state_;
  std::shared_ptr<darwin_art::input::InputChannelResources> resources;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    const auto found = state->resources.find(token);
    if (token == 0 || found == state->resources.end())
      return darwin_art::input::InputTransportStatus::kTerminal;
    resources = found->second;
  }
  const auto endpoint = resources->Endpoint();
  return endpoint == nullptr
      ? darwin_art::input::InputTransportStatus::kTerminal
      : endpoint->PublishFocus(epoch, focused);
}

darwin_art::input::InputTransportStatus WindowInputEndpointLease::Flush(
    WindowInputEndpointLeaseToken token) {
  auto state = state_;
  std::shared_ptr<darwin_art::input::InputChannelResources> resources;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    const auto found = state->resources.find(token);
    if (token == 0 || found == state->resources.end())
      return darwin_art::input::InputTransportStatus::kTerminal;
    resources = found->second;
  }
  const auto endpoint = resources->Endpoint();
  const auto transport = endpoint == nullptr ? nullptr : endpoint->Transport();
  return transport == nullptr
      ? darwin_art::input::InputTransportStatus::kTerminal
      : darwin_art::input::FlushInputTransport(transport.get());
}

darwin_art::input::InputTransportTxFenceStatus
WindowInputEndpointLease::QueryAcceptedTx(WindowInputEndpointLeaseToken token) {
  auto state = state_;
  std::shared_ptr<darwin_art::input::InputChannelResources> resources;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    const auto found = state->resources.find(token);
    if (token == 0 || found == state->resources.end())
      return darwin_art::input::InputTransportTxFenceStatus::kInvalid;
    resources = found->second;
  }
  const auto endpoint = resources->Endpoint();
  const auto transport = endpoint == nullptr ? nullptr : endpoint->Transport();
  if (transport == nullptr)
    return darwin_art::input::InputTransportTxFenceStatus::kInvalid;
  const auto fence = transport->CaptureAcceptedTxFence();
  return transport->QueryTxFence(fence);
}

bool WindowInputEndpointLease::TerminateAndQuiesce(WindowInputEndpointLeaseToken token) {
  auto state = state_;
  std::shared_ptr<darwin_art::input::InputChannelResources> resources;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    const auto found = state->resources.find(token);
    if (token == 0 || state->closed || found == state->resources.end()) return false;
    resources = found->second;
  }
  const auto endpoint = resources->Endpoint();
  const auto transport = endpoint == nullptr ? nullptr : endpoint->Transport();
  return transport != nullptr &&
      darwin_art::input::TerminateInputTransportTxAndQuiesce(transport.get());
}

bool WindowInputEndpointLease::Release(WindowInputEndpointLeaseToken token) noexcept {
  if (token == 0) return false;
  auto state = state_;
  std::shared_ptr<darwin_art::input::InputChannelResources> detached;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    const auto found = state->resources.find(token);
    if (found == state->resources.end()) return false;
    detached = std::move(found->second);
    state->resources.erase(found);
  }
  detached.reset();
  return true;
}

bool WindowInputEndpointLease::Close() noexcept {
  auto state = state_;
  std::unordered_map<WindowInputEndpointLeaseToken,
                     std::shared_ptr<darwin_art::input::InputChannelResources>> detached;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->closed) return false;
    state->closed = true;
    detached.swap(state->resources);
  }
  detached.clear();
  return true;
}

bool WindowInputEndpointLease::closed() const noexcept {
  auto state = state_;
  std::lock_guard<std::mutex> lock(state->mutex);
  return state->closed;
}

std::size_t WindowInputEndpointLease::size() const noexcept {
  auto state = state_;
  std::lock_guard<std::mutex> lock(state->mutex);
  return state->resources.size();
}

}  // namespace darwin_art::framework::wm
