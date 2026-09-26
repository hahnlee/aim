#include "receiver_jni.h"

#include "channel_endpoint.h"
#include "input_channel_jni.h"
#include "input_channel_jni_resources.h"
#include "diagnostics/view_resources.h"
#include "input_transport.h"
#include "input_transport_pump.h"
#include "input_routing.h"
#include "packet_dispatch.h"
#include "receiver_admission.h"
#include "receiver_jni_resources.h"
#include "receiver_lifecycle.h"
#include "receiver_registry.h"
#include "receiver_routing_access.h"
#include "receiver_transport_policy.h"
#include "receiver_input_consumer.h"
#include "receiver_packet_consumption.h"
#include "view_root_input_jni.h"
#include "../looper/message_queue_jni.h"
#include "../wm/desktop_root_client_jni.h"
#include "../wm/desktop_root_geometry_jni.h"

#include <atomic>
#include <bit>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <iostream>
#include <iterator>
#include <memory>
#include <mutex>
#include <new>
#include <utility>
#include <unistd.h>

namespace darwin_art::input {

namespace {

struct ReceiverBindings final {
  JavaVM* vm;
  ReceiverChannelOps ops;
};
std::shared_ptr<const ReceiverBindings> g_receiver_bindings;
std::mutex g_registration_mutex;
bool g_registration_inflight = false;

using DarwinInputReceiver = InputReceiver;

}  // namespace

bool RefreshInputReceiverWritable(const InputRoutingHandle& routing) {
  const auto endpoint = GetInputRoutingEndpoint(routing);
  if (endpoint == nullptr) return false;
  const auto receiver = AcquireInputReceiver(endpoint->endpoint_id);
  if (receiver == nullptr || receiver->channel == nullptr ||
      receiver->channel->Routing() != routing || endpoint->transport == nullptr ||
      receiver->channel->Endpoint() == nullptr ||
      receiver->channel->Endpoint()->Transport() != endpoint->transport ||
      receiver->disposed.load(std::memory_order_acquire))
    return false;
  const auto transport = endpoint->transport;
  const int fd = transport->RemoteEndpointFd();
  if (fd < 0) return true;
  const auto slot = fd == transport->ReadFd()
                        ? ReceiverEndpointBindingSlot::kLocalWake
                        : ReceiverEndpointBindingSlot::kRemote;
  return receiver->binding.RefreshWritable(slot, fd, transport->HasPendingTx());
}

namespace {

bool RefreshWritable(const InputRoutingHandle& routing) {
  const auto bindings = std::atomic_load_explicit(
      &g_receiver_bindings, std::memory_order_acquire);
  return bindings != nullptr && bindings->ops.refresh_writable != nullptr &&
         bindings->ops.refresh_writable(routing);
}

bool EnsureScheduler(const std::shared_ptr<InputChannelResources>& channel,
                     void* looper) {
  const auto bindings = std::atomic_load_explicit(
      &g_receiver_bindings, std::memory_order_acquire);
  return bindings != nullptr && bindings->ops.ensure_scheduler != nullptr &&
         bindings->ops.ensure_scheduler(channel, looper);
}

void WakePending() {
  const auto bindings = std::atomic_load_explicit(
      &g_receiver_bindings, std::memory_order_acquire);
  if (bindings != nullptr && bindings->ops.wake_pending != nullptr)
    bindings->ops.wake_pending();
}

void ClearInputChannelPending(InputChannelResources* channel) {
  if (channel == nullptr || InputRoutingHasPending(channel->Routing())) return;
  channel->Endpoint()->DrainLocalWake();
  ClearInputRoutingPending(channel->Routing());
}

bool RearmInputChannelTransport(InputChannelResources* channel) {
  if (channel == nullptr) return false;
  return InputRoutingHasPending(channel->Routing()) &&
         channel->Endpoint()->WakeLocal();
}

ReceiverEndpointBindingEventResult InputChannelTransportCallback(
    ReceiverEndpointBindingSlot slot, int fd, int events, void* data) {
  const ReceiverId receiver_id = static_cast<ReceiverId>(
      reinterpret_cast<std::uintptr_t>(data));
  auto receiver = AcquireInputReceiver(receiver_id);
  if (receiver == nullptr || fd < 0) return ReceiverEndpointBindingEventResult::kTerminal;
  JNIEnv* env = nullptr;
  const auto bindings = std::atomic_load_explicit(
      &g_receiver_bindings, std::memory_order_acquire);
  if (bindings != nullptr && bindings->vm != nullptr) {
    (void)bindings->vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6);
  }
  // A receiver is an owner-Looper callback. Keep its FD registered until the
  // owner thread is attached, without admitting JNI resources prematurely.
  if (env == nullptr) return ReceiverEndpointBindingEventResult::kKeep;
  ReceiverCallbackAdmission admitted(env, receiver);
  if (!admitted.Admitted()) return ReceiverEndpointBindingEventResult::kTerminal;
  auto channel = receiver->channel;
  if (channel == nullptr || receiver->disposed.load(std::memory_order_acquire))
    return ReceiverEndpointBindingEventResult::kTerminal;
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android InputChannel callback-enter fd=" << fd
              << " events=0x" << std::hex << events << std::dec
              << " read_fd=" << channel->Endpoint()->Transport()->ReadFd()
              << " remote_fd="
              << channel->Endpoint()->Transport()->RemoteEndpointFd() << "\n";
  }
  if (!CanDrainReceiverTransport(channel, fd, events)) return ReceiverEndpointBindingEventResult::kTerminal;
  ReceiverTransportPolicy policy{channel, receiver->routing_recipient.lock(),
                                WakePending, env, receiver};
  if (policy.recipient == nullptr) return ReceiverEndpointBindingEventResult::kTerminal;
  ReceiverPacketConsumptionContext packet_context{env, receiver};
  ReceiverInputConsumer consumer(channel->Routing(), policy.recipient,
                                ConsumeOriginalReceiverPacket, &packet_context);
  policy.consumer = &consumer;
  const auto callbacks = ReceiverTransportCallbacks(&policy);
  const auto transport = channel->Endpoint()->Transport();
  if (transport == nullptr) return ReceiverEndpointBindingEventResult::kTerminal;
  // Raw OnFd already flushed retained TX before this live-reader callback.
  // Retry rejected complete ACK frames independently of Java invocation.
  (void)receiver->finishes.RetryPending();
  // A local wake is a continuation of this same ordered consumer, not a
  // second packet queue/parser. It must resume retained remote frames even
  // after the kernel-readable prefix was drained on a budget-limited turn.
  if (fd != transport->RemoteEndpointFd())
    (void)PumpInputTransport(transport.get(), fd, {});
  const auto local = consumer.DrainLocal();
  auto pump_status = InputTransportStatus::kBackpressured;
  bool remote_pumped = false;
  if (local == InputTransportConsumptionResult::kConsumed) {
    const int framed_fd = transport->RemoteEndpointFd();
    remote_pumped = framed_fd >= 0;
    pump_status = framed_fd >= 0
        ? PumpInputTransport(transport.get(), framed_fd, callbacks)
        : InputTransportStatus::kAccepted;
  }
  // Seal only after the authoritative framed pump consumed its final prefix
  // and unwound Java invocation/observation. Kernel EOF alone is insufficient.
  // A local wake can finish that prefix too, without retiring its own reader.
  if (remote_pumped && pump_status == InputTransportStatus::kTerminal &&
      transport->IsRxTerminal())
    receiver->finishes.SealRemoteAdmission();
  const auto keep = pump_status != InputTransportStatus::kTerminal
      ? ReceiverEndpointBindingEventResult::kKeep
      : slot == ReceiverEndpointBindingSlot::kLocalWake && transport->IsRxTerminal()
          ? ReceiverEndpointBindingEventResult::kKeep
      : slot == ReceiverEndpointBindingSlot::kRemote &&
                fd == transport->RemoteEndpointFd() && transport->IsRxTerminal()
          ? ReceiverEndpointBindingEventResult::kReaderComplete
          : ReceiverEndpointBindingEventResult::kTerminal;
  if (consumer.CompletionFailed()) {
    std::fputs("ART InputReceiver: lost exact packet completion; retiring consumer\n", stderr);
    return ReceiverEndpointBindingEventResult::kTerminal;
  }
  if (receiver->disposed.load(std::memory_order_acquire) ||
      !IsInputRoutingRecipientCurrent(channel->Routing(), policy.recipient))
    return ReceiverEndpointBindingEventResult::kTerminal;
  const size_t consumed = consumer.ConsumedPackets();
  if (consumed > 0) ClearInputChannelPending(channel.get());
  const auto ack_progress = receiver->finishes.RetryPending();
  (void)RefreshWritable(channel->Routing());
  if (ack_progress.recovery_needed)
    std::fputs("ART InputReceiver: ACK admission needs non-writable recovery\n", stderr);
  // Recheck after the final local drain/clear: an earlier wake could have
  // been consumed by this same callback. Do not spin on empty-TX rejection.
  const bool ack_budget_retry = ack_progress.retry_needed &&
      !ack_progress.backpressured && !ack_progress.coalesced;
  const bool rearmed = consumer.BudgetRetryNeeded() || ack_budget_retry
      ? channel->Endpoint()->WakeLocal()
      : !env->ExceptionCheck() && RearmInputChannelTransport(channel.get());
  WakePending();
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android InputChannel callback consumed=" << consumed
              << " pending="
              << (InputRoutingHasPending(channel->Routing()) ? 1 : 0)
              << " rearmed=" << (rearmed ? 1 : 0) << "\n";
  }
  return receiver->disposed.load(std::memory_order_acquire)
             ? ReceiverEndpointBindingEventResult::kTerminal : keep;
}

struct ReceiverTransportToken {
  const ReceiverId id;
};

void ReceiverTransportFailure(void*) noexcept {
  JNIEnv* env = nullptr;
  const auto bindings = std::atomic_load_explicit(
      &g_receiver_bindings, std::memory_order_acquire);
  if (bindings == nullptr || bindings->vm == nullptr ||
      bindings->vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) !=
          JNI_OK ||
      env == nullptr) {
    std::fputs("ART InputReceiver: native callback failed without JNI attachment\n",
               stderr);
    return;
  }
  if (env->ExceptionCheck()) return;
  jclass type = env->FindClass("java/lang/RuntimeException");
  if (type != nullptr) {
    (void)env->ThrowNew(type, "Native input receiver callback failed");
    env->DeleteLocalRef(type);
  }
}

ReceiverEndpointBindingEventResult ReceiverTransportEvent(
    void* context, ReceiverEndpointBindingSlot slot, int fd, int events) {
  const auto* token = static_cast<const ReceiverTransportToken*>(context);
  return InputChannelTransportCallback(
      slot, fd, events,
      reinterpret_cast<void*>(static_cast<std::uintptr_t>(token->id)));
}

void ReceiverTransportStatus(void* context, ReceiverEndpointBindingStatus status) {
  const auto* token = static_cast<const ReceiverTransportToken*>(context);
  const auto receiver = AcquireInputReceiver(token->id);
  if (receiver == nullptr || receiver->channel == nullptr ||
      receiver->disposed.load(std::memory_order_acquire))
    return;
  SetInputRoutingTransportReady(receiver->channel->Routing(), token->id,
                                !status.closed &&
                                    (status.local_ready || status.remote_ready));
  WakePending();
}

jboolean InputReceiverConsume(JNIEnv* env, jclass, jlong pointer, jlong) {
  auto receiver = AcquireInputReceiver(static_cast<ReceiverId>(pointer));
  if (receiver == nullptr || !receiver->admission.Admit()) return JNI_FALSE;
  ReleaseReceiverAdmission(env, receiver.get());
  return JNI_FALSE;
}

void InputReceiverDispose(JNIEnv* env, jclass, jlong pointer) {
  const ReceiverId receiver_id = static_cast<ReceiverId>(pointer);
  auto receiver_lease = RetireInputReceiver(receiver_id);
  if (receiver_lease == nullptr) return;
  (void)CloseReceiverRetirement(*receiver_lease);
  RetireReceiverResources(env, receiver_lease);
  WakePending();
}

jstring InputReceiverDump(JNIEnv* env, jclass, jlong, jstring) {
  return env->NewStringUTF("");
}

void InputReceiverFinish(JNIEnv* env, jclass, jlong pointer, jint sequence,
                         jboolean handled) {
  auto receiver_lease = AcquireInputReceiver(static_cast<ReceiverId>(pointer));
  auto* receiver = receiver_lease.get();
  {
    ReceiverCallbackAdmission admission(env, receiver_lease);
    if (admission.Admitted() && receiver->channel != nullptr) {
      try {
        const auto progress = receiver->finishes.Finish(
            static_cast<uint32_t>(sequence), handled == JNI_TRUE);
        (void)RefreshWritable(receiver->channel->Routing());
        if (progress.accepted != 0) WakePending();
        if (progress.retry_needed && !progress.backpressured && !progress.coalesced)
          (void)receiver->channel->Endpoint()->WakeLocal();
        if (progress.recovery_needed)
          std::fputs("ART InputReceiver: ACK admission needs non-writable recovery\n", stderr);
      } catch (const std::bad_alloc&) {
        ThrowInputChannelOutOfMemory(env);
      } catch (...) {
        ReceiverTransportFailure(nullptr);  // Surface failure to the Java owner.
      }
    }
  }
  // Outside the callback admission: the fallback lookup runs Java code.
  if (receiver != nullptr && receiver->channel != nullptr) {
    const auto follow_ups = receiver->key_fallbacks.Finished(
        env, static_cast<uint32_t>(sequence), handled == JNI_TRUE);
    const auto recipient = receiver->routing_recipient.lock();
    bool queued = false;
    for (const auto& key : follow_ups) {
      // Queued behind the original, never dispatched reentrantly.
      darwin_art::DarwinArtInputPacket packet;
      packet.kind = darwin_art::DarwinArtInputPacketKind::kKey;
      packet.key = key;
      queued = EnqueueInputRoutingPacket(receiver->channel->Routing(), packet,
                                         recipient == nullptr ? 0 : recipient->id) ||
               queued;
      if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
        std::cerr << "ART Android InputEvent unhandled-key follow-up pid=" << getpid()
                  << " key=" << key.key_code << " action=" << key.action
                  << " flags=0x" << std::hex << key.flags << std::dec << "\n";
      }
    }
    if (queued) {
      WakePending();
      (void)receiver->channel->Endpoint()->WakeLocal();
    }
  }
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android InputEvent finish pid=" << getpid()
              << " sequence=" << sequence
              << " handled=" << (handled == JNI_TRUE ? 1 : 0) << "\n";
  }
}

struct ReceiverInitializationFailure {};

jlong InputReceiverInit(JNIEnv* env, jclass, jobject weak_receiver,
                        jobject input_channel, jobject message_queue) {
  std::shared_ptr<DarwinInputReceiver> receiver_shared;
  try {
    auto channel_resources = AcquireInputChannelResources(env, input_channel);
    if (channel_resources == nullptr || weak_receiver == nullptr ||
        env->ExceptionCheck())
      return 0;
    void* owner_looper = framework_system::message_queue_looper(env, message_queue);
    if (owner_looper == nullptr || env->ExceptionCheck()) return 0;
    receiver_shared = std::make_shared<DarwinInputReceiver>();
    ReceiverInitializationAdmission initialization(env, receiver_shared);
    if (!initialization.Admitted()) return 0;
    receiver_shared->weak_receiver = env->NewGlobalRef(weak_receiver);
    receiver_shared->channel = std::move(channel_resources);
    receiver_shared->original_channel_token =
        PinInputChannelToken(env, input_channel, receiver_shared->channel);
    if (receiver_shared->weak_receiver == nullptr ||
        receiver_shared->original_channel_token == nullptr ||
        env->ExceptionCheck())
      return 0;
    auto* receiver = receiver_shared.get();
    auto reservation = ReserveInputReceiverId();
    const ReceiverId receiver_id = reservation.Id();
    if (receiver_id == 0) return 0;
    receiver->registry_id = receiver_id;
    receiver->looper = owner_looper;
    const auto endpoint = std::make_shared<const InputRoutingEndpoint>(
        InputRoutingEndpoint{receiver->channel->Endpoint()->Transport(), receiver_id});
    const auto recipient = PrepareInputRoutingRecipient(receiver->channel->Routing(),
                                                        receiver_id, endpoint);
    if (recipient == nullptr) throw std::bad_alloc();
    auto token = std::make_shared<ReceiverTransportToken>(
        ReceiverTransportToken{receiver_id});
    const auto resolved = ResolveWeakWindowReceiverViewRoot(env, weak_receiver);
    jobject view_root = resolved.local_root;
    if (view_root != nullptr) {
      if (!env->ExceptionCheck()) receiver->view_root = env->NewGlobalRef(view_root);
      env->DeleteLocalRef(view_root);
    }
    if (env->ExceptionCheck() ||
        resolved.kind == ReceiverViewRootKind::kError ||
        (resolved.kind == ReceiverViewRootKind::kWindow &&
         receiver->view_root == nullptr))
      throw ReceiverInitializationFailure{};
    const auto transport = receiver->channel->Endpoint()->Transport();
    if (!receiver->binding.Prepare(
            receiver->looper, transport, transport->ReadFd(),
            transport->RemoteEndpointFd(),
            {.on_event = ReceiverTransportEvent,
             .on_status = ReceiverTransportStatus,
             .context = token.get(), .context_owner = token,
             .on_failure = ReceiverTransportFailure},
            receiver_id, receiver_id))
      throw ReceiverInitializationFailure{};
    if (!PrepareReceiverRetirement(
            *receiver, recipient, receiver->channel->Endpoint(),
            receiver->channel->Routing(), RefreshInputReceiverWritable))
      throw ReceiverInitializationFailure{};
    if (PublishInputReceiver(std::move(reservation), receiver_shared) != receiver_id)
      throw ReceiverInitializationFailure{};
    const auto routing_publication = PublishReceiverRetirement(*receiver);
    const auto publication = routing_publication.publication;
    if (!publication.Published()) throw ReceiverInitializationFailure{};
    const ReceiverId replaced_id = publication.predecessor.ConsumerId();
    if (replaced_id != 0 && replaced_id != receiver_id) {
      InputReceiverDispose(env, nullptr, static_cast<jlong>(replaced_id));
      if (env->ExceptionCheck()) throw std::bad_alloc();
    }
    if (receiver->view_root != nullptr) {
      diagnostics::LogViewRootResources(env, receiver->view_root);
      InputWindowGeometry geometry;
      if (ReadViewRootGeometry(env, receiver->view_root, &geometry) &&
          !receiver->disposed.load(std::memory_order_acquire)) {
        (void)UpdateInputRoutingReceiverGeometry(
            receiver->channel->Routing(), geometry.left, geometry.top,
            geometry.right, geometry.bottom, receiver->registry_id);
      }
      if (env->ExceptionCheck()) throw ReceiverInitializationFailure{};
    }
    if (!EnsureScheduler(receiver->channel, receiver->looper))
      throw ReceiverInitializationFailure{};
    if (InputRoutingConsumerMatches(receiver->channel->Routing(), receiver_id) &&
        !receiver->disposed.load(std::memory_order_acquire)) {
      if (!receiver->binding.Activate())
        throw ReceiverInitializationFailure{};
      if (env->ExceptionCheck()) throw ReceiverInitializationFailure{};
    }
    // Receiver construction is not a focus grant. WMS publication plus the
    // exact delivered-recipient/epoch readiness barrier authorize input.
    WakePending();
    if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
      const auto status = receiver->binding.Status();
      std::cerr << "ART Android InputChannel receiver-init name="
                << receiver->channel->Name()
                << " view_root=" << (receiver->view_root != nullptr ? 1 : 0)
                << " local_transport=" << (status.local_ready ? 1 : 0)
                << " remote_transport=" << (status.remote_ready ? 1 : 0)
                << "\n";
    }
    initialization.Commit();
    const auto registered = AcquireInputReceiver(receiver_id);
    bool current = false;
    if (registered == receiver_shared) {
      current = InputRoutingConsumerMatches(receiver->channel->Routing(),
                                             receiver_id) &&
                !receiver->disposed.load(std::memory_order_acquire);
    }
    if (current && receiver->view_root != nullptr &&
        (!framework::wm::EnsureDesktopRootClient(env, input_channel) ||
         !framework::wm::EnsureDesktopRootGeometryClient(env))) {
      // The receiver is committed already: retire this exact resource on
      // attachment exceptions rather than leaking a failed Java constructor.
      jthrowable attachment_exception = env->ExceptionOccurred();
      if (attachment_exception != nullptr) env->ExceptionClear();
      InputReceiverDispose(env, nullptr, static_cast<jlong>(receiver_id));
      if (attachment_exception != nullptr) {
        if (env->ExceptionCheck()) {
          env->ExceptionDescribe(); // cleanup must not replace the original
          env->ExceptionClear();
        }
        env->Throw(attachment_exception);
        env->DeleteLocalRef(attachment_exception);
      }
      throw ReceiverInitializationFailure{};
    }
    return current ? static_cast<jlong>(receiver_id) : 0;
  } catch (const std::bad_alloc&) {
    ThrowInputChannelOutOfMemory(env);
    return 0;
  } catch (const ReceiverInitializationFailure&) {
    if (!env->ExceptionCheck()) {
      jclass type = env->FindClass("java/lang/RuntimeException");
      if (type != nullptr) {
        (void)env->ThrowNew(type, "Could not initialize input event receiver");
        env->DeleteLocalRef(type);
      }
    }
    return 0;
  }
}

jboolean InputReceiverProbablyHasInput(JNIEnv* env, jclass, jlong pointer) {
  auto receiver_lease = AcquireInputReceiver(static_cast<ReceiverId>(pointer));
  auto* receiver = receiver_lease.get();
  const bool admitted = receiver != nullptr && receiver->admission.Admit();
  if (!admitted || receiver->channel == nullptr) {
    if (admitted) ReleaseReceiverAdmission(env, receiver);
    return JNI_FALSE;
  }
  const jboolean result = InputRoutingHasPending(receiver->channel->Routing())
                              ? JNI_TRUE
                              : JNI_FALSE;
  ReleaseReceiverAdmission(env, receiver);
  return result;
}

void InputReceiverReportTimeline(JNIEnv*, jclass, jlong, jint, jlong, jlong) {}

}  // namespace

InputRoutingHandle ReceiverRoutingHandle(const InputReceiver* receiver) {
  return receiver == nullptr || receiver->channel == nullptr
             ? InputRoutingHandle{}
             : receiver->channel->Routing();
}

bool AllocateReceiverFinish(InputReceiver* receiver, jint* sequence,
                            InputEventOrigin origin,
                            InputRoutingRecipientHandle original) {
  if (receiver == nullptr || sequence == nullptr) return false;
  uint32_t allocated = 0;
  if (!receiver->finishes.ReserveNext(&receiver->next_sequence, &allocated,
                                    origin, std::move(original)))
    return false;
  *sequence = std::bit_cast<jint>(allocated);
  return true;
}

bool CancelReceiverFinish(const InputReceiver* receiver, jint sequence) {
  return receiver != nullptr &&
         receiver->finishes.Cancel(static_cast<uint32_t>(sequence));
}

bool CloseReceiverFinishObservation(const InputReceiver* receiver, jint sequence,
                                    bool* handled) {
  return receiver != nullptr && receiver->finishes.CloseObservation(
      static_cast<uint32_t>(sequence), handled);
}

void RequestReceiverRoutingProgress(const InputReceiver* receiver) {
  if (receiver == nullptr || receiver->channel == nullptr) return;
  WakePending();
}

bool RegisterInputReceiverNatives(JNIEnv* env, JavaVM* vm,
                                 ReceiverChannelOps ops) {
  if (env == nullptr || ops.refresh_writable == nullptr ||
      ops.ensure_scheduler == nullptr || ops.wake_pending == nullptr ||
      vm == nullptr)
    return false;
  std::shared_ptr<const ReceiverBindings> previous;
  {
    std::lock_guard<std::mutex> lock(g_registration_mutex);
    if (g_registration_inflight) return false;
    g_registration_inflight = true;
    previous = std::atomic_load_explicit(&g_receiver_bindings,
                                         std::memory_order_acquire);
  }
  std::shared_ptr<const ReceiverBindings> bindings;
  try {
    bindings = std::make_shared<const ReceiverBindings>(
        ReceiverBindings{.vm = vm, .ops = ops});
  } catch (...) {
    std::lock_guard<std::mutex> lock(g_registration_mutex);
    g_registration_inflight = false;
    return false;
  }
  std::atomic_store_explicit(&g_receiver_bindings, bindings,
                             std::memory_order_release);
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeConsumeBatchedInputEvents"),
       const_cast<char*>("(JJ)Z"), reinterpret_cast<void*>(&InputReceiverConsume)},
      {const_cast<char*>("nativeDispose"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&InputReceiverDispose)},
      {const_cast<char*>("nativeDump"),
       const_cast<char*>("(JLjava/lang/String;)Ljava/lang/String;"),
       reinterpret_cast<void*>(&InputReceiverDump)},
      {const_cast<char*>("nativeFinishInputEvent"), const_cast<char*>("(JIZ)V"),
       reinterpret_cast<void*>(&InputReceiverFinish)},
      {const_cast<char*>("nativeInit"),
       const_cast<char*>("(Ljava/lang/ref/WeakReference;Landroid/view/InputChannel;"
                         "Landroid/os/MessageQueue;)J"),
       reinterpret_cast<void*>(&InputReceiverInit)},
      {const_cast<char*>("nativeProbablyHasInput"), const_cast<char*>("(J)Z"),
       reinterpret_cast<void*>(&InputReceiverProbablyHasInput)},
      {const_cast<char*>("nativeReportTimeline"), const_cast<char*>("(JIJJ)V"),
       reinterpret_cast<void*>(&InputReceiverReportTimeline)},
  };
  bool registered = false;
  jclass klass = nullptr;
  try {
    klass = env->FindClass("android/view/InputEventReceiver");
    if (klass != nullptr) {
      registered =
          env->RegisterNatives(klass, methods,
                               static_cast<jint>(std::size(methods))) == JNI_OK;
    }
  } catch (...) {
    registered = false;
  }
  if (klass != nullptr) {
    try {
      env->DeleteLocalRef(klass);
    } catch (...) {
      registered = false;
    }
  }
  if (!registered) {
    const auto current = std::atomic_load_explicit(
        &g_receiver_bindings, std::memory_order_acquire);
    if (current == bindings)
      std::atomic_store_explicit(&g_receiver_bindings, previous,
                                 std::memory_order_release);
  }
  {
    std::lock_guard<std::mutex> lock(g_registration_mutex);
    g_registration_inflight = false;
  }
  return registered;
}

}  // namespace darwin_art::input
