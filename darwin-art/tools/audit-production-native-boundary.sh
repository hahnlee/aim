#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Host-state mutation belongs only to audit executables, not numeric providers
# copied into the production closure (even if the final linker strips it).
numeric_test_leaks="$(
  rg -n '_test_(prepare_host_state|host_state_is_preserved)' \
    "$root/tools/bionic-float-conversion-facade/src/provider.cc" \
    "$root/tools/bionic-float-conversion-facade/include/darwin_art_bionic_float_conversion.h" \
    "$root/tools/bionic-binary128-conversion-facade/src/provider.cc" \
    "$root/tools/bionic-binary128-conversion-facade/include/darwin_art_bionic_binary128_conversion.h" || true
)"
if [[ -n "$numeric_test_leaks" ]]; then
  echo 'production-native-boundary: numeric provider owns test host-state mutation:' >&2
  echo "$numeric_test_leaks" >&2
  exit 1
fi
case "${1:-graphics}" in
  graphics)
    runtime="$root/_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib"
    link_map="$root/_build/runtime-graphics-link-probe/runtime-graphics-link.map" ;;
  headless)
    runtime="$root/_build/runtime-link-probe/libdarwin_art_runtime.dylib"
    link_map="$root/_build/runtime-link-probe/runtime-link.map" ;;
  *) echo 'usage: audit-production-native-boundary.sh [graphics|headless]' >&2; exit 2 ;;
esac

# Packet admission is not receiver consumption. The actual receiver path
# must not restore the enqueue-only or destructive dequeue/requeue bypass.
receiver_consumption_bypasses="$(
  rg -n 'EnqueueInputRoutingPacket|DequeueInputRoutingPacket|RequeueInputRoutingPacket' \
    "$root/runtime/framework/input/receiver_transport_policy.cc" \
    "$root/runtime/framework/input/receiver_jni.cc" || true
)"
if [[ -n "$receiver_consumption_bypasses" ]]; then
  echo 'production-native-boundary: receiver bypasses ordered consumption owner:' >&2
  echo "$receiver_consumption_bypasses" >&2
  exit 1
fi

# Java sequence allocation must be reserved by the receiver epoch ledger;
# advancing the ViewRoot cursor first can collide with a live wrapped ID.
sequence_allocation_bypass="$(
  rg -n 'next_sequence|RegisterReceiverFinish' \
    "$root/runtime/framework/input/view_root_input_jni.cc" || true
)"
if [[ -n "$sequence_allocation_bypass" ]]; then
  echo 'production-native-boundary: ViewRoot bypasses atomic receiver sequence admission:' >&2
  echo "$sequence_allocation_bypass" >&2
  exit 1
fi

# Wire ACK observation and Java receiver completion are opposite directions.
# Receiver epoch state belongs to ReceiverFinishOwner, not the channel ledger.
finish_identity_bypasses="$(
  rg -n 'RecordFinish|TryRegisterFinish|TakeFinish|SendRemoteAckFrame' \
    "$root/runtime/framework/input/receiver_transport_policy.cc" \
    "$root/runtime/framework/input/receiver_jni.cc" || true
)"
if [[ -n "$finish_identity_bypasses" ]]; then
  echo 'production-native-boundary: receiver bypasses epoch/original-sink finish owner:' >&2
  echo "$finish_identity_bypasses" >&2
  exit 1
fi

# Aggregate transport owns descriptors/TX; framed RX belongs to its reader.
rx_owner_leaks="$(
  rg -n 'rx_admitted|rx_eof|impl_->rx\b|DecodeFocusControl|kAckFrameMagic|kWindowFrameMagic|kInputFrameMagic' \
    "$root/runtime/framework/input/input_transport.cc" || true
)"
if [[ -n "$rx_owner_leaks" ]]; then
  echo 'production-native-boundary: aggregate transport owns framed RX parser state:' >&2
  echo "$rx_owner_leaks" >&2
  exit 1
fi

# IOSurface/Metal allocation belongs to the narrow display backing provider,
# not the AppKit window/scanout owner. Its callers only commit owned tuples.
# Scanout may reimport an existing IOSurface texture after a backing epoch;
# that does not allocate a new display backing.
display_backing_owner_leaks="$(
  rg -n 'IOSurfaceCreate|AllocateSurfaceBacking' \
    "$root/compat/darwin_surface_bridge.mm" || true
)"
if [[ -n "$display_backing_owner_leaks" ]]; then
  echo 'production-native-boundary: surface bridge owns display backing allocation:' >&2
  echo "$display_backing_owner_leaks" >&2
  exit 1
fi

# AppKit scanout borrows fence readiness, not the monitor's FD/thread/FIFO
# internals. Keep the independently tested Darwin resource owner distinct.
composition_owner_leaks="$(
  rg -n 'MonitorCompositionFences|composition_monitor_started|composition_monitor_stop|composition_fences' \
    "$root/compat/darwin_surface_bridge.mm" \
    "$root/compat/darwin_surface_internal.h" || true
)"
if [[ -n "$composition_owner_leaks" ]]; then
  echo 'production-native-boundary: surface bridge owns composition monitor internals:' >&2
  echo "$composition_owner_leaks" >&2
  exit 1
fi

# Host process termination is an ABI/provider boundary, not a generic process
# state shim's lifecycle policy. Keep its independently tested owner distinct.
exit_owner_leaks="$(
  rg -n '^void darwin_art_bionic_(_?exit)[[:space:]]*\(' \
    "$root/tools/bionic-process-state-facade/src/shims.c" || true
)"
if [[ -n "$exit_owner_leaks" ]]; then
  echo 'production-native-boundary: process-state shim owns termination:' >&2
  echo "$exit_owner_leaks" >&2
  exit 1
fi

source_leaks="$(
  rg -n '#include[[:space:]]+["<][^">]*probes/' \
    "$root/runtime" "$root/compat" "$root/include" 2>/dev/null || true
)"
if [[ -n "$source_leaks" ]]; then
  echo "production-native-boundary: product source includes probes/:" >&2
  echo "$source_leaks" >&2
  exit 1
fi

# ViewRoot JNI owns Java resolution/dispatch, not the channel's private layout.
# The channel facade must not regain the moved framework implementations.
queue_owner_leaks="$(
  rg -n 'class DarwinMessageQueue|ToMessageQueue|^(jlong|void\*?|jboolean) message_queue_' \
    "$root/compat/darwin_framework_system_natives.cc" || true
)"
if [[ -n "$queue_owner_leaks" ]]; then
  echo 'production-native-boundary: mixed system natives regained MessageQueue ownership:' >&2
  echo "$queue_owner_leaks" >&2
  exit 1
fi

channel_jni_owner_leaks="$(
  rg -n '#include[[:space:]]+["<][^">]*channel_owner\.h[">]' \
    "$root/runtime/framework/input/input_channel_jni.h" \
    "$root/runtime/framework/input/input_channel_jni.cc" \
    "$root/runtime/framework/input/input_channel_jni_resources.h" || true
)"
if [[ -n "$channel_jni_owner_leaks" ]]; then
  echo 'production-native-boundary: channel JNI depends on receiver owner:' >&2
  echo "$channel_jni_owner_leaks" >&2
  exit 1
fi

viewroot_owner_leaks="$(
  rg -n 'DarwinInputChannelState|->channel|ExceptionClear|SetFrameworkViewRootFocus|FocusFrameworkViewRoot' \
    "$root/runtime/framework/input/view_root_input_jni.cc" || true
  rg -n '^bool (SetFrameworkViewRootFocus|FocusFrameworkViewRoot|DispatchFrameworkInputEvent)\(|^FrameworkInputEventDispatchResult DispatchFrameworkInputEventResult\(' \
    "$root/runtime/framework/input/channel_owner.cc" || true
)"
if [[ -n "$viewroot_owner_leaks" ]]; then
  echo 'production-native-boundary: ViewRoot/channel ownership or exception bypass:' >&2
  echo "$viewroot_owner_leaks" >&2
  exit 1
fi

# A focus receiver adapter delivers the original framework callback only;
# selection, channel authority and geometry remain with their own owners.
receiver_policy_leaks="$(
  rg -n '^(bool QueueTransportPacket|void ApplyTransport(Window|Ack))\(' \
    "$root/runtime/framework/input/receiver_jni.cc" || true
)"
if [[ -n "$receiver_policy_leaks" ]]; then
  echo 'production-native-boundary: receiver JNI regained transport policy:' >&2
  echo "$receiver_policy_leaks" >&2
  exit 1
fi

focus_adapter_leaks="$(
  rg -n 'GetFieldID\(|NewGlobalRef\(|ReadViewRoot|ResolveWeakWindowReceiverViewRoot|SetInputRoutingFocus\(|ClearInputRoutingFocus\(' \
    "$root/runtime/framework/input/receiver_focus_jni.cc" || true
)"
if [[ -n "$focus_adapter_leaks" ]]; then
  echo 'production-native-boundary: focus JNI adapter fabricates root/state or owns selection:' >&2
  echo "$focus_adapter_leaks" >&2
  exit 1
fi

# The focus executor uses prepared ledger operations, not packet/history
# mutation. Only publication/packet ledger, action owner, exact packet lease
# and focus executor may access
# private layout; it must not become a channel/receiver API.
focus_ledger_bypasses="$(
  rg -n '\.(packets|actions|streams|recipient_generations)\b' \
    "$root/runtime/framework/input/input_routing_focus.cc" || true
  rg -n '^bool (SetInputRoutingFocus|ClearInputRoutingFocus)\(' \
    "$root/runtime/framework/input/input_routing.cc" || true
  rg -n '#include.*input_routing_state_internal\.h' \
    "$root/runtime" "$root/compat" "$root/include" \
    | rg -v '/(input_routing|input_routing_actions|input_routing_focus|input_routing_packet_lease)\.cc:' || true
)"
if [[ -n "$focus_ledger_bypasses" ]]; then
  echo 'production-native-boundary: focus executor bypasses ledger or leaks private state:' >&2
  echo "$focus_ledger_bypasses" >&2
  exit 1
fi

# Action leases and the reservation/send/CANCEL ledger have one compiled
# owner. Publication must use its named locked ports, not regain layout writes
# or textually include implementation code into the old mixed owner.
action_owner_bypasses="$(
  rg -n 'struct RoutingActionLease(State|Access)|\.(actions|streams|terminated_endpoints|active_stream_generation)\b|->(actions|streams|terminated_endpoints|active_stream_generation)\b|^(bool|InputRoutingPacketCompletionStatus) (ReserveInputRoutingPacket|AcquireInputRoutingHead|BeginInputRoutingTransportSend|CompleteInputRoutingPacket|CompleteInputRoutingPacketWithStatus|AcquireInputRoutingCancellation|CompleteInputRoutingCancellation)\(' \
    "$root/runtime/framework/input/input_routing.cc" \
    "$root/runtime/framework/input/input_routing_actions_internal.h" || true
  rg -n '#include.*input_routing_actions\.cc' \
    "$root/runtime" "$root/compat" "$root/include" || true
  rg -n '\.(packets|recipient_generations|pending_input|notification_subscriptions)\b|->(packets|recipient_generations|pending_input|notification_subscriptions)\b|SendInputTransportPacket\(|GetFieldID\(|CallVoidMethod\(' \
    "$root/runtime/framework/input/input_routing_actions.cc" || true
)"
if [[ -n "$action_owner_bypasses" ]]; then
  echo 'production-native-boundary: publication bypasses compiled action owner:' >&2
  echo "$action_owner_bypasses" >&2
  exit 1
fi

# SurfaceControl may retain opaque buffers, not their gralloc layout/storage.
# Keep the actual ABI and IOSurface lifetime inside the buffer owner TU.
buffer_storage_leaks="$(
  rg -n 'struct AHardwareBuffer[[:space:]]*\{|g_hardware_buffer_aliases|buffer->(description|surface)' \
    "$root/compat/darwin_android_platform.mm" 2>/dev/null || true
)"
if [[ -n "$buffer_storage_leaks" ]]; then
  echo 'production-native-boundary: SurfaceControl knows private gralloc storage:' >&2
  echo "$buffer_storage_leaks" >&2
  exit 1
fi

# NDK/JNI entry points delegate mutation to the transaction builder; they may
# not expose reallocatable Update pointers across arbitrary discard callbacks.
transaction_builder_bypasses="$(
  rg -n 'FindUpdate[[:space:]]*\(|void Remember[[:space:]]*\(|transaction->(updates|controls|buffer_callbacks)' \
    "$root/compat/darwin_android_platform.mm" || true
)"
if [[ -n "$transaction_builder_bypasses" ]]; then
  echo 'production-native-boundary: platform facade owns transaction mutation:' >&2
  echo "$transaction_builder_bypasses" >&2
  exit 1
fi

# EGL context calls share one typed transport boundary after JNI conversion.
# Keep creation/current-context diagnostics out of the mixed EGL/JNI/image facade.
egl_context_bypasses="$(
  rg -n 'api\.(create_context|make_current)[[:space:]]*\(' \
    "$root/compat/darwin_angle_egl.cc" 2>/dev/null || true
)"
if [[ -n "$egl_context_bypasses" ]]; then
  echo 'production-native-boundary: EGL facade bypasses context dispatch:' >&2
  echo "$egl_context_bypasses" >&2
  exit 1
fi

# EGLSync ABI conversion remains in the facade, but native-fence identities,
# operation admission and retirement belong to the independently tested owner.
egl_fence_owner_bypasses="$(
  rg -n 'struct DarwinNativeFenceSync|NativeFenceSyncs[[:space:]]*\(|NativeFenceSyncMutex[[:space:]]*\(' \
    "$root/compat/darwin_angle_egl.cc" || true
)"
if [[ -n "$egl_fence_owner_bypasses" ]]; then
  echo 'production-native-boundary: EGL facade owns private fence lifetime:' >&2
  echo "$egl_fence_owner_bypasses" >&2
  exit 1
fi

# Android Vulkan owners may query/publish through typed interfaces, never
# acquire another owner's cache or registry. General raw driver dispatch must
# not live in WSI: Dawn's offscreen memory path is not a swapchain operation.
vulkan_owner_leaks="$(
  rg -n 'pub(\([^)]*\))?[[:space:]]+static[[:space:]]+(MOLTENVK|VULKAN_[A-Z_]+|IMPORTED_ANDROID_MEMORY)|wsi_backend_raw_device_symbol' \
    "$root/tools/android-dso-namespace/src" 2>/dev/null || true
)"
if [[ -n "$vulkan_owner_leaks" ]]; then
  echo 'production-native-boundary: Vulkan exposes owner state or routes raw dispatch through WSI:' >&2
  echo "$vulkan_owner_leaks" >&2
  exit 1
fi

# A logical device selects its own borrowed physical/Metal identity. A private
# last-created cache is still a lifetime bypass even when it is not exported.
vulkan_last_device_bypasses="$(
  rg -n '\b(VULKAN_INSTANCE|VULKAN_PHYSICAL_DEVICE|VULKAN_METAL_DEVICE|cached_physical_device|cached_metal_device|publish_physical_device|publish_metal_device|moltenvk_metal_device)\b' \
    "$root/tools/android-dso-namespace/src/vulkan" 2>/dev/null || true
)"
if [[ -n "$vulkan_last_device_bypasses" ]]; then
  echo 'production-native-boundary: Vulkan uses a last-instance/device lifetime bypass:' >&2
  echo "$vulkan_last_device_bypasses" >&2
  exit 1
fi

loader_policy_leaks="$(
    rg -n 'fixture_graph|IsExactFixtureGraph|g_elf_fixture|elf_jni_fixture_identity|darwin_art_fixture_record_lifecycle|bind_fixture_state|fixture_state_cleanup|fixture_state_for_state|darwin_art_unwindstack_check_|DARWIN_ART_TEST_FONTS_XML|DARWIN_ART_APK_APP_EXPECT_WIDGETS|MinimalForDarwinProbe|kProbeCanvas|struct DarwinPaint|struct DarwinRenderNode|RegisterHeadlessFixtureNatives|RegisterFrameworkRenderNodeNatives|RegisterFrameworkAssetManagerNatives|VerifyDarwinMediaCodecSurfaceLifecycle|darwin_art_debug_entrypoint' \
    "$root/runtime" "$root/compat" 2>/dev/null || true
)"
if [[ -n "$loader_policy_leaks" ]]; then
  echo 'production-native-boundary: product knows fixture loader policy:' >&2
  echo "$loader_policy_leaks" >&2
  exit 1
fi

# Service-owned process lifetime is not an AOSP ActiveServices concern. Keep
# probe-era scheduleExit cleanup out of the product implementation; the
# process owner decides shutdown after its own launch/transport failure.
active_services_exit_policy="$(
  rg -n 'scheduleExit[[:space:]]*\(' "$root/runtime/framework/am/ActiveServices.java" \
    2>/dev/null || true
)"
if [[ -n "$active_services_exit_policy" ]]; then
  echo 'production-native-boundary: ActiveServices retains service-owned scheduleExit policy:' >&2
  echo "$active_services_exit_policy" >&2
  exit 1
fi

# Surface input is a single-submit boundary. Keep the old AppKit mailbox and
# host pop/redispatch route out of product source even if a stale generated
# header or link object happens to make the ordinary fixture checks pass. The
# probes are intentionally excluded: they install an explicit retained sink
# whose queue and wake acknowledgement are fixture-owned.
input_bypass_leaks="$(
  rg -n 'darwin_art_surface_next_pointer_event|darwin_art_surface_next_key_event|darwin_art_surface_clear_input_hint_if_empty|nextPointerEvent|nextKeyEvent|clearInputHintIfEmpty|_pointerEvents|_keyEvents|ownerWakePending|NotifyFocusedInputChannel|FocusedChannelHasPackets|DequeueFocusedPacket|DequeueFrameworkPointerPacket|DequeueFrameworkKeyPacket|dispatch_queued_events' \
    "$root/runtime" "$root/compat" "$root/crates/darwin-art-engine" \
    "$root/crates/darwin-art-host" 2>/dev/null || true
)"
if [[ -n "$input_bypass_leaks" ]]; then
  echo 'production-native-boundary: product retains removed surface input mailbox/replay symbols:' >&2
  echo "$input_bypass_leaks" >&2
  exit 1
fi

# Binder owns Parcel layout and Binder RPC, not InputChannel state, receiver
# leases or the private input byte-stream decoder. The typed Parcel tuple is
# the only cross-owner serialization boundary.
remote_identity_owner_leaks="$(
  rg -n '"(controlFd|targetId|channelGeneration)"' \
    "$root/compat/darwin_framework_binder_natives.cc" || true
)"
if [[ -n "$remote_identity_owner_leaks" ]]; then
  echo 'production-native-boundary: wire owner regained RemoteBinder JNI field decoding:' >&2
  echo "$remote_identity_owner_leaks" >&2
  exit 1
fi

input_owner_leaks="$(
  rg -n 'DarwinInputChannelState|DarwinInputEventReceiver|remote_rx|remote_tx_mutex|InputReceiverInit|InputChannelReadParcel|SendRemoteInput|InputWindowPublish' \
    "$root/compat/darwin_framework_binder_natives.cc" 2>/dev/null || true
)"
if [[ -n "$input_owner_leaks" ]]; then
  echo 'production-native-boundary: Binder retains private input owner implementation:' >&2
  echo "$input_owner_leaks" >&2
  exit 1
fi
input_private_exports="$(
  rg -n 'DarwinInputChannelState|DarwinInputEventReceiver|g_input_owner_vm|std::(mutex|recursive_mutex|shared_ptr)|remote_rx|remote_tx_mutex' \
    "$root/runtime/framework/input/channel_owner.h" 2>/dev/null || true
)"
if [[ -n "$input_private_exports" ]]; then
  echo 'production-native-boundary: input interface exposes private owner state:' >&2
  echo "$input_private_exports" >&2
  exit 1
fi

# Receiver IDs must remain opaque: neither JNI handles nor Looper payloads
# may be decoded into allocation addresses. The registry owns lookup lifetime,
# while its interface must not expose JNI or private channel transport state.
receiver_owner_leaks="$(
  rg -n 'reinterpret_cast<(DarwinInputReceiver|InputReceiver)\*>|delete receiver|g_input_receivers|active_callbacks|dispose_requested' \
    "$root/runtime/framework/input/channel_owner.cc" 2>/dev/null || true
  rg -n 'JNIEnv|jobject|jni[.]h|DarwinInputChannelState|remote_rx|remote_tx_mutex' \
    "$root/runtime/framework/input/receiver_registry.h" 2>/dev/null || true
)"
import_reader_bypasses="$(
  sed -n '/^jlong InputChannelReadParcel(/,/^void InputChannelWriteParcel(/p' \
    "$root/runtime/framework/input/channel_owner.cc" |
    rg -n 'prepare_current_looper|RegisterServer|EnsureRoutingScheduler|[.]Register\(' || true
  rg -n 'RegisterServerOnlyTransport|ServerTransportPolicy|ResetLastFinish|LastFinish\(' \
    "$root/runtime/framework/input/channel_owner.cc" \
    "$root/runtime/framework/input/channel_endpoint.cc" \
    "$root/runtime/framework/input/channel_endpoint.h" || true
)"
if [[ -n "$import_reader_bypasses" ]]; then
  echo 'production-native-boundary: Parcel import registers a reader or input retains legacy ownership/ACK bypass:' >&2
  echo "$import_reader_bypasses" >&2
  exit 1
fi
if [[ -n "$receiver_owner_leaks" ]]; then
  echo 'production-native-boundary: receiver lifetime bypass or private interface leak:' >&2
  echo "$receiver_owner_leaks" >&2
  exit 1
fi

# InputChannel owns delivery/receiver leases, not Java packet construction.
# Keep event creation and pre-handoff cleanup in the independently tested
# packet dispatcher; recycling after framework invocation is framework-owned.
input_packet_owner_leaks="$(
  rg -n 'CreateMotionEvent|CreateKeyEvent|RecycleMotionEvent' \
    "$root/runtime/framework/input/channel_owner.cc" 2>/dev/null || true
)"
if [[ -n "$input_packet_owner_leaks" ]]; then
  echo 'production-native-boundary: InputChannel retains packet construction/recycling:' >&2
  echo "$input_packet_owner_leaks" >&2
  exit 1
fi

# Acknowledged storage is imported by the backing provider before queueing.
# Retained layers and in-flight composition must share that storage reference,
# not try to resurrect a possibly destroyed IOSurface ID in the worker.
composition_reimports="$(
  rg -n 'IOSurfaceLookup[[:space:]]*\(' \
    "$root/compat/surfaceflinger/service_darwin.mm" 2>/dev/null || true
)"
if [[ -n "$composition_reimports" ]]; then
  echo 'production-native-boundary: composition worker reimports unowned storage IDs:' >&2
  echo "$composition_reimports" >&2
  exit 1
fi

image_owner_bypasses="$(
  rg -n 'DarwinAhbEglImage|AhbEglImageMutex|AhbEglImages|std::unordered_map<[^>]*EGLImage' \
    "$root/compat/darwin_angle_egl.cc" 2>/dev/null || true
)"
if [[ -n "$image_owner_bypasses" ]]; then
  echo 'production-native-boundary: EGL facade owns private AHB image state:' >&2
  echo "$image_owner_bypasses" >&2
  exit 1
fi
output_recreation="$(
  rg -n 'targets[[:space:]]*\[' \
    "$root/compat/surfaceflinger/service_darwin.mm" 2>/dev/null || true
)"
if [[ -n "$output_recreation" ]]; then
  echo 'production-native-boundary: composition recreates unregistered output:' >&2
  echo "$output_recreation" >&2
  exit 1
fi
composition_owner_bypasses="$(
  rg -n 'g_composition_(target|layers|buffer_leases|context)|thread_local.*(CompositionTarget|ContextScope|CompositionBufferLease)' \
    "$root/compat/darwin_angle_egl.cc" 2>/dev/null || true
  rg -n 'NativeFenceSyncs|NativeFenceSyncMutex' \
    "$root/compat/graphics/composition_consumer.cc" 2>/dev/null || true
)"
if [[ -n "$composition_owner_bypasses" ]]; then
  echo 'production-native-boundary: composition consumer/fence ownership bypass:' >&2
  echo "$composition_owner_bypasses" >&2
  exit 1
fi

surface_control_bypasses="$(
  rg -n 'g_surface_controls|g_next_surface_transaction_id|struct[[:space:]]+SurfaceControl[[:space:]]*\{|ApplyReadySurfaceTransaction|CopySurfaceControlViewsLocked' \
    "$root/compat/darwin_android_platform.mm" 2>/dev/null || true
  rg -n 'g_surface_controls|reinterpret_cast<SurfaceControl|ASurfaceTransaction_apply|CompleteSurfaceTransaction' \
    "$root/compat/window/surface_control_submit_darwin.mm" 2>/dev/null || true
  rg -n 'IOSurfaceLookup|IOSurfaceGet|#import|glBind|eglMakeCurrent' \
    "$root/compat/window/surface_control_ready_transaction.cc" 2>/dev/null || true
)"
if [[ -n "$surface_control_bypasses" ]]; then
  echo 'production-native-boundary: SurfaceControl state/sequence/provider ownership bypass:' >&2
  echo "$surface_control_bypasses" >&2
  exit 1
fi

[[ -f "$runtime" ]] || {
  echo "production-native-boundary: missing runtime dylib: $runtime" >&2
  exit 2
}
[[ -f "$link_map" ]] || {
  echo "production-native-boundary: missing runtime link map: $link_map" >&2
  exit 2
}

# HWUI's directly linked lifecycle entry must share the Android EGL owner with
# JNI/ELF dispatch. Importing ANGLE's initializer bypasses display admission,
# even when window creation itself correctly binds our platform wrapper.
if [[ "${1:-graphics}" == graphics ]]; then
  egl_symbols="$(nm "$runtime")"
  if ! rg '[[:space:]][Tt][[:space:]]+_eglInitialize$' <<<"$egl_symbols" >/dev/null ||
      rg '[[:space:]]U[[:space:]]+_eglInitialize$' <<<"$egl_symbols" >/dev/null; then
    echo 'production-native-boundary: HWUI bypasses Android EGL lifecycle ownership:' >&2
    exit 1
  fi
fi

linked_fixtures="$(
  sed -n '/^# Object files:/,/^# Sections:/p' "$link_map" |
    rg '/probes/|runtime_.*_probe[.]cc[.]o|graphics_fixture|registration_fixture|acceptance_state|fixture_options|arttest_' || true
)"
if [[ -n "$linked_fixtures" ]]; then
  echo "production-native-boundary: product link contains fixture objects:" >&2
  echo "$linked_fixtures" >&2
  exit 1
fi

exported_fixtures="$(
  nm -gU "$runtime" |
    rg '(^|[[:space:]_])(Java_(Test|TestFast|TestCritical|CriticalSignatures|CriticalClinitCheck|Main_)|ProbeCanvas|darwin_art_runtime_.*probe|darwin_art_.*fixture)|TrampolineLiveCount|SetNextInputReceiverIdForTesting|darwin_art_android_metal_shared_event_reject_workers_for_test|darwin_art_bionic_(socket_broker_(is_active|live_objects)|dns_(live_results_for_test|retired_results_for_test|reset_for_test))' || true
)"
removed_input_exports="$(
  nm -gU "$runtime" |
    rg 'darwin_art_surface_(next_pointer_event|next_key_event|clear_input_hint_if_empty)|dispatch_queued_events' || true
)"
if [[ -n "$removed_input_exports" ]]; then
  echo 'production-native-boundary: product dylib exports removed surface input polling/replay ABI:' >&2
  echo "$removed_input_exports" >&2
  exit 1
fi
private_bypasses="$(nm "$runtime" | rg 'StartMinimalForDarwinProbe|FinishMinimalForDarwinProbe|darwin_art_debug_entrypoint' || true)"
if [[ -n "$private_bypasses" ]]; then
  echo 'production-native-boundary: product retains Probe startup or injected diagnostics:' >&2
  echo "$private_bypasses" >&2
  exit 1
fi
if [[ -n "$exported_fixtures" ]]; then
  echo "production-native-boundary: product dylib exports fixture ABI:" >&2
  echo "$exported_fixtures" >&2
  exit 1
fi

echo "production-native-boundary: PASS source-closure=clean link-closure=clean fixture-exports=0"
