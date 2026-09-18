#pragma once

#include <android/hardware_buffer.h>
#include <android/surface_control.h>

#include <cstdint>
#include <memory>

struct ASurfaceTransactionStats;

namespace darwin_art::window {

// The queue owns slot storage; this owner retains only the immutable identity
// needed to return a submitted frame after SurfaceControl completion.
struct NativeWindowTransactionFrame {
  ASurfaceControl* control = nullptr;
  AHardwareBuffer* buffer = nullptr;
  int32_t slot = -1;
  uint64_t generation = 0;
  uint64_t frame = 0;
  // The producer sets this while holding its mutex so a concurrent surface
  // control replacement cannot invalidate the captured target identity.
  bool control_retained = false;
};

using NativeWindowTransactionComplete =
    void (*)(void*, ASurfaceTransactionStats*);
using NativeWindowTransactionDiscard = void (*)(void*, int);
using NativeWindowTransactionObserver =
    bool (*)(void*, void*, uint64_t);

struct NativeWindowTransactionConsumerHooks {
  void* context = nullptr;
  ASurfaceTransaction* (*create_transaction)(void*) = nullptr;
  void (*delete_transaction)(void*, ASurfaceTransaction*) = nullptr;
  void (*apply_transaction)(void*, ASurfaceTransaction*) = nullptr;
  bool (*set_buffer_checked)(void*, ASurfaceTransaction*, ASurfaceControl*,
                             AHardwareBuffer*, int, uint64_t) = nullptr;
  bool (*set_callbacks_checked)(void*, ASurfaceTransaction*, ASurfaceControl*,
                               void*, NativeWindowTransactionComplete,
                               NativeWindowTransactionDiscard) = nullptr;
  int (*duplicate_fence)(void*, int) = nullptr;
  void (*close_fence)(void*, int) = nullptr;
  int (*previous_release_fence)(void*, ASurfaceTransactionStats*,
                                ASurfaceControl*) = nullptr;
  bool (*previous_buffer_metadata)(void*, ASurfaceTransactionStats*,
                                   ASurfaceControl*, AHardwareBuffer**,
                                   uint64_t*) = nullptr;
  // Returns the stable underlying layer identity. Different wrapper pointers
  // for one layer must return the same pair; false falls back to the retained
  // wrapper address, which cannot be reused while its control lease is live.
  bool (*get_layer_identity)(void*, ASurfaceControl*, uint32_t* owner_process,
                             uint32_t* layer) = nullptr;
  void (*acquire_control)(void*, ASurfaceControl*) = nullptr;
  void (*release_control)(void*, ASurfaceControl*) = nullptr;
  void (*retain_owner)(void*) = nullptr;
  void (*release_owner)(void*) = nullptr;
  void (*return_frame)(void*, const NativeWindowTransactionFrame&, int,
                       bool quarantine) = nullptr;
};

// Android-owned completion/admission state for the ANativeWindow fallback.
// Mechanical slot storage stays with the ANativeWindow producer. Every
// Callback contexts retain generation/frame identity. Completion retires only
// the exact submitted predecessor reported by SurfaceControl's latch cookie
// for the same canonical layer; producer serial order is not latch order.
class NativeWindowTransactionConsumer final {
 public:
  struct State;

  explicit NativeWindowTransactionConsumer(
      NativeWindowTransactionConsumerHooks hooks);
  NativeWindowTransactionConsumer(const NativeWindowTransactionConsumer&) =
      delete;
  NativeWindowTransactionConsumer& operator=(
      const NativeWindowTransactionConsumer&) = delete;
  ~NativeWindowTransactionConsumer();

  // Takes the producer fence on every return path. On false, the caller owns
  // no transaction and the frame was returned or quarantined through hooks.
  // On true, ownership is either retained by completion callbacks or handed
  // to observer after it accepts the transaction.
  bool Submit(const NativeWindowTransactionFrame& frame, int acquire_fence,
              NativeWindowTransactionObserver observer = nullptr,
              void* observer_context = nullptr,
              std::shared_ptr<void> observer_lifetime = {});

 private:
  std::shared_ptr<State> state_;
};

}  // namespace darwin_art::window
