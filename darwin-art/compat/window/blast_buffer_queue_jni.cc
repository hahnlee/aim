#include "blast_buffer_queue_jni.h"

#include "blast_transaction_state.h"
#include "blast_callback_gate.h"
#include "../darwin_android_platform.h"
#include "../darwin_angle_egl.h"
#include "../graphics/blast_frame_policy.h"

#include <android/surface_control.h>

#include <algorithm>
#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <deque>
#include <iterator>
#include <list>
#include <memory>
#include <mutex>
#include <new>
#include <utility>
#include <vector>

namespace {

struct DarwinBlastBufferQueue {
  struct State {
    JavaVM* vm = nullptr;
    void* native_window = nullptr;
    jobject sync_consumer = nullptr;
    // A callback thread may fail to attach after claiming a global consumer
    // reference. Keep that reference until a valid JNI environment can
    // release it; deleting with a null JNIEnv would silently leak it.
    std::vector<jobject> deferred_consumers;
    // Transaction protocol state has no JNI lifetime or Java references.
    // Keeping it as a separate owner makes queue callbacks safe to retain
    // across the adapter's observer context and gives close one boundary.
    darwin_art::window::BlastTransactionState transactions;
    darwin_art::window::BlastCallbackGate callbacks;
  };

  std::shared_ptr<State> state;
  void* native_window = nullptr;
  ASurfaceControl* surface_control = nullptr;
  jint width = 0;
  jint height = 0;
  jint format = 1;
};

struct BlastTransactionObserverContext {
  std::shared_ptr<DarwinBlastBufferQueue::State> state;
};

struct BlastTransactionGateContext {
  std::shared_ptr<DarwinBlastBufferQueue::State> state;
  uint64_t generation = 0;
};

using BlastTransactionState = darwin_art::window::BlastTransactionState;
// Register ownership before exposing a JNI handle. Destruction never needs a
// fallible allocation to retain consumers claimed by an admitted callback.
std::mutex g_blast_states_mutex;
std::list<std::shared_ptr<DarwinBlastBufferQueue::State>> g_blast_states;
bool g_blast_states_closed = false;
size_t g_blast_states_cleaning = 0;
std::atomic<uint32_t> g_blast_debug_events{0};

void BlastDebugTrace(const char* event, const void* state, const void* window,
                     const void* transaction, uint64_t value) {
  if (std::getenv("DARWIN_ART_DEBUG_BLAST") == nullptr &&
      std::getenv("DEBUG_BLAST") == nullptr) {
    return;
  }
  const uint32_t sequence =
      g_blast_debug_events.fetch_add(1, std::memory_order_relaxed);
  if (sequence >= 128) return;
  std::fprintf(stderr,
               "ART BLAST[%u] %s state=%p window=%p tx=%p value=%llu\n",
               sequence, event == nullptr ? "?" : event, state, window,
               transaction, static_cast<unsigned long long>(value));
}

void ReleaseBlastTransactionObserverContext(void* opaque) {
  delete static_cast<BlastTransactionObserverContext*>(opaque);
}

JNIEnv* AttachBlastThread(JavaVM* vm, bool* attached) {
  if (attached != nullptr) *attached = false;
  if (vm == nullptr) return nullptr;
  JNIEnv* env = nullptr;
  if (vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) == JNI_OK) {
    return env;
  }
  if (vm->AttachCurrentThread(&env, nullptr) != JNI_OK) return nullptr;
  if (attached != nullptr) *attached = true;
  return env;
}

void DeleteBlastConsumer(JNIEnv* env, jobject consumer) {
  if (env != nullptr && consumer != nullptr) env->DeleteGlobalRef(consumer);
}

void DrainDeferredBlastConsumers(
    const std::shared_ptr<DarwinBlastBufferQueue::State>& state,
    JNIEnv* env) {
  if (state == nullptr || env == nullptr) return;
  std::vector<jobject> deferred;
  {
    std::lock_guard<std::mutex> lock(state->transactions.mutex);
    deferred.swap(state->deferred_consumers);
  }
  for (jobject consumer : deferred) DeleteBlastConsumer(env, consumer);
}

void DeferBlastConsumer(
    const std::shared_ptr<DarwinBlastBufferQueue::State>& state,
    jobject consumer) {
  if (state == nullptr || consumer == nullptr) return;
  std::lock_guard<std::mutex> lock(state->transactions.mutex);
  state->deferred_consumers.push_back(consumer);
}

void ApplyAndDeleteBlastTransaction(ASurfaceTransaction* transaction) {
  if (transaction == nullptr) return;
  ASurfaceTransaction_apply(transaction);
  ASurfaceTransaction_delete(transaction);
}

jobject NewBlastJavaTransaction(JNIEnv* env,
                                ASurfaceTransaction* transaction) {
  if (env == nullptr || transaction == nullptr || env->ExceptionCheck()) {
    return nullptr;
  }
  jclass transaction_class =
      env->FindClass("android/view/SurfaceControl$Transaction");
  jmethodID constructor =
      transaction_class == nullptr
          ? nullptr
          : env->GetMethodID(transaction_class, "<init>", "(J)V");
  jobject result =
      constructor == nullptr
          ? nullptr
          : env->NewObject(transaction_class, constructor,
                           reinterpret_cast<jlong>(transaction));
  if (transaction_class != nullptr) env->DeleteLocalRef(transaction_class);
  return result;
}

void DiscardHeldBlastTransactions(
    const std::shared_ptr<DarwinBlastBufferQueue::State>& state,
    bool apply) {
  if (state == nullptr) return;
  std::deque<ASurfaceTransaction*> held;
  {
    std::lock_guard<std::mutex> lock(state->transactions.mutex);
    held.swap(state->transactions.held_transactions);
  }
  for (ASurfaceTransaction* transaction : held) {
    if (apply) {
      ApplyAndDeleteBlastTransaction(transaction);
    } else if (transaction != nullptr) {
      // Deletion invokes the platform discard callbacks, returning any
      // producer-held buffer without presenting a stale transaction.
      ASurfaceTransaction_delete(transaction);
    }
  }
}

void ReleaseReservedBlastGate(
    const std::shared_ptr<DarwinBlastBufferQueue::State>& state,
    uint64_t generation, bool apply) {
  if (state == nullptr) return;
  std::deque<ASurfaceTransaction*> held;
  {
    std::lock_guard<std::mutex> lock(state->transactions.mutex);
    if (state->transactions.sync_generation != generation ||
        state->transactions.sync_phase == BlastTransactionState::SyncPhase::kIdle) {
      return;
    }
    state->transactions.outstanding_sync = false;
    state->transactions.sync_phase = BlastTransactionState::SyncPhase::kIdle;
    held.swap(state->transactions.held_transactions);
  }
  for (ASurfaceTransaction* transaction : held) {
    if (apply) {
      ApplyAndDeleteBlastTransaction(transaction);
    } else if (transaction != nullptr) {
      ASurfaceTransaction_delete(transaction);
    }
  }
}

void BlastTransactionGateFinished(void* opaque, bool apply) {
  auto* gate = static_cast<BlastTransactionGateContext*>(opaque);
  if (gate == nullptr) return;
  auto state = std::move(gate->state);
  const uint64_t generation = gate->generation;
  delete gate;
  if (state == nullptr) return;
  auto admission = state->callbacks.TryEnter();
  if (!admission) return;
  // Keep the generation gated while taking and applying each FIFO batch.
  // Queue callbacks may arrive during ApplyAndDelete and are admitted to the
  // same FIFO; only the locked empty check transitions back to idle.
  for (;;) {
    std::deque<ASurfaceTransaction*> batch;
    bool should_apply = apply;
    {
      std::lock_guard<std::mutex> lock(state->transactions.mutex);
      if (state->transactions.sync_generation != generation ||
          !state->transactions.outstanding_sync ||
          state->transactions.sync_phase == BlastTransactionState::SyncPhase::kIdle) {
        return;
      }
      if (state->transactions.destroyed) should_apply = false;
      if (state->transactions.held_transactions.empty()) {
        state->transactions.outstanding_sync = false;
        state->transactions.sync_phase = BlastTransactionState::SyncPhase::kIdle;
        return;
      }
      state->transactions.sync_phase = BlastTransactionState::SyncPhase::kDraining;
      batch.swap(state->transactions.held_transactions);
    }
    for (ASurfaceTransaction* transaction : batch) {
      if (should_apply) {
        ApplyAndDeleteBlastTransaction(transaction);
      } else if (transaction != nullptr) {
        ASurfaceTransaction_delete(transaction);
      }
    }
  }
}

void BlastTransactionGateOnCommit(void* opaque,
                                  ASurfaceTransactionStats*) {
  auto* gate = static_cast<BlastTransactionGateContext*>(opaque);
  BlastDebugTrace("gate-commit", gate == nullptr ? nullptr : gate->state.get(),
                  nullptr, nullptr,
                  gate == nullptr ? 0 : gate->generation);
  BlastTransactionGateFinished(opaque, true);
}

void BlastTransactionGateOnDiscard(void* opaque) {
  auto* gate = static_cast<BlastTransactionGateContext*>(opaque);
  BlastDebugTrace("gate-discard", gate == nullptr ? nullptr : gate->state.get(),
                  nullptr, nullptr,
                  gate == nullptr ? 0 : gate->generation);
  BlastTransactionGateFinished(opaque, false);
}

bool ArmBlastTransactionGate(
    const std::shared_ptr<DarwinBlastBufferQueue::State>& state,
    ASurfaceTransaction* transaction, uint64_t generation) {
  if (state == nullptr || transaction == nullptr) return false;
  auto* gate = new (std::nothrow)
      BlastTransactionGateContext{state, generation};
  if (gate == nullptr) return false;
  {
    std::lock_guard<std::mutex> lock(state->transactions.mutex);
    if (state->transactions.destroyed || !state->transactions.outstanding_sync ||
        state->transactions.sync_generation != generation ||
        state->transactions.sync_phase != BlastTransactionState::SyncPhase::kReserved) {
      delete gate;
      return false;
    }
    state->transactions.sync_phase = BlastTransactionState::SyncPhase::kGated;
  }
  // The same context is installed on mutually-exclusive paths: the platform
  // clears discard callbacks before invoking commit, so exactly one callback
  // owns and destroys the gate context.
  ASurfaceTransaction_setOnCommit(transaction, gate,
                                  &BlastTransactionGateOnCommit);
  darwin_art_android_surface_transaction_set_on_discard(
      transaction, gate, &BlastTransactionGateOnDiscard);
  BlastDebugTrace("gate-arm", state.get(), nullptr, transaction, generation);
  return true;
}

std::vector<ASurfaceTransaction*> TakeDueBlastTransactions(
    const std::shared_ptr<DarwinBlastBufferQueue::State>& state,
    uint64_t frame, bool all) {
  std::vector<ASurfaceTransaction*> result;
  if (state == nullptr) return result;
  std::lock_guard<std::mutex> lock(state->transactions.mutex);
  return state->transactions.TakeDueLocked(frame, all);
}

void MergeBlastTransactions(ASurfaceTransaction* destination,
                            const std::vector<ASurfaceTransaction*>& sources) {
  if (destination == nullptr) {
    for (ASurfaceTransaction* source : sources) {
      if (source != nullptr) ASurfaceTransaction_delete(source);
    }
    return;
  }
  for (ASurfaceTransaction* source : sources) {
    if (source == nullptr) continue;
    darwin_art_android_surface_transaction_merge(destination, source);
    ASurfaceTransaction_delete(source);
  }
}

bool DeliverBlastTransaction(
    const std::shared_ptr<DarwinBlastBufferQueue::State>& state,
    jobject consumer, ASurfaceTransaction* transaction,
    uint64_t generation) {
  if (transaction == nullptr) return false;
  if (state == nullptr) {
    // There is no callback owner left to consume this transaction. Returning
    // false leaves it to the native-window caller, which still owns it.
    return false;
  }
  if (consumer == nullptr) {
    ApplyAndDeleteBlastTransaction(transaction);
    return true;
  }
  bool attached = false;
  JNIEnv* env = AttachBlastThread(state->vm, &attached);
  if (env == nullptr) {
    // The sync reservation was made before JNI attach. Consume this callback's
    // transaction locally, then release any FIFO entries in order.
    DeferBlastConsumer(state, consumer);
    ApplyAndDeleteBlastTransaction(transaction);
    ReleaseReservedBlastGate(state, generation, true);
    return true;
  }
  BlastDebugTrace("consumer-invoke", state.get(), nullptr, transaction,
                  generation);
  jobject java_transaction = NewBlastJavaTransaction(env, transaction);
  jclass consumer_class =
      consumer == nullptr ? nullptr : env->GetObjectClass(consumer);
  jmethodID accept =
      consumer_class == nullptr
          ? nullptr
          : env->GetMethodID(consumer_class, "accept", "(Ljava/lang/Object;)V");
  const bool callable = java_transaction != nullptr && accept != nullptr &&
                        !env->ExceptionCheck();
  bool handed_to_java = false;
  if (callable) {
    // Arm before invoking Consumer.accept: accept may synchronously call
    // Transaction.apply(), and queue callbacks may arrive as soon as that
    // re-entrant call returns.
    if (ArmBlastTransactionGate(state, transaction, generation)) {
      handed_to_java = true;
      env->CallVoidMethod(consumer, accept, java_transaction);
    } else {
      // The Java wrapper owns the pointer, but no consumer saw it. Apply it
      // while the wrapper is still local and release the reservation; never
      // hand an ungated transaction to Java where later frames could pass it.
      ASurfaceTransaction_apply(transaction);
      ReleaseReservedBlastGate(state, generation, true);
    }
  }
  const bool exception = env->ExceptionCheck();
  BlastDebugTrace(exception ? "consumer-exception" : "consumer-return",
                  state.get(), nullptr, transaction, generation);
  if (exception) {
    // Once the Java Transaction has been passed to Consumer, Java owns the
    // native pointer even if Consumer closes it and then throws.  Never touch
    // the raw pointer on that path: it may already have been deleted.
    env->ExceptionClear();
    if (!handed_to_java && java_transaction == nullptr) {
      ApplyAndDeleteBlastTransaction(transaction);
    } else if (!handed_to_java) {
      // The wrapper exists but was not handed off (for example, accept could
      // not be resolved). It still owns the pointer, so apply only and let
      // its finalizer perform deletion.
      ASurfaceTransaction_apply(transaction);
    }
    if (!handed_to_java) ReleaseReservedBlastGate(state, generation, true);
  } else if (!callable) {
    if (java_transaction != nullptr) {
      ASurfaceTransaction_apply(transaction);
    } else {
      ApplyAndDeleteBlastTransaction(transaction);
    }
    ReleaseReservedBlastGate(state, generation, true);
  }
  if (consumer_class != nullptr) env->DeleteLocalRef(consumer_class);
  if (java_transaction != nullptr) env->DeleteLocalRef(java_transaction);
  DeleteBlastConsumer(env, consumer);
  if (attached) state->vm->DetachCurrentThread();
  // A Java Transaction constructed with the private native-pointer constructor
  // owns the transaction after construction. If it reached Consumer, its
  // finalizer (or Consumer.apply/close) is the only valid native owner; this
  // function deliberately never reuses the raw pointer on that path.
  return true;
}

bool BlastBufferQueueTransactionCallback(void* opaque, void* transaction,
                                         uint64_t frame_number) {
  auto* observer = static_cast<BlastTransactionObserverContext*>(opaque);
  if (observer == nullptr || observer->state == nullptr || transaction == nullptr)
    return false;
  auto state = observer->state;
  auto admission = state->callbacks.TryEnter();
  if (!admission) {
    // Detachment cannot revoke an observer snapshot already held by a producer.
    // Consume the rejected transaction, rather than allowing normal-apply
    // fallback to present a frame after BLAST has closed.
    ASurfaceTransaction_delete(static_cast<ASurfaceTransaction*>(transaction));
    return true;
  }
  BlastDebugTrace("queue-callback", state.get(), nullptr, transaction,
                  frame_number);
  jobject consumer = nullptr;
  ASurfaceTransaction* deliver = nullptr;
  uint64_t generation = 0;
  std::vector<ASurfaceTransaction*> due;
  auto* incoming = static_cast<ASurfaceTransaction*>(transaction);
  {
    std::unique_lock<std::mutex> lock(state->transactions.mutex);
    state->transactions.NoteAcquiredFrameLocked(frame_number);
    if (state->transactions.destroyed) {
      lock.unlock();
      ASurfaceTransaction_delete(incoming);
      return true;
    }
    due = state->transactions.TakeDueLocked(frame_number, false);
  }
  // Merge and release frame-indexed transactions outside the state mutex. A
  // platform discard hook may synchronously re-enter the queue adapter.
  for (ASurfaceTransaction* source : due) {
    if (source != nullptr) {
      darwin_art_android_surface_transaction_merge(incoming, source);
      ASurfaceTransaction_delete(source);
    }
  }
  // One operation-local batch preserves the displaced generation's acquire
  // fences. Its destructor runs after the transaction mutex is unlocked,
  // while this callback's State and admission lease are still alive.
  std::unique_ptr<ASurfaceTransaction, decltype(&ASurfaceTransaction_delete)>
      disposal(ASurfaceTransaction_create(), &ASurfaceTransaction_delete);
  {
    std::unique_lock<std::mutex> lock(state->transactions.mutex);
    if (state->transactions.destroyed) {
      lock.unlock();
      ASurfaceTransaction_delete(incoming);
      return true;
    }
    if (state->transactions.continuous_sync) {
      if (state->transactions.continuous_transaction == nullptr) {
        state->transactions.continuous_transaction = incoming;
        state->transactions.continuous_frame = frame_number;
      } else {
        if (disposal == nullptr ||
            !darwin_art_android_surface_transaction_merge_deferred(
                state->transactions.continuous_transaction, incoming,
                disposal.get())) {
          // Structural failure leaves both transactions intact. Do not apply
          // the incoming frame ahead of the sync gate or publish a partial
          // accumulator; return its producer ownership outside the mutex.
          lock.unlock();
          std::fprintf(stderr, "ART BLAST: continuous merge allocation failed\n");
          ASurfaceTransaction_delete(incoming);
          return true;
        }
        state->transactions.continuous_frame =
            std::max(state->transactions.continuous_frame, frame_number);
        lock.unlock();
        ASurfaceTransaction_delete(incoming);
        return true;
      }
      return true;
    }
    // A reserved single-sync consumer must claim the first queued transaction
    // even though outstanding_sync is already true. That reservation closes
    // the race between SyncNextTransaction and JNI delivery.
    if (state->sync_consumer != nullptr) {
      consumer = state->sync_consumer;
      state->sync_consumer = nullptr;
      state->transactions.continuous_sync = false;
      generation = state->transactions.sync_generation;
      deliver = incoming;
    } else if (state->transactions.outstanding_sync) {
      // A commit gate is active even though the Java consumer slot has been
      // consumed. Preserve queue order until that transaction commits.
      if (state->transactions.HoldTransactionLocked(incoming)) {
        return true;
      }
      // Do not let a bounded-queue overflow violate the commit gate. The
      // incoming transaction is still owned by this callback, so discard it
      // (and return its producer buffer through the platform lifecycle hook)
      // rather than applying it ahead of the outstanding Java transaction.
      lock.unlock();
      ASurfaceTransaction_delete(incoming);
      return true;
    }
  }
  // Consumer invocation may re-enter framework code and must never run under
  // the state mutex. The callback has claimed transaction ownership here.
  return DeliverBlastTransaction(state, consumer, deliver, generation);
}

void DiscardPendingBlastTransaction(
    const std::shared_ptr<DarwinBlastBufferQueue::State>& state,
    JNIEnv* env) {
  if (state == nullptr) return;
  jobject consumer = nullptr;
  std::vector<ASurfaceTransaction*> owned;
  {
    std::lock_guard<std::mutex> lock(state->transactions.mutex);
    consumer = state->sync_consumer;
    state->sync_consumer = nullptr;
    state->transactions.continuous_sync = false;
    // A consumer still present here has not been claimed by the queue
    // callback, so clear can safely cancel its reservation. If it is already
    // null, a delivery is in flight and its generation owns the reservation.
    if (consumer != nullptr &&
        state->transactions.sync_phase == BlastTransactionState::SyncPhase::kReserved) {
      state->transactions.outstanding_sync = false;
      state->transactions.sync_phase = BlastTransactionState::SyncPhase::kIdle;
      state->transactions.BeginSyncGenerationLocked();
    }
    owned = state->transactions.TakeAllOwnedLocked();
  }
  DrainDeferredBlastConsumers(state, env);
  DeleteBlastConsumer(env, consumer);
  for (ASurfaceTransaction* transaction : owned) {
    if (transaction != nullptr) ASurfaceTransaction_delete(transaction);
  }
}

void DispatchContinuousBlastTransaction(
    std::shared_ptr<DarwinBlastBufferQueue::State> state, JNIEnv* env) {
  if (state == nullptr) return;
  auto admission = state->callbacks.TryEnter();
  if (!admission) return;
  DrainDeferredBlastConsumers(state, env);
  jobject consumer = nullptr;
  ASurfaceTransaction* pending = nullptr;
  uint64_t generation = 0;
  std::vector<ASurfaceTransaction*> future;
  {
    std::lock_guard<std::mutex> lock(state->transactions.mutex);
    if (state->sync_consumer == nullptr || !state->transactions.continuous_sync) return;
    consumer = state->sync_consumer;
    generation = state->transactions.sync_generation;
    state->sync_consumer = nullptr;
    state->transactions.continuous_sync = false;
    pending = state->transactions.continuous_transaction;
    state->transactions.continuous_transaction = nullptr;
    state->transactions.continuous_frame = 0;
    for (const auto& entry : state->transactions.future_transactions) {
      if (entry.transaction != nullptr) future.push_back(entry.transaction);
    }
    state->transactions.future_transactions.clear();
  }
  if (pending == nullptr) pending = ASurfaceTransaction_create();
  // The native stop call is made by the Java/UI thread, but delivery is kept
  // outside the queue mutex for the same re-entry guarantee as frame delivery.
  if (pending == nullptr) {
    DeleteBlastConsumer(env, consumer);
    for (ASurfaceTransaction* transaction : future) {
      if (transaction != nullptr) ASurfaceTransaction_delete(transaction);
    }
    return;
  }
  MergeBlastTransactions(pending, future);
  if (!DeliverBlastTransaction(state, consumer, pending, generation)) {
    // Stop is not itself an ANativeWindow callback, so there is no caller to
    // perform the normal-apply fallback when JNI delivery cannot attach.
    ApplyAndDeleteBlastTransaction(pending);
  }
}

bool SweepClosedBlastStates(JNIEnv* env, bool close_all) {
  if (env == nullptr) return false;
  std::list<std::shared_ptr<DarwinBlastBufferQueue::State>> drained;
  bool pending = false;
  {
    std::lock_guard<std::mutex> registry_lock(g_blast_states_mutex);
    if (close_all) g_blast_states_closed = true;
    for (auto it = g_blast_states.begin(); it != g_blast_states.end();) {
      const auto& state = *it;
      if (close_all) state->callbacks.Close();
      bool closed;
      {
        std::lock_guard<std::mutex> lock(state->transactions.mutex);
        if (close_all) state->transactions.CloseLocked();
        closed = state->transactions.destroyed;
      }
      if (closed && state->callbacks.Drained()) {
        auto current = it++;
        drained.splice(drained.end(), g_blast_states, current);
        ++g_blast_states_cleaning;
      } else {
        if (closed) pending = true;
        ++it;
      }
    }
  }
  // Observer release, transaction discard and JNI calls may re-enter BLAST.
  // No registry, transaction or admission mutex is held during final cleanup.
  for (const auto& state : drained) {
    void* window = nullptr;
    {
      std::lock_guard<std::mutex> lock(state->transactions.mutex);
      window = state->native_window;
      if (window != nullptr) darwin_art_android_ANativeWindow_acquire(window);
      state->native_window = nullptr;
    }
    if (window != nullptr) {
      darwin_art_android_ANativeWindow_set_transaction_callback(
          window, nullptr, nullptr, nullptr);
      darwin_art_android_ANativeWindow_release(window);
    }
    DiscardPendingBlastTransaction(state, env);
    DiscardHeldBlastTransactions(state, false);
  }
  {
    std::lock_guard<std::mutex> lock(g_blast_states_mutex);
    g_blast_states_cleaning -= drained.size();
    // A concurrent/reentrant sweep must not report readiness merely because
    // another owner moved records out of the list to perform JNI cleanup.
    return !pending && g_blast_states_cleaning == 0;
  }
}

jlong BlastBufferQueueNativeCreate(JNIEnv* env, jclass, jstring, jboolean) {
  auto* queue = new (std::nothrow) DarwinBlastBufferQueue();
  if (queue == nullptr) return 0;
  try {
    queue->state = std::make_shared<DarwinBlastBufferQueue::State>();
  } catch (const std::bad_alloc&) {
    delete queue;
    return 0;
  }
  if (queue->state == nullptr || env == nullptr ||
      env->GetJavaVM(&queue->state->vm) != JNI_OK) {
    delete queue;
    return 0;
  }
  try {
    std::lock_guard<std::mutex> lock(g_blast_states_mutex);
    if (g_blast_states_closed) {
      delete queue;
      return 0;
    }
    g_blast_states.push_back(queue->state);
  } catch (const std::bad_alloc&) {
    delete queue;
    return 0;
  }
  return reinterpret_cast<jlong>(queue);
}
void* BlastBufferQueueAcquireNativeWindow(jlong handle) {
  auto* queue = reinterpret_cast<DarwinBlastBufferQueue*>(handle);
  if (queue == nullptr || queue->state == nullptr) return nullptr;
  std::lock_guard<std::mutex> lock(queue->state->transactions.mutex);
  if (queue->state->transactions.destroyed ||
      queue->state->native_window == nullptr) {
    return nullptr;
  }
  void* window = queue->state->native_window;
  darwin_art_android_ANativeWindow_acquire(window);
  return window;
}


void BlastBufferQueueNativeDestroy(JNIEnv* env, jclass, jlong handle) {
  auto* queue = reinterpret_cast<DarwinBlastBufferQueue*>(handle);
  if (queue == nullptr) return;
  auto state = queue->state;
  void* native_window = queue->native_window;
  if (state != nullptr) {
    state->callbacks.Close();
    {
      std::lock_guard<std::mutex> lock(state->transactions.mutex);
      state->transactions.CloseLocked();
      state->native_window = nullptr;
    }
    if (native_window != nullptr) {
      darwin_art_android_ANativeWindow_set_transaction_callback(
          native_window, nullptr, nullptr, nullptr);
    }
    // Consumer cleanup follows the final admitted delivery, including JNI
    // detach. Reentrant Java destruction must never block an active Consumer.
    // The creation-time registry retains pending state until an owner sweep.
  }
  if (queue->native_window != nullptr) {
    darwin_art_android_ANativeWindow_release(queue->native_window);
  }
  delete queue;
  SweepClosedBlastStates(env, false);
}
void BlastBufferQueueNativeUpdate(JNIEnv*, jclass, jlong handle,
                                  jlong surface_control, jlong width,
                                  jlong height, jint format) {
  auto* queue = reinterpret_cast<DarwinBlastBufferQueue*>(handle);
  if (queue == nullptr || width <= 0 || height <= 0 || width > INT32_MAX ||
      height > INT32_MAX) {
    return;
  }
  queue->width = static_cast<jint>(width);
  queue->height = static_cast<jint>(height);
  queue->format = format == 0 ? 1 : format;
  queue->surface_control = reinterpret_cast<ASurfaceControl*>(surface_control);
  // BLAST owns this queue's buffer dimensions, not the display dimensions.
  // A popup or child Surface therefore must not resize the process-wide HWC
  // target. DisplayManager/HWC configure that target independently.
  if (queue->native_window == nullptr) {
    queue->native_window = darwin_art_android_ANativeWindow_create(
        queue->width, queue->height, queue->format);
  } else {
    (void)darwin_art_android_ANativeWindow_setBuffersGeometry(
        queue->native_window, queue->width, queue->height, queue->format);
  }
  darwin_art_android_ANativeWindow_set_surface_control(
      queue->native_window, queue->surface_control);
  if (queue->state != nullptr) {
    std::lock_guard<std::mutex> lock(queue->state->transactions.mutex);
    if (queue->state->native_window == nullptr) {
      auto* context = new (std::nothrow) BlastTransactionObserverContext{
          queue->state};
      if (context != nullptr &&
          darwin_art_android_ANativeWindow_set_transaction_callback(
              queue->native_window, &BlastBufferQueueTransactionCallback,
              context, &ReleaseBlastTransactionObserverContext)) {
        queue->state->native_window = queue->native_window;
        BlastDebugTrace("observer-registered", queue->state.get(),
                        queue->native_window, nullptr, 1);
      } else {
        BlastDebugTrace("observer-register-failed", queue->state.get(),
                        queue->native_window, nullptr, 0);
        delete context;
      }
    }
  }
}
jlong BlastBufferQueueNativeGetLastAcquiredFrameNum(JNIEnv*, jclass,
                                                   jlong handle) {
  auto* queue = reinterpret_cast<DarwinBlastBufferQueue*>(handle);
  if (queue == nullptr || queue->state == nullptr) return 0;
  std::lock_guard<std::mutex> lock(queue->state->transactions.mutex);
  return static_cast<jlong>(queue->state->transactions.last_acquired_frame);
}
jboolean BlastBufferQueueNativeIsSameSurfaceControl(JNIEnv*, jclass,
                                                    jlong handle,
                                                    jlong surface_control) {
  const auto* queue = reinterpret_cast<DarwinBlastBufferQueue*>(handle);
  return queue != nullptr &&
                 queue->surface_control ==
                     reinterpret_cast<ASurfaceControl*>(surface_control)
             ? JNI_TRUE
             : JNI_FALSE;
}
jobject BlastBufferQueueNativeGetSurface(JNIEnv* env, jclass, jlong handle,
                                         jboolean) {
  auto* queue = reinterpret_cast<DarwinBlastBufferQueue*>(handle);
  jclass surface_class = env->FindClass("android/view/Surface");
  jmethodID constructor = surface_class == nullptr
                              ? nullptr
                              : env->GetMethodID(surface_class, "<init>", "()V");
  jobject surface = constructor == nullptr
                        ? nullptr
                        : env->NewObject(surface_class, constructor);
  jfieldID native_object = surface_class == nullptr
                               ? nullptr
                               : env->GetFieldID(surface_class, "mNativeObject", "J");
  if (surface != nullptr && native_object != nullptr && queue != nullptr &&
      queue->native_window != nullptr && !env->ExceptionCheck()) {
    darwin_art_android_ANativeWindow_acquire(queue->native_window);
    env->SetLongField(
        surface, native_object,
        reinterpret_cast<jlong>(queue->native_window));
  }
  env->DeleteLocalRef(surface_class);
  return surface;
}
jobject BlastBufferQueueNativeGatherPendingTransactions(JNIEnv* env, jclass,
                                                         jlong handle,
                                                         jlong frame) {
  auto* queue = reinterpret_cast<DarwinBlastBufferQueue*>(handle);
  std::shared_ptr<DarwinBlastBufferQueue::State> state =
      queue == nullptr ? nullptr : queue->state;
  const auto future = TakeDueBlastTransactions(
      state, frame <= 0 ? 0 : static_cast<uint64_t>(frame), frame <= 0);
  ASurfaceTransaction* pending = ASurfaceTransaction_create();
  MergeBlastTransactions(pending, future);
  if (pending == nullptr) pending = ASurfaceTransaction_create();
  jobject result = NewBlastJavaTransaction(env, pending);
  if (result == nullptr) ApplyAndDeleteBlastTransaction(pending);
  return result;
}
void BlastBufferQueueNativeSetApplyToken(JNIEnv*, jclass, jlong, jobject) {}
void BlastBufferQueueNativeSetHangCallback(JNIEnv*, jclass, jlong, jobject) {}

void BlastBufferQueueNativeMergeWithNextTransaction(JNIEnv*, jclass,
                                                    jlong handle,
                                                    jlong transaction,
                                                    jlong frame) {
  auto* queue = reinterpret_cast<DarwinBlastBufferQueue*>(handle);
  auto* source = reinterpret_cast<ASurfaceTransaction*>(transaction);
  if (queue == nullptr || queue->state == nullptr || source == nullptr) return;
  auto state = queue->state;
  auto admission = state->callbacks.TryEnter();
  if (!admission) return;
  const uint64_t target_frame = frame <= 0 ? 0 : static_cast<uint64_t>(frame);
  bool due = false;
  uint64_t last_acquired_frame = 0;
  {
    std::lock_guard<std::mutex> lock(state->transactions.mutex);
    last_acquired_frame = state->transactions.last_acquired_frame;
    if (state->transactions.destroyed) return;
    due = darwin_art::graphics::IsBlastTransactionDue(last_acquired_frame,
                                                    target_frame);
  }
  if (due) {
    // Preserve the caller-owned due transaction; apply does not consume its
    // Java wrapper's native ownership.
    ASurfaceTransaction_apply(source);
    return;
  }
  ASurfaceTransaction* owned = ASurfaceTransaction_create();
  if (owned == nullptr) return;
  // A fresh destination has no displaced callbacks, and provider work must
  // still remain outside the Android transaction-state lock.
  darwin_art_android_surface_transaction_merge(owned, source);
  bool closed = false;
  {
    std::lock_guard<std::mutex> lock(state->transactions.mutex);
    closed = state->transactions.destroyed;
    last_acquired_frame = state->transactions.last_acquired_frame;
    due = darwin_art::graphics::IsBlastTransactionDue(last_acquired_frame,
                                                    target_frame);
    if (!closed && !due) state->transactions.QueueFutureLocked(owned, target_frame);
  }
  BlastDebugTrace(closed ? "merge-closed" : due ? "merge-due" : "merge-queued",
                  state.get(), nullptr, source, target_frame);
  if (closed) ASurfaceTransaction_delete(owned);
  else if (due) ApplyAndDeleteBlastTransaction(owned);
}

void BlastBufferQueueNativeClearSyncTransaction(JNIEnv* env, jclass,
                                                jlong handle) {
  auto* queue = reinterpret_cast<DarwinBlastBufferQueue*>(handle);
  if (queue == nullptr) return;
  DiscardPendingBlastTransaction(queue->state, env);
}

void BlastBufferQueueNativeStopContinuousSyncTransaction(JNIEnv* env, jclass,
                                                         jlong handle) {
  auto* queue = reinterpret_cast<DarwinBlastBufferQueue*>(handle);
  if (queue == nullptr) return;
  DispatchContinuousBlastTransaction(queue->state, env);
}

void BlastBufferQueueNativeApplyPendingTransactions(JNIEnv*, jclass,
                                                    jlong handle, jlong frame) {
  auto* queue = reinterpret_cast<DarwinBlastBufferQueue*>(handle);
  if (queue == nullptr || queue->state == nullptr) return;
  const auto future = TakeDueBlastTransactions(
      queue->state, frame <= 0 ? 0 : static_cast<uint64_t>(frame), frame <= 0);
  ASurfaceTransaction* pending = ASurfaceTransaction_create();
  MergeBlastTransactions(pending, future);
  ApplyAndDeleteBlastTransaction(pending);
}

jboolean BlastBufferQueueNativeSyncNextTransaction(JNIEnv* env, jclass,
                                                   jlong handle,
                                                   jobject consumer,
                                                   jboolean acquire_single) {
  auto* queue = reinterpret_cast<DarwinBlastBufferQueue*>(handle);
  if (queue == nullptr || queue->state == nullptr || env == nullptr ||
      consumer == nullptr) {
    return JNI_FALSE;
  }
  DrainDeferredBlastConsumers(queue->state, env);
  jobject global = env->NewGlobalRef(consumer);
  if (global == nullptr) return JNI_FALSE;
  auto state = queue->state;
  bool accepted = false;
  uint64_t generation = 0;
  void* observed_window = nullptr;
  {
    std::lock_guard<std::mutex> lock(state->transactions.mutex);
    observed_window = state->native_window;
    if (!state->transactions.destroyed && state->sync_consumer == nullptr &&
        !state->transactions.outstanding_sync) {
      state->sync_consumer = global;
      state->transactions.continuous_sync = acquire_single == JNI_FALSE;
      // Reserve the generation before JNI attach/wrapper construction. A
      // queue callback racing this call will either claim this consumer or be
      // held behind this reservation, never applied as an ordinary frame.
      state->transactions.outstanding_sync = true;
      state->transactions.sync_phase = BlastTransactionState::SyncPhase::kReserved;
      generation = state->transactions.BeginSyncGenerationLocked();
      accepted = true;
    }
  }
  if (!accepted) env->DeleteGlobalRef(global);
  BlastDebugTrace(accepted ? "sync-reserved" : "sync-rejected", state.get(),
                  observed_window, nullptr,
                  accepted ? generation : 0);
  return accepted ? JNI_TRUE : JNI_FALSE;
}


bool RegisterBlastNatives(JNIEnv* env, const char* class_name,
                          JNINativeMethod* methods, jint method_count) {
  if (env == nullptr) return false;
  jclass klass = env->FindClass(class_name);
  if (klass == nullptr) return false;
  const bool registered =
      env->RegisterNatives(klass, methods, method_count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return registered;
}

}  // namespace

extern "C" void* darwin_art_android_blast_buffer_queue_acquire_native_window(
    jlong handle) {
  return BlastBufferQueueAcquireNativeWindow(handle);
}

namespace darwin_art::window {

bool QuiesceBlastBufferQueues(JNIEnv* env) {
  return SweepClosedBlastStates(env, true);
}

bool RegisterBlastBufferQueueNatives(JNIEnv* env) {
  {
    std::lock_guard<std::mutex> lock(g_blast_states_mutex);
    // A retained provider can serve a later VM only after the previous owner
    // drained every registered queue. Never reuse states containing old JNI.
    if (!g_blast_states.empty() || g_blast_states_cleaning != 0) return false;
    g_blast_states_closed = false;
  }
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeApplyPendingTransactions"),
       const_cast<char*>("(JJ)V"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeApplyPendingTransactions)},
      {const_cast<char*>("nativeClearSyncTransaction"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeClearSyncTransaction)},
      {const_cast<char*>("nativeCreate"),
       const_cast<char*>("(Ljava/lang/String;Z)J"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeCreate)},
      {const_cast<char*>("nativeDestroy"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeDestroy)},
      {const_cast<char*>("nativeGatherPendingTransactions"),
       const_cast<char*>("(JJ)Landroid/view/SurfaceControl$Transaction;"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeGatherPendingTransactions)},
      {const_cast<char*>("nativeGetLastAcquiredFrameNum"),
       const_cast<char*>("(J)J"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeGetLastAcquiredFrameNum)},
      {const_cast<char*>("nativeGetSurface"),
       const_cast<char*>("(JZ)Landroid/view/Surface;"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeGetSurface)},
      {const_cast<char*>("nativeIsSameSurfaceControl"),
       const_cast<char*>("(JJ)Z"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeIsSameSurfaceControl)},
      {const_cast<char*>("nativeMergeWithNextTransaction"),
       const_cast<char*>("(JJJ)V"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeMergeWithNextTransaction)},
      {const_cast<char*>("nativeSetApplyToken"),
       const_cast<char*>("(JLandroid/os/IBinder;)V"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeSetApplyToken)},
      {const_cast<char*>("nativeSetTransactionHangCallback"),
       const_cast<char*>(
           "(JLandroid/graphics/BLASTBufferQueue$TransactionHangCallback;)V"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeSetHangCallback)},
      {const_cast<char*>("nativeSetWaitForBufferReleaseCallback"),
       const_cast<char*>(
           "(JLandroid/graphics/BLASTBufferQueue$WaitForBufferReleaseCallback;)V"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeSetHangCallback)},
      {const_cast<char*>("nativeStopContinuousSyncTransaction"),
       const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeStopContinuousSyncTransaction)},
      {const_cast<char*>("nativeSyncNextTransaction"),
       const_cast<char*>("(JLjava/util/function/Consumer;Z)Z"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeSyncNextTransaction)},
      {const_cast<char*>("nativeUpdate"), const_cast<char*>("(JJJJI)V"),
       reinterpret_cast<void*>(&BlastBufferQueueNativeUpdate)},
  };
  return RegisterBlastNatives(env, "android/graphics/BLASTBufferQueue",
                              methods,
                              static_cast<jint>(std::size(methods)));
}

}  // namespace darwin_art::window
