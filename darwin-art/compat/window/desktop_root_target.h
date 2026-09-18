#pragma once

#include "desktop_root_events.h"

#ifdef __cplusplus
extern "C" {
#endif

// Observer ABI shared by the surface compatibility adapter and the retained
// process desktop-root target.  The target owns the original AppKit window;
// the callback remains a host-fact notification and does not grant Android
// focus.
typedef struct DarwinArtDesktopRootObserver {
  DarwinArtDesktopRootEventCallback callback;
  struct DarwinArtDesktopRootEventContext context;
} DarwinArtDesktopRootObserver;

#ifdef __cplusplus
}

namespace darwin_art::window {

// Strong, process-owned capability for the exact AppKit root created during
// surface initialization.  The object contains no Android focus or policy;
// its only host authority is the retained original NSWindow and its
// DesktopRootEvents owner.
class DesktopRootTarget final
    : public std::enable_shared_from_this<DesktopRootTarget> {
 public:
  // Internal construction path used by InitializeDesktopRoot.  Publication
  // is separate so allocation failure can unwind before the catalog changes.
  static std::shared_ptr<DesktopRootTarget> Create(
      std::shared_ptr<DesktopRootEvents> owner, NSWindow* window) noexcept;

  DesktopRootTarget(const DesktopRootTarget&) = delete;
  DesktopRootTarget& operator=(const DesktopRootTarget&) = delete;

  uint64_t incarnation() const noexcept;
  bool IsRetired() const noexcept;

  // Any-thread retention of this capability's exact event owner. A retained
  // owner remains safe through subsequent Close, which seals its local stamp.
  // This is identity/lifetime only, not Android focus authority. Retired targets
  // cannot attach a new consumer; no process/current-window lookup is used.
  std::shared_ptr<DesktopRootEvents> RetainEvents() const noexcept;

  // AppKit-main-only.  Binding always addresses the original retained
  // window, never a current-window lookup or the active GPU surface.
  bool Bind(const DarwinArtDesktopRootObserver& observer) noexcept;
  bool UnbindExpected(void* context) noexcept;

  // AppKit-main-only.  Retires this exact catalog entry before closing the
  // existing event owner.  A retained handle remains safe to inspect after
  // retirement, but cannot bind again.
  bool Close() noexcept;
  bool Retire() noexcept;

  // Internal publication step used by InitializeDesktopRoot.
  bool Publish() noexcept;

  // Kept public so the ordinary shared_ptr deleter remains valid for an
  // opaque handle.  The destructor itself marshals AppKit-owned cleanup.
  ~DesktopRootTarget();

 private:
  // Native arithmetic-boundary tests only; no production hooks are exported.
  friend struct DesktopRootTargetTestPeer;
  friend std::shared_ptr<DesktopRootTarget>
  AcquireProcessDesktopRootTarget() noexcept;

  explicit DesktopRootTarget(std::shared_ptr<DesktopRootEvents> owner,
                             NSWindow* window) noexcept;
  static void RemovePublished(const std::shared_ptr<DesktopRootTarget>& target)
      noexcept;

  const uint64_t incarnation_;
  mutable std::mutex mutex_;
  std::shared_ptr<DesktopRootEvents> owner_;
  // This is an ARC +1 represented as an opaque pointer.  The implementation
  // releases it on AppKit's main actor even if the final C++ handle dies on a
  // worker thread.
  void* retained_window_ = nullptr;
  bool published_ = false;
  bool retired_ = false;
};

// Any-thread acquisition of the process's published root.  It is deliberately
// fail-closed when more than one root is live; no active-window or GPU lookup
// is performed.
std::shared_ptr<DesktopRootTarget> AcquireProcessDesktopRootTarget() noexcept;

}  // namespace darwin_art::window
#endif
