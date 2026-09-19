// Test-only access to the actual private Parcel/wire owner. No test port is
// exported by the product and no alternate transport policy is implemented.
#include "../../compat/darwin_framework_binder_natives.cc"
#include "../../runtime/framework/wm/root_key_server_jni.h"

#include <cassert>
#include <cstdarg>
#include <poll.h>

// Unrelated host-provider boundaries are deliberately unavailable in this
// descriptor-free fixture; invoking one must fail the test, not fake success.
extern "C" int darwin_art_bionic_socket_broker_dup(int) { std::abort(); }
extern "C" int darwin_art_bionic_socket_broker_close(int) { std::abort(); }
extern "C" int darwin_art_bionic_fd_export_for_scm(int) { std::abort(); }
extern "C" int darwin_art_bionic_fd_import_from_scm(int) { std::abort(); }
extern "C" int darwin_art_android_shared_memory_get_guest_info(int, size_t*, int*) {
  std::abort();
}
extern "C" int darwin_art_android_shared_memory_adopt(int, size_t, int) {
  std::abort();
}
extern "C" int32_t darwin_art_runtime_registered_process_uid(uint32_t) { return -1; }

namespace {
struct Object {
  int fd;
  int target;
  jlong generation;
  jlong parcel = 0;
  bool remote = true;
};
Object constructed{-1, 0, 0};
WireHandle retire_during_constructor;
WireHandle retire_during_field_lookup;
int global_pins = 0;
bool pending = false;
int field_lookups = 0;
JavaVM* fake_vm = nullptr;
bool fail_get_vm = false;
jint GetVM(JNIEnv*, JavaVM** result) {
  *result = fail_get_vm ? nullptr : fake_vm;
  return fail_get_vm ? JNI_ERR : JNI_OK;
}
JNIEnv* worker_env = nullptr;
bool fail_attach = true;
std::atomic<int> detached{0};
jint Attach(JavaVM*, JNIEnv** result, void*) {
  *result = fail_attach ? nullptr : worker_env;
  return fail_attach ? JNI_ERR : JNI_OK;
}
jint Detach(JavaVM*) { detached.fetch_add(1, std::memory_order_release); return JNI_OK; }
jclass Class(JNIEnv*, jobject) { return reinterpret_cast<jclass>(1); }
jclass FindClass(JNIEnv*, const char*) { return reinterpret_cast<jclass>(1); }
jfieldID Field(JNIEnv* env, jclass, const char* name, const char*) {
  ++field_lookups;
  if (retire_during_field_lookup != nullptr) {
    auto retired = std::move(retire_during_field_lookup);
    auto transaction = g_wire_registry.Lock();
    RetireWireConnection(env, transaction, retired);
  }
  if (std::strcmp(name, "controlFd") == 0) return reinterpret_cast<jfieldID>(1);
  if (std::strcmp(name, "targetId") == 0) return reinterpret_cast<jfieldID>(2);
  if (std::strcmp(name, "channelGeneration") == 0) return reinterpret_cast<jfieldID>(3);
  assert(std::strcmp(name, "mNativePtr") == 0);
  return reinterpret_cast<jfieldID>(4);
}
jint Int(JNIEnv*, jobject object, jfieldID field) {
  auto* value = reinterpret_cast<Object*>(object);
  return field == reinterpret_cast<jfieldID>(1) ? value->fd : value->target;
}
jlong Long(JNIEnv*, jobject object, jfieldID field) {
  auto* value = reinterpret_cast<Object*>(object);
  return field == reinterpret_cast<jfieldID>(3) ? value->generation : value->parcel;
}
void DeleteLocal(JNIEnv*, jobject) {}
jobject Local(JNIEnv*, jobject object) { return object; }
jobject Global(JNIEnv*, jobject object) { ++global_pins; return object; }
void DeleteGlobal(JNIEnv*, jobject) { --global_pins; }
jboolean Same(JNIEnv*, jobject a, jobject b) { return a == b; }
jboolean Instance(JNIEnv*, jobject object, jclass) {
  return reinterpret_cast<Object*>(object)->remote;
}
jboolean Exception(JNIEnv*) { return pending; }
void ClearException(JNIEnv*) { pending = false; }
jint Register(JNIEnv*, jclass, const JNINativeMethod* methods, jint count) {
  assert(count == 1);
  assert(std::strcmp(methods[0].signature,
      "(IJIILandroid/os/Parcel;Landroid/os/Parcel;I)Z") == 0);
  return JNI_OK;
}
jmethodID Method(JNIEnv*, jclass, const char* name, const char* signature) {
  assert(std::strcmp(name, "<init>") == 0);
  assert(std::strcmp(signature, "(IIJ)V") == 0);
  return reinterpret_cast<jmethodID>(1);
}
jobject Construct(JNIEnv* env, jclass, jmethodID, va_list values) {
  constructed.fd = va_arg(values, jint);
  constructed.target = va_arg(values, jint);
  constructed.generation = va_arg(values, jlong);
  if (retire_during_constructor) {
    auto transaction = g_wire_registry.Lock();
    RetireWireConnection(env, transaction, retire_during_constructor);
  }
  return reinterpret_cast<jobject>(&constructed);
}
WireHandle Establish(int fd) {
  auto transaction = g_wire_registry.Lock();
  return EstablishWireConnection(transaction, fd);
}
void NoMessage(int fd) {
  pollfd descriptor{fd, POLLIN, 0};
  assert(poll(&descriptor, 1, 0) == 0);
}
}

int main() {
  JNINativeInterface table{};
  table.GetObjectClass = Class; table.FindClass = FindClass;
  table.GetFieldID = Field; table.GetIntField = Int; table.GetLongField = Long;
  table.DeleteLocalRef = DeleteLocal; table.NewLocalRef = Local;
  table.NewGlobalRef = Global; table.DeleteGlobalRef = DeleteGlobal;
  table.IsSameObject = Same; table.ExceptionCheck = Exception;
  table.IsInstanceOf = Instance;
  table.ExceptionClear = ClearException; table.RegisterNatives = Register;
  table.GetMethodID = Method; table.NewObjectV = Construct;
  table.GetJavaVM = GetVM;
  JNIEnv env{&table};
  JNIInvokeInterface vm_table{};
  vm_table.AttachCurrentThread = Attach;
  vm_table.DetachCurrentThread = Detach;
  JavaVM vm{&vm_table};
  fake_vm = &vm;
  worker_env = &env;
  int sockets[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);
  auto owner = Establish(sockets[0]);
  ScopedWireReader old_reader;
  {
    auto tx = g_wire_registry.Lock();
    assert(old_reader.Duplicate(owner->fd));
  }
  auto retained = owner->lifetime;
  DarwinParcel parcel;
  Object data{-1, 0, 0, reinterpret_cast<jlong>(&parcel)};
  assert(darwin_art::TransactRemoteBinder(&env, sockets[0], 0, 1, 7,
      reinterpret_cast<jobject>(&data), nullptr, 0) == JNI_FALSE);
  assert(field_lookups == 0);
  NoMessage(sockets[1]);

  Object proxy{sockets[0], 42, std::bit_cast<jlong>(owner->Generation())};
  using darwin_art::framework::wm::CaptureRootKeyServer;
  using darwin_art::framework::wm::RootKeyServerStatus;
  const auto trusted_class = reinterpret_cast<jclass>(1);
  auto capture = CaptureRootKeyServer(&env, reinterpret_cast<jobject>(&proxy), trusted_class);
  assert(capture.status == RootKeyServerStatus::kCaptured && capture.lifetime == retained);
  pending = true;
  auto exception_capture = CaptureRootKeyServer(&env,
      reinterpret_cast<jobject>(&proxy), trusted_class);
  assert(exception_capture.status == RootKeyServerStatus::kJniFailure && pending &&
         !exception_capture.lifetime);
  pending = false;
  auto transaction = [&] {
    auto tx = g_wire_registry.Lock();
    uint32_t target = 0;
    assert(RemoteTarget(&env, tx, owner, reinterpret_cast<jobject>(&proxy), &target)
        == RemoteTargetKind::kReturnHome && target == 42);
  };
  transaction();
  Object field_shape_impostor = proxy;
  field_shape_impostor.remote = false;
  const int before_impostor_capture = field_lookups;
  auto impostor_capture = CaptureRootKeyServer(&env,
      reinterpret_cast<jobject>(&field_shape_impostor), trusted_class);
  assert(impostor_capture.status == RootKeyServerStatus::kInvalidIdentity &&
         !impostor_capture.lifetime && field_lookups == before_impostor_capture);
  {
    auto tx = g_wire_registry.Lock();
    uint32_t target = 0;
    assert(RemoteTarget(&env, tx, owner,
        reinterpret_cast<jobject>(&field_shape_impostor), &target)
        == RemoteTargetKind::kLocal);
  }
  auto destination = Establish(sockets[1]);
  {
    auto tx = g_wire_registry.Lock();
    uint32_t target = 0;
    assert(RemoteTarget(&env, tx, destination, reinterpret_cast<jobject>(&proxy),
        &target) == RemoteTargetKind::kForward);
  }
  // The extracted JNI decoder can initialize/reenter through field lookup.
  // It must not authorize a proxy after original-owner teardown, nor forward
  // a retired source through another live destination.
  int field_sockets[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, field_sockets) == 0);
  auto field_owner = Establish(field_sockets[0]);
  Object field_proxy{field_sockets[0], 19,
      std::bit_cast<jlong>(field_owner->Generation())};
  retire_during_field_lookup = field_owner;
  {
    auto tx = g_wire_registry.Lock();
    uint32_t target = 77;
    assert(RemoteTarget(&env, tx, field_owner,
        reinterpret_cast<jobject>(&field_proxy), &target) == RemoteTargetKind::kStale);
    assert(target == 77 && !field_owner->lifetime->Live());
    assert(RemoteTarget(&env, tx, destination,
        reinterpret_cast<jobject>(&field_proxy), &target) == RemoteTargetKind::kStale);
    assert(target == 77 && WireOwnerCurrent(tx, destination));
  }
  close(field_sockets[0]);
  close(field_sockets[1]);
  // Actual capture decodes outside wire locks. JNI class initialization may
  // retire this exact generation; no FD-only fallback can revive its identity.
  int capture_sockets[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, capture_sockets) == 0);
  auto capture_owner = Establish(capture_sockets[0]);
  Object capture_proxy{capture_sockets[0], 23,
      std::bit_cast<jlong>(capture_owner->Generation())};
  retire_during_field_lookup = capture_owner;
  auto retired_capture = CaptureRootKeyServer(&env,
      reinterpret_cast<jobject>(&capture_proxy), trusted_class);
  assert(retired_capture.status == RootKeyServerStatus::kUnavailable &&
         !retired_capture.lifetime && !capture_owner->lifetime->Live());
  close(capture_sockets[0]);
  close(capture_sockets[1]);
  darwin_art::CloseRemoteBinderChannel(&env, sockets[0]);
  assert(!retained->Live());
  const int original_fd = sockets[0];
  close(original_fd);
  int replacement[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, replacement) == 0);
  // This is real OS descriptor reuse, not just replacing a map entry while
  // continuing to use the old shutdown socket.
  assert(replacement[0] == original_fd);
  // A stale reader must see the shutdown original, not consume successor data.
  WireHeader ready;
  ready.type = kWireReady;
  assert(SendWireMessage(replacement[1], ready, {}, {}, {}));
  WireMessage old_message;
  assert(!ReceiveWireMessage(old_reader.Fd(), &old_message));
  WireMessage successor_message;
  assert(ReceiveWireMessage(original_fd, &successor_message));
  assert(successor_message.header.type == kWireReady);
  auto successor = Establish(sockets[0]);
  assert(successor->Generation() != owner->Generation());
  assert(darwin_art::TransactRemoteBinder(&env, sockets[0], owner->Generation(),
      1, 7, reinterpret_cast<jobject>(&data), nullptr, 0) == JNI_FALSE);
  NoMessage(replacement[1]);
  assert(!darwin_art::FindRemoteBinderChannelLifetime(sockets[0], owner->Generation()));
  assert(darwin_art::FindRemoteBinderChannelLifetime(sockets[0], successor->Generation())
      == successor->lifetime);
  auto stale_capture = CaptureRootKeyServer(&env,
      reinterpret_cast<jobject>(&proxy), trusted_class);
  assert(stale_capture.status == RootKeyServerStatus::kUnavailable && !stale_capture.lifetime);
  Object successor_proxy{sockets[0], 42, std::bit_cast<jlong>(successor->Generation())};
  auto successor_capture = CaptureRootKeyServer(&env,
      reinterpret_cast<jobject>(&successor_proxy), trusted_class);
  assert(successor_capture.status == RootKeyServerStatus::kCaptured &&
         successor_capture.lifetime == successor->lifetime);
  WireHeader header;
  std::vector<WireBinder> binders;
  std::vector<uint8_t> bytes;
  std::vector<int> descriptors;
  parcel.binders.push_back(reinterpret_cast<jobject>(&proxy));
  assert(!ExportParcel(&env, destination, &parcel, &header, &binders, &bytes, &descriptors));
  assert(destination->payload->local_binders.empty() && global_pins == 0);
  parcel.binders.clear();

  // Actual wire send/read/ACK, descriptor-free and with no Java dispatch.
  successor->payload->ready = true;
  std::thread peer([&] {
    WireMessage request;
    assert(ReceiveWireMessage(replacement[1], &request));
    assert(request.header.type == kWireTransaction && request.header.code == 7);
    WireHeader reply;
    reply.type = kWireReply; reply.sequence = request.header.sequence;
    assert(SendWireMessage(replacement[1], reply, {}, {}, {}));
  });
  assert(darwin_art::TransactRemoteBinder(&env, sockets[0], successor->Generation(),
      1, 7, reinterpret_cast<jobject>(&data), nullptr, 0) == JNI_TRUE);
  peer.join();

  retire_during_constructor = successor;
  WireMessage imported;
  imported.binders.push_back({1, 0});
  assert(!ImportParcel(&env, successor, &imported, &parcel));
  assert(!successor->lifetime->Live() && parcel.binders.empty() && global_pins == 0);
  retire_during_constructor.reset();
  // Real provider startup owns cleanup even when no worker JNIEnv is available.
  auto startup_owner = Establish(sockets[0]);
  startup_owner->payload->local_binders.emplace(1,
      env.NewGlobalRef(reinterpret_cast<jobject>(&proxy)));
  fail_get_vm = true;
  assert(!darwin_art::StartRemoteBinderDispatcherForOwner(&env, startup_owner));
  assert(!startup_owner->lifetime->Live() && global_pins == 0);
  fail_get_vm = false;
  auto attach_owner = Establish(sockets[0]);
  attach_owner->payload->local_binders.emplace(1,
      env.NewGlobalRef(reinterpret_cast<jobject>(&proxy)));
  // Start from an outer recursive callback transaction: attach failure must
  // report/join without acquiring that mutex from the worker.
  {
    auto tx = g_wire_registry.Lock();
    assert(!darwin_art::StartRemoteBinderDispatcherForOwner(&env, attach_owner));
  }
  assert(!attach_owner->lifetime->Live() && global_pins == 0);
  int worker_sockets[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, worker_sockets) == 0);
  auto live_worker = Establish(worker_sockets[0]);
  live_worker->payload->local_binders.emplace(1,
      env.NewGlobalRef(reinterpret_cast<jobject>(&proxy)));
  fail_attach = false;
  assert(darwin_art::StartRemoteBinderDispatcherForOwner(&env, live_worker));
  close(worker_sockets[1]);
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
  while (detached.load(std::memory_order_acquire) == 0 &&
         std::chrono::steady_clock::now() < deadline) std::this_thread::yield();
  assert(detached.load(std::memory_order_acquire) == 1);
  assert(!live_worker->lifetime->Live() && global_pins == 0);
  close(worker_sockets[0]);
  darwin_art::CloseRemoteBinderChannel(&env, sockets[1]);
  close(sockets[0]); close(sockets[1]); close(replacement[1]);
  std::puts("Actual native Binder generation/Parcel/wire/reentrant retirement: PASS");
}
