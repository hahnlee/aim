#pragma once

#ifdef __cplusplus
#include <cstdint>
#include <memory>
#include <mutex>
#else
#include <stdbool.h>
#include <stdint.h>
#endif

#ifdef __OBJC__
@class NSWindow;
#else
typedef struct NSWindow NSWindow;
#endif

#ifdef __cplusplus
extern "C" {
#endif

#ifdef __cplusplus
enum DarwinArtDesktopRootEventKind : uint32_t {
#else
typedef enum DarwinArtDesktopRootEventKind {
#endif
  DARWIN_ART_DESKTOP_ROOT_ACTIVATED = 1,
  DARWIN_ART_DESKTOP_ROOT_RESIGNED = 2,
  DARWIN_ART_DESKTOP_ROOT_CLOSED = 3,
}
#ifndef __cplusplus
DarwinArtDesktopRootEventKind
#endif
;

struct DarwinArtDesktopRootEvent {
  DarwinArtDesktopRootEventKind kind;
  uint64_t incarnation;
  uint64_t serial;
  bool key_window_snapshot;
};

#ifdef __cplusplus
typedef void (*DarwinArtDesktopRootEventCallback)(
    void* context, DarwinArtDesktopRootEvent event) noexcept;
#else
typedef void (*DarwinArtDesktopRootEventCallback)(
    void* context, struct DarwinArtDesktopRootEvent event);
#endif

struct DarwinArtDesktopRootEventContext {
  void* value;
#ifdef __cplusplus
  void (*retain)(void* value) noexcept;
  void (*release)(void* value) noexcept;
#else
  void (*retain)(void* value);
  void (*release)(void* value);
#endif
};

#ifdef __cplusplus
}  // extern "C"
#endif

#ifdef __cplusplus
namespace darwin_art::window {

class DesktopRootEvents final
    : public std::enable_shared_from_this<DesktopRootEvents> {
  struct Binding;
  struct WindowIdentity;
 public:
  struct LocalStamp final {
    uint64_t incarnation = 0;
    uint64_t state_revision = 0;
    uint64_t latest_emitted_serial = 0;
    bool key = false;
    bool closed = false;
    // Orders every effective publication/reset, separately from activation
    // state_revision. Saturation seals the root instead of wrapping.
    uint64_t stamp_revision = 0;
  };

  // Host identity/lifecycle facts only; this observer does not grant Android
  // focus or input authority. The root keeps one weak subscription.
  class StampObserver {
   public:
    virtual ~StampObserver() = default;
    virtual void OnLocalStamp(LocalStamp stamp) noexcept = 0;
  };

  // Allocation-free terminal notification. Preparing seals the root before
  // other host callbacks; destruction settles an unconsumed notification.
  class DeferredClose final {
   public:
    DeferredClose() noexcept = default;
    DeferredClose(DeferredClose&& other) noexcept;
    DeferredClose(const DeferredClose&) = delete;
    DeferredClose& operator=(const DeferredClose&) = delete;
    ~DeferredClose();
    bool Deliver() noexcept;
   private:
    friend class DesktopRootEvents;
    DeferredClose(std::shared_ptr<DesktopRootEvents> owner,
        std::shared_ptr<Binding> binding, DarwinArtDesktopRootEvent event) noexcept;
    std::shared_ptr<DesktopRootEvents> owner_;
    std::shared_ptr<Binding> binding_;
    DarwinArtDesktopRootEvent event_{};
  };
  // A move-only non-terminal observation. PrepareNotify commits the validated
  // host stamp while holding the root mutex; Deliver later revalidates that
  // exact stamp and binding before invoking the observer without the mutex.
  // Like DeferredClose, destruction must occur on the AppKit main thread and
  // settles an unconsumed capability by attempting delivery.
  class DeferredFact final {
   public:
    DeferredFact() noexcept = default;
    DeferredFact(DeferredFact&& other) noexcept;
    DeferredFact(const DeferredFact&) = delete;
    DeferredFact& operator=(const DeferredFact&) = delete;
    ~DeferredFact();
    bool Deliver() noexcept;
   private:
    friend class DesktopRootEvents;
    DeferredFact(std::shared_ptr<DesktopRootEvents> owner,
        std::shared_ptr<Binding> binding, DarwinArtDesktopRootEvent event,
        LocalStamp stamp) noexcept;
    std::shared_ptr<DesktopRootEvents> owner_;
    std::shared_ptr<Binding> binding_;
    DarwinArtDesktopRootEvent event_{};
    LocalStamp stamp_{};
  };
  // The original window is retained only as a weak identity. Returns null on
  // allocation or per-process incarnation exhaustion.
  static std::shared_ptr<DesktopRootEvents> Create(NSWindow* window) noexcept;

  DesktopRootEvents(const DesktopRootEvents&) = delete;
  DesktopRootEvents& operator=(const DesktopRootEvents&) = delete;

  // All lifecycle methods require the AppKit main thread. Serials order
  // emitted validated facts; superseded contradictory notifications coalesce.
  // DeferredClose must also be delivered/destroyed on the AppKit main thread.
  // Bind emits one
  // initial Activated/Resigned snapshot before returning to the caller.
  bool Bind(NSWindow* window, DarwinArtDesktopRootEventCallback callback,
            DarwinArtDesktopRootEventContext context) noexcept;
  bool Unbind() noexcept;
  // A delayed resource cleanup cannot remove a successor observer.
  bool UnbindExpected(void* context) noexcept;
  DeferredFact PrepareNotify(DarwinArtDesktopRootEventKind kind) noexcept;
  bool Notify(DarwinArtDesktopRootEventKind kind) noexcept;
  bool Close() noexcept;
  DeferredClose PrepareClose() noexcept;

  uint64_t incarnation() const noexcept;
  bool closed() const noexcept;
  // Any-thread immutable view of the latest validated host stamp.
  LocalStamp Snapshot() const noexcept;

  // Any-thread weak observer subscription. A repeated subscription of the
  // same live observer is an idempotent baseline/retry; a distinct live
  // observer is rejected. Initial/current stamps are delivered outside mutex_,
  // before existing fact/context tails. Callbacks may race or reenter: consumers
  // must ignore older/equal stamp_revision, except closed is always sticky.
  // Unsubscribe is exact and non-quiescent; already captured callbacks retain
  // the observer and may finish after it. Never wait for callbacks in a callback.
  bool SubscribeStampObserver(
      const std::shared_ptr<StampObserver>& observer) noexcept;
  bool UnsubscribeStampObserverExpected(
      const std::shared_ptr<StampObserver>& observer) noexcept;

 private:
  // Native arithmetic boundary tests only; no fixture hooks are exported.
  friend struct DesktopRootEventsTestPeer;
  explicit DesktopRootEvents(uint64_t incarnation) noexcept;
  DesktopRootEvents(uint64_t incarnation,
                    std::shared_ptr<WindowIdentity> window_identity,
                    bool key_window) noexcept;
  bool MainThread() const noexcept;
  bool ReserveSerialLocked(uint64_t* serial) noexcept;
  bool ReserveBindingAttemptLocked(uint64_t* attempt) noexcept;
  bool AdvanceRevisionLocked() noexcept;
  bool AdvanceStampRevisionLocked() noexcept;
  LocalStamp CurrentStampLocked() const noexcept;
  void FinishStampMutationLocked(const LocalStamp& before,
      std::shared_ptr<StampObserver>* observer, LocalStamp* stamp) noexcept;
  bool UpdateKeyLocked(bool key_window) noexcept;
  void FailClosedLocked() noexcept;
  void Dispatch(const std::shared_ptr<Binding>& binding,
                DarwinArtDesktopRootEvent event) noexcept;

  const uint64_t incarnation_;
  std::shared_ptr<WindowIdentity> original_window_;
  mutable std::mutex mutex_;
  std::shared_ptr<Binding> binding_;
  uint64_t next_serial_ = 1;
  uint64_t next_binding_attempt_ = 1;
  uint64_t latest_binding_attempt_ = 0;
  uint64_t state_revision_ = 0;
  uint64_t stamp_revision_ = 1;
  uint64_t latest_emitted_serial_ = 0;
  bool key_window_ = false;
  bool closed_ = false;
  std::weak_ptr<StampObserver> stamp_observer_;
};

}  // namespace darwin_art::window
#endif
