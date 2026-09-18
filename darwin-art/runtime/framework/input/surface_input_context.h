#pragma once

#include <atomic>
#include <cstdint>
#include <exception>
#include <limits>
#include <memory>
#include <new>
#include <utility>

namespace darwin_art::window {
class DesktopRootEvents;
}  // namespace darwin_art::window

namespace darwin_art::input {
class RootKeyAuthority;
class RootKeyIngress;

// Retained native input callback context for one surface.  The context owns
// the exact desktop-root event owner and its canonical Android key-authority
// facade. The context itself owns no focus policy or surface resources.
//
// Create adopts one creator reference.  The surface provider takes another
// reference while publishing the context through DarwinArtSurfaceInputSink;
// its release callback eventually drops that provider reference.  The caller
// must always release the creator reference after publication (or failure).
class SurfaceInputContext final {
 public:
  // Returns an owner with one creator reference, or null for a null root or
  // allocation failure.  The root is never replaced by a snapshot or policy
  // grant: root() returns this same shared owner.
  static SurfaceInputContext* Create(
      std::shared_ptr<darwin_art::window::DesktopRootEvents> root,
      std::shared_ptr<RootKeyAuthority> key_authority = {},
      std::shared_ptr<RootKeyIngress> key_ingress = {}) noexcept {
    if (root == nullptr) return nullptr;
    return new (std::nothrow)
        SurfaceInputContext(std::move(root), std::move(key_authority),
                            std::move(key_ingress));
  }

  SurfaceInputContext(const SurfaceInputContext&) = delete;
  SurfaceInputContext& operator=(const SurfaceInputContext&) = delete;

  // These callbacks have C-ABI-compatible signatures for
  // DarwinArtSurfaceInputSink and are safe to call from any callback thread.
  static void Retain(void* opaque) noexcept {
    if (opaque == nullptr) return;
    auto* context = static_cast<SurfaceInputContext*>(opaque);
    uint32_t references = context->references_.load(std::memory_order_relaxed);
    for (;;) {
      if (references == 0 ||
          references == std::numeric_limits<uint32_t>::max()) {
        std::terminate();
      }
      if (context->references_.compare_exchange_weak(
              references, references + 1, std::memory_order_relaxed,
              std::memory_order_relaxed)) {
        return;
      }
    }
  }

  static void Release(void* opaque) noexcept {
    if (opaque == nullptr) return;
    auto* context = static_cast<SurfaceInputContext*>(opaque);
    const uint32_t previous = context->references_.fetch_sub(
        1, std::memory_order_acq_rel);
    if (previous == 0) {
      std::terminate();
    }
    if (previous == 1) {
      delete context;
    }
  }

  // Returns a shared snapshot of the exact root owner.  This does not grant
  // focus, route input, or derive authority from the host stamp.
  std::shared_ptr<darwin_art::window::DesktopRootEvents> root() const noexcept {
    return root_;
  }

  // Retention is not a grant. Routing still needs an authenticated WMS decision
  // and successful receiver-focus fence. Fixtures may leave this owner absent.
  std::shared_ptr<RootKeyAuthority> key_authority() const noexcept {
    return key_authority_;
  }
  std::shared_ptr<RootKeyIngress> key_ingress() const noexcept {
    return key_ingress_;
  }

 private:
  explicit SurfaceInputContext(
      std::shared_ptr<darwin_art::window::DesktopRootEvents> root,
      std::shared_ptr<RootKeyAuthority> key_authority,
      std::shared_ptr<RootKeyIngress> key_ingress) noexcept
      : root_(std::move(root)), key_authority_(std::move(key_authority)),
        key_ingress_(std::move(key_ingress)),
        references_(1) {}

  ~SurfaceInputContext() = default;

  const std::shared_ptr<darwin_art::window::DesktopRootEvents> root_;
  const std::shared_ptr<RootKeyAuthority> key_authority_;
  // Last binding cancels ingress before authority/root references are dropped.
  const std::shared_ptr<RootKeyIngress> key_ingress_;
  std::atomic<uint32_t> references_{1};
};

}  // namespace darwin_art::input
