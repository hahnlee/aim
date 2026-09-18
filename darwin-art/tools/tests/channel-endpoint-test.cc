#include "runtime/framework/input/channel_endpoint.h"
#include "runtime/framework/input/channel_resources.h"
#include "runtime/framework/input/channel_identity_catalog.h"
#include "runtime/framework/input/input_channel_jni.h"
#include "runtime/framework/wm/window_input_publisher_jni.h"
#include "runtime/framework/input/input_transport_pump.h"
#include "runtime/framework/input/routing_transport_dispatch.h"
#include "runtime/framework/input/receiver_jni_resources.h"
#include "runtime/framework/input/receiver_lifecycle.h"
#include "runtime/framework/input/receiver_transport_policy.h"
#include "runtime/framework/input/receiver_input_consumer.h"
#include "runtime/framework/input/receiver_focus_control.h"
#include "runtime/framework/input/pending_receiver_retirement.h"
#include "runtime/framework/input/receiver_retirement_driver.h"
#include "tools/tests/input-owner-task-fixture.h"

#include <cassert>
#include <cstring>
#include <cstdio>
#include <cstdlib>
#include <new>
#include <vector>

static std::vector<uint8_t> wire;
static darwin_art::input::InputChannelIdentityCatalog* reentrant_catalog = nullptr;
static bool close_during_identity = false;
static bool reentrant_close_ready = true;
static bool reentrant_clear_ready = true;
static int created_weak_tokens = 0;
static int deleted_weak_tokens = 0;
static int identity_exception_checks = 0;
static std::shared_ptr<darwin_art::input::InputChannelResources> bind_during_weak_creation;
static std::unique_ptr<darwin_art::input::InputChannelIdentityCatalog>*
    destroy_catalog_on_weak_delete = nullptr;
static int close_count = 0;
static std::vector<int> closed_descriptors;
static bool wake_pending = false;
static bool blocked = false;
static bool terminal_send = false;
static bool open_failure = false;
static int remote_closes = 0;
static int allocation_failure_after = -1;
static bool allocation_failed = false;
static std::vector<uint8_t> incoming;
static bool incoming_eof = false;
static darwin_art::looper::FdCallback fd_callback = nullptr;
static void* fd_context = nullptr;
static darwin_art::looper::OwnerRelease fd_release = nullptr;
static int fd_events = 0;
static darwin_art::input::InputTransportPumpLease* retire_during_add = nullptr;
static darwin_art::input::InputTransportPumpLease* pump_refresh_during_add = nullptr;
static bool callback_before_publication = false;
static bool provider_allocation_failure = false;
void* operator new(size_t count) {
  if (allocation_failure_after == 0) {
    allocation_failure_after = -1;
    allocation_failed = true;
    throw std::bad_alloc();
  }
  if (allocation_failure_after > 0) --allocation_failure_after;
  if (void* value = std::malloc(count == 0 ? 1 : count)) return value;
  throw std::bad_alloc();
}
void operator delete(void* value) noexcept { std::free(value); }
void operator delete(void* value, size_t) noexcept { std::free(value); }
extern "C" int darwin_art_bionic_socket_broker_socketpair(int, int, int, int* fds) {
  if (open_failure) return -1;
  fds[0] = 17; fds[1] = 18; return 0;
}
extern "C" intptr_t darwin_art_bionic_socket_broker_send(
    int fd, const void* bytes, size_t count, int) {
  if (blocked || terminal_send) return -1;
  if (fd == 18) { assert(count == 1); wake_pending = true; }
  else {
    const auto* begin = static_cast<const uint8_t*>(bytes);
    wire.insert(wire.end(), begin, begin + count);
  }
  return static_cast<intptr_t>(count);
}
extern "C" intptr_t darwin_art_bionic_socket_broker_recv(
    int fd, void* bytes, size_t capacity, int) {
  if (fd != 17) {
    if (incoming.empty()) return incoming_eof ? 0 : -1;
    const size_t count = std::min(capacity, incoming.size());
    std::memcpy(bytes, incoming.data(), count);
    incoming.erase(incoming.begin(), incoming.begin() + count);
    return static_cast<intptr_t>(count);
  }
  assert(fd == 17);
  if (!wake_pending) return -1;
  *static_cast<uint8_t*>(bytes) = 1;
  wake_pending = false;
  return 1;
}
extern "C" int darwin_art_bionic_socket_broker_close(int fd) {
  ++close_count;
  closed_descriptors.push_back(fd);
  if (fd == 77) ++remote_closes;
  return 0;
}
extern "C" int darwin_art_bionic_errno_load() { return terminal_send ? 32 : 11; }
namespace darwin_art::looper {
int ScheduleTimedTaskAt(void*, int64_t, TimedTaskCallback, void*, TimedTaskRelease) {
  assert(false && "resource-lifetime test must not schedule an unbound continuation");
  return -1;
}
int AddFdOwned(void*, int, int, int events, FdCallback callback, void* context,
               void*, OwnerRelease release) {
  if (provider_allocation_failure) {
    provider_allocation_failure = false;
    release(context);  // Actual AddFdOwned consumes owner even when it throws.
    throw std::bad_alloc();
  }
  assert(fd_context == nullptr);
  fd_callback = callback; fd_context = context; fd_release = release;
  fd_events = events;
  if (callback_before_publication) {
    callback_before_publication = false;
    assert(callback(77, 1, context) == 1);
  }
  if (pump_refresh_during_add != nullptr) {
    auto* lease = pump_refresh_during_add;
    pump_refresh_during_add = nullptr;
    assert(lease->SetWritableResult(true) ==
           darwin_art::input::InputTransportWritableResult::kDeferred);
  }
  if (retire_during_add != nullptr) {
    auto* lease = retire_during_add;
    retire_during_add = nullptr;
    assert(lease->Retire());
    assert(lease->SetWritableResult(true) ==
           darwin_art::input::InputTransportWritableResult::kTerminal);
  }
  return 1;
}
int RemoveFdIfOwned(void*, int, FdCallback callback, void* context) {
  if (callback != fd_callback || context != fd_context) return 0;
  const auto release = fd_release;
  fd_callback = nullptr; fd_context = nullptr; fd_release = nullptr;
  release(context);
  return 1;
}
}
static void TestReceiverInitializationUnwind() {
  using namespace darwin_art::input;
  static int deleted_globals = 0;
  JNINativeInterface table{};
  table.DeleteGlobalRef = [](JNIEnv*, jobject) { ++deleted_globals; };
  JNIEnv env{&table};
  auto receiver = std::make_shared<InputReceiver>();
  receiver->weak_receiver = reinterpret_cast<jobject>(1);
  receiver->view_root = reinterpret_cast<jobject>(2);
  receiver->original_channel_token = reinterpret_cast<jobject>(3);
  try {
    ReceiverInitializationAdmission construction(&env, receiver);
    assert(construction.Admitted());
    receiver->registry_id = RegisterInputReceiver(receiver);
    // A callback admitted before rollback keeps JNI references alive.
    assert(receiver->admission.Admit());
    throw std::bad_alloc();
  } catch (const std::bad_alloc&) {}
  assert(AcquireInputReceiver(receiver->registry_id) == nullptr);
  assert(receiver->disposed && !receiver->admission.Admit());
  assert(deleted_globals == 0);
  ReleaseReceiverAdmission(&env, receiver.get());
  assert(deleted_globals == 3 && receiver->refs_cleaned);
  RetireReceiverResources(&env, receiver);
  assert(deleted_globals == 3);

  receiver = std::make_shared<InputReceiver>();
  receiver->weak_receiver = reinterpret_cast<jobject>(3);
  {
    ReceiverInitializationAdmission construction(&env, receiver);
    receiver->registry_id = RegisterInputReceiver(receiver);
    construction.Commit();
  }
  assert(AcquireInputReceiver(receiver->registry_id) == receiver);
  assert(!receiver->disposed && deleted_globals == 3);
  auto retired = RetireInputReceiver(receiver->registry_id);
  RetireReceiverResources(&env, retired);
  assert(deleted_globals == 4);

  receiver = std::make_shared<InputReceiver>();
  receiver->weak_receiver = reinterpret_cast<jobject>(4);
  { ReceiverInitializationAdmission early_return(&env, receiver); }
  assert(receiver->disposed && receiver->refs_cleaned && deleted_globals == 5);

  receiver = std::make_shared<InputReceiver>();
  receiver->weak_receiver = reinterpret_cast<jobject>(5);
  try {
    ReceiverCallbackAdmission callback(&env, receiver);
    assert(callback.Admitted());
    RetireReceiverResources(&env, receiver);
    assert(!receiver->refs_cleaned && deleted_globals == 5);
    throw std::bad_alloc();
  } catch (const std::bad_alloc&) {}
  assert(receiver->refs_cleaned && deleted_globals == 6);
  ReceiverCallbackAdmission rejected(&env, receiver);
  assert(!rejected.Admitted());
}

static void TestWmsPublisherRegistration() {
  static bool pending;
  static int mode;
  static int lookups;
  static int deleted;
  JNINativeInterface table{};
  JNIEnv env{&table};
  table.ExceptionCheck = [](JNIEnv*) -> jboolean { return pending ? JNI_TRUE : JNI_FALSE; };
  table.FindClass = [](JNIEnv*, const char* name) -> jclass {
    assert(!std::strcmp(name, "dev/darwinart/runtime/wm/WindowInputPublisher"));
    ++lookups;
    if (mode == 1) return nullptr;
    if (mode == 2) pending = true;
    return reinterpret_cast<jclass>(1);
  };
  table.RegisterNatives = [](JNIEnv*, jclass, const JNINativeMethod* methods,
                             jint count) -> jint {
    assert(count == 9 && !std::strcmp(methods[0].name, "nativePublish"));
    assert(!std::strcmp(methods[0].signature, "(Landroid/view/InputChannel;IIIIZ)I"));
    assert(!std::strcmp(methods[1].name, "nativePublishFocus"));
    assert(!std::strcmp(methods[1].signature, "(Landroid/view/InputChannel;JZ)I"));
    assert(!std::strcmp(methods[2].name, "nativeAcquireLease"));
    assert(!std::strcmp(methods[2].signature, "(Landroid/view/InputChannel;)J"));
    assert(!std::strcmp(methods[3].name, "nativeReleaseLease"));
    assert(!std::strcmp(methods[3].signature, "(J)Z"));
    assert(!std::strcmp(methods[4].name, "nativeTerminateLeaseAndQuiesce"));
    assert(!std::strcmp(methods[4].signature, "(J)Z"));
    assert(!std::strcmp(methods[5].name, "nativePublishLease"));
    assert(!std::strcmp(methods[5].signature, "(JIIIIZ)I"));
    assert(!std::strcmp(methods[6].name, "nativePublishFocusLease"));
    assert(!std::strcmp(methods[6].signature, "(JJZ)I"));
    assert(!std::strcmp(methods[7].name, "nativeFlushLease"));
    assert(!std::strcmp(methods[7].signature, "(J)I"));
    assert(!std::strcmp(methods[8].name, "nativeQueryAcceptedLeaseTx"));
    assert(!std::strcmp(methods[8].signature, "(J)I"));
    if (mode == 3) return JNI_ERR;
    if (mode == 4) pending = true;
    return JNI_OK;
  };
  table.DeleteLocalRef = [](JNIEnv*, jobject) { ++deleted; };
  using darwin_art::framework::wm::RegisterWindowInputPublisherNatives;
  assert(!RegisterWindowInputPublisherNatives(nullptr));
  pending = true;
  assert(!RegisterWindowInputPublisherNatives(&env) && !lookups && !deleted);
  for (mode = 1; mode <= 4; ++mode) {
    pending = false;
    deleted = 0;
    assert(!RegisterWindowInputPublisherNatives(&env));
    assert(deleted == (mode == 1 ? 0 : 1));
    assert(pending == (mode == 2 || mode == 4));
  }
  mode = 0;
  pending = false;
  deleted = 0;
  assert(RegisterWindowInputPublisherNatives(&env) && deleted == 1 && !pending);
}

static void TestNativeParcelIdentity() {
  using namespace darwin_art::input;
  static JNIEnv* current_env;
  static JavaVM* current_vm;
  static bool pending;
  static int globals;
  static int utf_calls;
  static int write_calls;
  static bool read_sets_exception;
  static bool write_failure;
  static bool fail_global_ref;
  static bool fail_global_ref_sets_exception;
  static int channel_lookups;
  static bool field_failure = true;
  static jlong pair_values[2];
  static jlongArray (*open_pair)(JNIEnv*, jclass, jstring);
  static jint (*publish)(JNIEnv*, jclass, jobject, jint, jint, jint, jint, jboolean);
  static jint (*publish_focus)(JNIEnv*, jclass, jobject, jlong, jboolean);
  static jlong (*acquire_lease)(JNIEnv*, jclass, jobject);
  static jboolean (*release_lease)(JNIEnv*, jclass, jlong);
  static jboolean (*terminate_lease)(JNIEnv*, jclass, jlong);
  static jint (*publish_lease)(JNIEnv*, jclass, jlong, jint, jint, jint, jint, jboolean);
  static jint (*publish_focus_lease)(JNIEnv*, jclass, jlong, jlong, jboolean);
  static jint (*flush_lease)(JNIEnv*, jclass, jlong);
  static jint (*query_accepted_lease_tx)(JNIEnv*, jclass, jlong);
  static jlong (*read)(JNIEnv*, jobject, jobject);
  static void (*write)(JNIEnv*, jobject, jobject, jlong);
  static void (*dispose)(JNIEnv*, jclass, jlong);
  static jlong (*get_finalizer)(JNIEnv*, jclass);
  struct Parcel { jobject token; const char* name; int fd; bool written = false; };
  JNINativeInterface table{};
  JNIInvokeInterface invoke{};
  JNIEnv env{&table};
  JavaVM vm{&invoke};
  current_env = &env; current_vm = &vm;
  invoke.GetEnv = [](JavaVM*, void** out, jint) -> jint {
    *out = current_env; return JNI_OK;
  };
  table.GetJavaVM = [](JNIEnv*, JavaVM** out) -> jint {
    *out = current_vm; return JNI_OK;
  };
  table.FindClass = [](JNIEnv*, const char*) -> jclass {
    return reinterpret_cast<jclass>(1);
  };
  table.RegisterNatives = [](JNIEnv*, jclass, const JNINativeMethod* methods,
                             jint count) -> jint {
    for (int i = 0; i < count; ++i) {
      const auto& m = methods[i];
      if (!std::strcmp(m.name, "nativeReadFromParcel")) read = reinterpret_cast<decltype(read)>(m.fnPtr);
      if (!std::strcmp(m.name, "nativeWriteToParcel")) write = reinterpret_cast<decltype(write)>(m.fnPtr);
      if (!std::strcmp(m.name, "nativeDispose")) dispose = reinterpret_cast<decltype(dispose)>(m.fnPtr);
      if (!std::strcmp(m.name, "nativeGetFinalizer")) get_finalizer = reinterpret_cast<decltype(get_finalizer)>(m.fnPtr);
      if (!std::strcmp(m.name, "nativePublish")) publish = reinterpret_cast<decltype(publish)>(m.fnPtr);
      if (!std::strcmp(m.name, "nativePublishFocus")) publish_focus = reinterpret_cast<decltype(publish_focus)>(m.fnPtr);
      if (!std::strcmp(m.name, "nativeAcquireLease")) acquire_lease = reinterpret_cast<decltype(acquire_lease)>(m.fnPtr);
      if (!std::strcmp(m.name, "nativeReleaseLease")) release_lease = reinterpret_cast<decltype(release_lease)>(m.fnPtr);
      if (!std::strcmp(m.name, "nativeTerminateLeaseAndQuiesce")) terminate_lease = reinterpret_cast<decltype(terminate_lease)>(m.fnPtr);
      if (!std::strcmp(m.name, "nativePublishLease")) publish_lease = reinterpret_cast<decltype(publish_lease)>(m.fnPtr);
      if (!std::strcmp(m.name, "nativePublishFocusLease")) publish_focus_lease = reinterpret_cast<decltype(publish_focus_lease)>(m.fnPtr);
      if (!std::strcmp(m.name, "nativeFlushLease")) flush_lease = reinterpret_cast<decltype(flush_lease)>(m.fnPtr);
      if (!std::strcmp(m.name, "nativeQueryAcceptedLeaseTx")) query_accepted_lease_tx = reinterpret_cast<decltype(query_accepted_lease_tx)>(m.fnPtr);
      if (!std::strcmp(m.name, "nativeOpenInputChannelPair")) open_pair = reinterpret_cast<decltype(open_pair)>(m.fnPtr);
    }
    return JNI_OK;
  };
  table.ExceptionCheck = [](JNIEnv*) -> jboolean { return pending ? JNI_TRUE : JNI_FALSE; };
  table.ThrowNew = [](JNIEnv*, jclass, const char*) -> jint { pending = true; return JNI_OK; };
  table.DeleteLocalRef = [](JNIEnv*, jobject) {};
  table.NewGlobalRef = [](JNIEnv*, jobject value) -> jobject {
    if (fail_global_ref) {
      if (fail_global_ref_sets_exception) pending = true;
      return nullptr;
    }
    ++globals;
    return value;
  };
  table.DeleteGlobalRef = [](JNIEnv*, jobject) { --globals; };
  table.NewWeakGlobalRef = [](JNIEnv*, jobject value) -> jweak { return value; };
  table.DeleteWeakGlobalRef = [](JNIEnv*, jweak) {};
  table.IsSameObject = [](JNIEnv*, jobject a, jobject b) -> jboolean { return a == b; };
  table.GetStringUTFChars = [](JNIEnv*, jstring value, jboolean*) -> const char* {
    ++utf_calls;
    return reinterpret_cast<const char*>(value);
  };
  table.ReleaseStringUTFChars = [](JNIEnv*, jstring, const char*) {};
  table.GetObjectClass = [](JNIEnv*, jobject) -> jclass {
    ++channel_lookups;
    return reinterpret_cast<jclass>(1);
  };
  table.GetFieldID = [](JNIEnv*, jclass, const char*, const char*) -> jfieldID {
    return reinterpret_cast<jfieldID>(1);
  };
  table.GetLongField = [](JNIEnv*, jobject object, jfieldID) -> jlong {
    if (field_failure) {
      pending = true;
      return 1; // Invalid address must not be dereferenced on JNI failure.
    }
    return *reinterpret_cast<jlong*>(object);
  };
  table.GetMethodID = [](JNIEnv*, jclass, const char*, const char*) -> jmethodID {
    return reinterpret_cast<jmethodID>(1);
  };
  table.NewObjectV = [](JNIEnv*, jclass, jmethodID, va_list) -> jobject {
    return reinterpret_cast<jobject>(77);
  };
  table.NewLongArray = [](JNIEnv*, jsize length) -> jlongArray {
    assert(length == 2);
    return reinterpret_cast<jlongArray>(pair_values);
  };
  table.SetLongArrayRegion = [](JNIEnv*, jlongArray, jsize start, jsize count,
                                const jlong* values) {
    assert(start == 0 && count == 2);
    std::copy(values, values + count, pair_values);
  };
  InputChannelParcelBridge bridge;
  bridge.read = [](JNIEnv*, jobject object, InputChannelParcelData* out) {
    auto* parcel = reinterpret_cast<Parcel*>(object);
    *out = {true, parcel->token, reinterpret_cast<jstring>(const_cast<char*>(parcel->name)), parcel->fd};
    parcel->fd = -1;
    if (read_sets_exception) pending = true;
    return true;
  };
  bridge.write = [](JNIEnv*, jobject object, bool initialized, jobject token,
                    const char* name, int fd) {
    auto* parcel = reinterpret_cast<Parcel*>(object);
    ++write_calls;
    if (write_failure) return false;
    *parcel = {token, name, fd, initialized}; return true;
  };
  assert(RegisterInputChannelJni(&env, bridge));
  assert(read && write && dispose && get_finalizer);
  assert(!publish); // WMS registration is not part of channel JNI ownership.
  assert(darwin_art::framework::wm::RegisterWindowInputPublisherNatives(&env));
  assert(publish && publish_focus);
  pending = true;
  const int lookups_before = channel_lookups;
  assert(publish(&env, nullptr, reinterpret_cast<jobject>(1), 0, 0, 100, 100,
                 JNI_TRUE) == 2);
  assert(pending && channel_lookups == lookups_before);
  assert(publish_focus(&env, nullptr, reinterpret_cast<jobject>(1), 0,
                       JNI_TRUE) == 2);
  assert(pending && channel_lookups == lookups_before);
  pending = false;
  assert(publish_focus(&env, nullptr, reinterpret_cast<jobject>(1), 0,
                       JNI_TRUE) == 2);
  assert(pending && channel_lookups == lookups_before);
  pending = false;
  assert(publish(&env, nullptr, reinterpret_cast<jobject>(1), 0, 0, 100, 100,
                 JNI_TRUE) == 2);
  assert(pending && channel_lookups == lookups_before + 1);
  pending = false;
  auto finalize = reinterpret_cast<void (*)(void*)>(get_finalizer(&env, nullptr));
  assert(open_pair);
  assert(open_pair(&env, nullptr, reinterpret_cast<jstring>(const_cast<char*>("wms-channel"))));
  assert(!pending && pair_values[0] && pair_values[1]);
  field_failure = false;
  assert(!AcquireServerInputChannelResources(&env, nullptr));
  const auto client_object = reinterpret_cast<jobject>(&pair_values[0]);
  const auto server_object = reinterpret_cast<jobject>(&pair_values[1]);
  const auto client_resources = AcquireInputChannelResources(pair_values[0]);
  pending = true;
  field_failure = true;
  assert(PinInputChannelToken(&env, client_object, client_resources) == nullptr &&
         pending);
  pending = false;
  field_failure = true;
  assert(PinInputChannelToken(&env, client_object, client_resources) == nullptr &&
         pending);
  pending = false;
  field_failure = false;
  assert(PinInputChannelToken(&env, client_object, nullptr) == nullptr && !pending);
  {
    auto other_core = std::make_shared<InputChannelResources>(
        client_resources->Name(), client_resources->Endpoint());
    const auto globals_before = globals;
    assert(PinInputChannelToken(&env, client_object, other_core) == nullptr && !pending);
    assert(globals == globals_before); // Equal metadata does not establish identity.
  }
  assert(PinInputChannelToken(&env, client_object,
                              std::make_shared<InputChannelResources>(
                                  "wrong-core", ChannelEndpoint::CreateLocal())) ==
         nullptr);
  fail_global_ref = true;
  assert(PinInputChannelToken(&env, client_object, client_resources) == nullptr &&
         !pending);
  fail_global_ref_sets_exception = true;
  assert(PinInputChannelToken(&env, client_object, client_resources) == nullptr &&
         pending);
  pending = false;
  fail_global_ref = false;
  fail_global_ref_sets_exception = false;
  const auto pinned_original_token =
      PinInputChannelToken(&env, client_object, client_resources);
  assert(pinned_original_token == reinterpret_cast<jobject>(77));
  assert(globals == 2);  // one channel-state ref plus the receiver-lifetime pin.
  env.DeleteGlobalRef(pinned_original_token);
  assert(globals == 1);
  {
    auto partial = std::make_shared<InputReceiver>();
    {
      ReceiverInitializationAdmission initialization(&env, partial);
      assert(initialization.Admitted());
      partial->channel = client_resources;
      partial->weak_receiver = env.NewGlobalRef(reinterpret_cast<jobject>(88));
      partial->original_channel_token =
          PinInputChannelToken(&env, client_object, client_resources);
      assert(partial->original_channel_token && globals == 3);
      pending = true; // Failure after acquiring the pin, before publication/Commit.
    }
    assert(pending && globals == 1 && partial->disposed && partial->refs_cleaned);
    assert(!partial->weak_receiver && !partial->original_channel_token);
    CleanupReceiverRefs(&env, partial.get());
    assert(globals == 1); // Initialization rollback owns exact-once deletion.
    pending = false;
  }
  assert(!AcquireServerInputChannelResources(&env, client_object));
  auto publication_lease = AcquireServerInputChannelResources(&env, server_object);
  assert(publication_lease && publication_lease == AcquireInputChannelResources(pair_values[0]));
  wire.clear();
  assert(publish(&env, nullptr, client_object, 0, 0, 720, 1280, JNI_TRUE) == 2);
  assert(wire.empty()); // A client wrapper cannot publish WMS control.
  assert(publish(&env, nullptr, server_object, 0, 0, 720, 1280, JNI_TRUE) == 0);
  assert(!pending && wire.size() == 32);
  wire.clear();
  assert(publish_focus(&env, nullptr, client_object, 1001, JNI_TRUE) == 2);
  assert(wire.empty()); // Only the server-side capability may publish focus.
  assert(publish_focus(&env, nullptr, server_object, 1001, JNI_TRUE) == 0);
  assert(!pending && wire.size() == sizeof(transport_wire::FocusControlFrame));
  transport_wire::FocusControlFrame focus_frame;
  std::memcpy(&focus_frame, wire.data(), sizeof(focus_frame));
  assert(focus_frame.epoch == 1001 && focus_frame.focused == 1);
  wire.clear();
  blocked = true;
  assert(publish(&env, nullptr, server_object, 0, 0, 720, 1280, JNI_TRUE) == 0);
  assert(wire.empty()); // Accepted means retained, not necessarily written yet.
  allocation_failed = false;
  allocation_failure_after = 0;
  assert(publish_focus(&env, nullptr, server_object, 1002, JNI_TRUE) == 1);
  assert(allocation_failed && !pending && wire.empty());
  allocation_failure_after = -1;
  blocked = false;
  assert(publish_focus(&env, nullptr, server_object, 1002, JNI_TRUE) == 0);
  assert(wire.size() == 32 + sizeof(transport_wire::FocusControlFrame));
  std::memcpy(&focus_frame, wire.data() + 32, sizeof(focus_frame));
  assert(focus_frame.epoch == 1002 && focus_frame.focused == 1);
  const auto retained_endpoint = publication_lease->Endpoint();
  assert(acquire_lease && release_lease && publish_lease && publish_focus_lease &&
         flush_lease && query_accepted_lease_tx);
  const jlong original_lease = acquire_lease(&env, nullptr, server_object);
  assert(original_lease != 0 && !pending);
  assert(acquire_lease(&env, nullptr, client_object) == 0);
  dispose(&env, nullptr, pair_values[0]);
  dispose(&env, nullptr, pair_values[1]);
  assert(PinInputChannelToken(&env, client_object, client_resources) == nullptr);
  assert(!AcquireServerInputChannelResources(&env, server_object));
  // Actual registered JNI lease retains the original server endpoint even
  // after Java dispose; its identity never re-resolves a current Java handle.
  assert(publish_lease(&env, nullptr, original_lease, 0, 0, 720, 1280, JNI_TRUE) == 0);
  assert(publish_focus_lease(&env, nullptr, original_lease, 1003, JNI_TRUE) == 0);
  assert(query_accepted_lease_tx(&env, nullptr, original_lease) == 0);
  assert(publish_focus_lease(&env, nullptr, original_lease, 0, JNI_TRUE) == 2);
  assert(terminate_lease && terminate_lease(&env, nullptr, 0) == JNI_FALSE);
  blocked = true;
  assert(publish_lease(&env, nullptr, original_lease, 1, 2, 721, 1282, JNI_TRUE) == 0);
  assert(query_accepted_lease_tx(&env, nullptr, original_lease) == 1);
  assert(flush_lease(&env, nullptr, original_lease) == 1);
  blocked = false;
  assert(flush_lease(&env, nullptr, original_lease) == 0);
  assert(query_accepted_lease_tx(&env, nullptr, original_lease) == 0);
  blocked = true;
  assert(publish_lease(&env, nullptr, original_lease, 2, 3, 722, 1283, JNI_TRUE) == 0);
  assert(query_accepted_lease_tx(&env, nullptr, original_lease) == 1);
  const auto pending_fence = retained_endpoint->Transport()->CaptureAcceptedTxFence();
  assert(retained_endpoint->Transport()->QueryTxFence(pending_fence)
         == InputTransportTxFenceStatus::kPending);
  assert(terminate_lease(&env, nullptr, original_lease) == JNI_TRUE);
  assert(!retained_endpoint->Transport()->HasPendingTx());
  assert(retained_endpoint->Transport()->QueryTxFence(pending_fence)
         == InputTransportTxFenceStatus::kTerminal);
  assert(query_accepted_lease_tx(&env, nullptr, original_lease) == 2);
  assert(!retained_endpoint->Transport()->IsRxTerminal());
  assert(terminate_lease(&env, nullptr, original_lease) == JNI_TRUE);
  assert(publish_focus_lease(&env, nullptr, original_lease, 1004, JNI_TRUE) == 2);
  blocked = false;
  assert(release_lease(&env, nullptr, original_lease) == JNI_TRUE);
  assert(query_accepted_lease_tx(&env, nullptr, original_lease) == 3);
  assert(terminate_lease(&env, nullptr, original_lease) == JNI_FALSE);
  assert(release_lease(&env, nullptr, original_lease) == JNI_FALSE);
  assert(publish_lease(&env, nullptr, original_lease, 0, 0, 720, 1280, JNI_TRUE) == 2);
  finalize(reinterpret_cast<void*>(pair_values[0]));
  finalize(reinterpret_cast<void*>(pair_values[1]));
  assert(globals == 0 && publication_lease->Endpoint() == retained_endpoint);
  assert(retained_endpoint->Transport()->ReadFd() == 17);
  publication_lease.reset();
  wire.clear();
  Parcel first{reinterpret_cast<jobject>(91), "parcel-channel", 88};
  const auto handle = read(&env, nullptr, reinterpret_cast<jobject>(&first));
  assert(handle && !pending && globals == 1);
  auto receiver = std::make_shared<InputReceiver>();
  receiver->channel = AcquireInputChannelResources(handle);
  const auto endpoint = receiver->channel->Endpoint();
  Parcel saved{};
  write(&env, nullptr, reinterpret_cast<jobject>(&saved), handle);
  assert(saved.written && saved.fd == 88 && saved.token == first.token);
  const int writes_before = write_calls;
  pending = true;
  write(&env, nullptr, reinterpret_cast<jobject>(&saved), handle);
  assert(pending && write_calls == writes_before);
  pending = false; write_failure = true;
  write(&env, nullptr, reinterpret_cast<jobject>(&saved), handle);
  assert(pending && write_calls == writes_before + 1);
  pending = false; write_failure = false;
  const int utf_before = utf_calls;
  read_sets_exception = true;
  Parcel failed_read{first.token, "parcel-channel", 95};
  const auto closes_before = closed_descriptors.size();
  assert(read(&env, nullptr, reinterpret_cast<jobject>(&failed_read)) == 0);
  assert(pending && utf_calls == utf_before);
  assert(closed_descriptors.size() == closes_before + 1 && closed_descriptors.back() == 95);
  pending = false; read_sets_exception = false;
  dispose(&env, nullptr, handle);
  finalize(reinterpret_cast<void*>(handle));
  assert(globals == 0);
  // Only the real receiver owns the core; no Java channel wrapper remains.
  saved.name = "parcel-channel"; saved.fd = 92;
  const auto before = closed_descriptors.size();
  const auto imported = read(&env, nullptr, reinterpret_cast<jobject>(&saved));
  assert(imported && !pending);
  assert(AcquireInputChannelResources(imported) == receiver->channel);
  assert(AcquireInputChannelResources(imported)->Endpoint() == endpoint);
  assert(closed_descriptors.size() == before + 1 && closed_descriptors.back() == 92);
  Parcel mismatch{first.token, "wrong-name", 93};
  assert(read(&env, nullptr, reinterpret_cast<jobject>(&mismatch)) == 0 && pending);
  assert(closed_descriptors.back() == 93);
  pending = false;
  auto& catalog = GetChannelIdentityCatalog();
  assert(catalog.CloseAdmission() && catalog.Clear(&env));
  Parcel closed{first.token, "parcel-channel", 94};
  assert(read(&env, nullptr, reinterpret_cast<jobject>(&closed)) == 0);
  assert(closed_descriptors.back() == 94);
  dispose(&env, nullptr, imported);
  finalize(reinterpret_cast<void*>(imported));
  assert(globals == 0);
  pending = false;
  close_count = 0;
  closed_descriptors.clear();
}

static void TestPreparedReceiverLifecycleRollback() {
  using namespace darwin_art::input;
  JNINativeInterface table{};
  table.DeleteGlobalRef = [](JNIEnv*, jobject) {};
  JNIEnv env{&table};
  auto routing = CreateInputRoutingState();
  auto endpoint = ChannelEndpoint::CreateLocal();
  assert(endpoint);
  std::weak_ptr<ChannelEndpoint> weak_endpoint = endpoint;
  auto receiver = std::make_shared<InputReceiver>();
  PendingReceiverRetirementHandle record;
  ReceiverRetirementDriverHandle driver;
  ReceiverId id = 0;
  try {
    ReceiverInitializationAdmission initialization(&env, receiver);
    assert(initialization.Admitted());
    auto reservation = ReserveInputReceiverId();
    id = receiver->registry_id = reservation.Id();
    receiver->looper = reinterpret_cast<void*>(1);
    const auto recipient = PrepareInputRoutingRecipient(
        routing, id, std::make_shared<const InputRoutingEndpoint>(
            InputRoutingEndpoint{endpoint->Transport(), id}));
    assert(receiver->binding.Prepare(receiver->looper, endpoint->Transport(),
                                    endpoint->Transport()->ReadFd(), -1, {}, id, id));
    assert(PrepareReceiverRetirement(*receiver, recipient, endpoint, routing,
                                    [](const InputRoutingHandle&) { return true; }));
    assert(receiver->routing_recipient.lock() == recipient);
    record = receiver->pending_retirement;
    driver = receiver->retirement_driver;
    assert(IsPendingReceiverRetirementRetained(record));
    assert(PublishInputReceiver(std::move(reservation), receiver) == id);
    assert(PublishReceiverRetirement(*receiver).publication.Published());
    assert(InputRoutingConsumerMatches(routing, id));
    throw std::bad_alloc();  // Failure after visibility must close the exact epoch.
  } catch (const std::bad_alloc&) {}
  assert(AcquireInputReceiver(id) == nullptr);
  assert(!InputRoutingConsumerMatches(routing, id));
  assert(receiver->disposed && receiver->refs_cleaned);
  receiver.reset();
  endpoint.reset();
  // The independently retained resource hook survives its receiver/wrapper.
  assert(!weak_endpoint.expired());
  assert(RequestPendingReceiverRetirementProgress(record));
  for (int i = 0; i < 16; ++i)
    (void)darwin_art::test::DispatchInputOwnerTasks();
  assert(driver->IsQuiescent());
  assert(!IsPendingReceiverRetirementRetained(record));
  record.reset();
  driver.reset();
  assert(weak_endpoint.expired());
  std::puts("receiver lifecycle: exact prepared/public rollback and independent resource hooks PASS");
}

static int policy_wakes = 0;
static bool focus_pending = false;
static bool focus_throw = false;
static bool focus_dispose = false;
static int focus_calls = 0;
static bool focused_value = false;
static std::shared_ptr<darwin_art::input::InputReceiver> focus_receiver;

// Kept separate from the existing focus-consumer fixture so the ordering
// regression has its own Java-observation count.
static int packet_focus_calls = 0;

static void TestReceiverFocusConsumer() {
  using namespace darwin_art::input;
  static bool fail_focus_lookup = false;
  JNINativeInterface table{};
  table.ExceptionCheck = [](JNIEnv*) -> jboolean { return focus_pending; };
  table.GetObjectClass = [](JNIEnv*, jobject) { return reinterpret_cast<jclass>(1); };
  table.FindClass = [](JNIEnv*, const char*) { return reinterpret_cast<jclass>(1); };
  table.GetMethodID = [](JNIEnv*, jclass, const char* name, const char*) {
    if (fail_focus_lookup && std::strcmp(name, "onFocusEvent") == 0) {
      focus_pending = true;
      return static_cast<jmethodID>(nullptr);
    }
    return reinterpret_cast<jmethodID>(1);
  };
  table.CallObjectMethodV = [](JNIEnv*, jobject, jmethodID, va_list) {
    return reinterpret_cast<jobject>(2);
  };
  table.CallVoidMethodV = [](JNIEnv* env, jobject, jmethodID, va_list arguments) {
    ++focus_calls;
    focused_value = va_arg(arguments, int) != 0;
    if (focus_throw) focus_pending = true;
    if (focus_dispose) {
      (void)RetireInputRoutingRecipient(focus_receiver->routing_recipient.lock());
      RetireReceiverResources(env, focus_receiver);
    }
  };
  table.DeleteLocalRef = [](JNIEnv*, jobject) {};
  table.DeleteGlobalRef = [](JNIEnv*, jobject) {};
  table.ThrowNew = [](JNIEnv*, jclass, const char*) -> jint {
    focus_pending = true;
    return JNI_OK;
  };
  JNIEnv env{&table};
  auto resources = std::make_shared<InputChannelResources>("focus", ChannelEndpoint::CreateLocal());
  auto routing = resources->Routing();
  auto endpoint = std::make_shared<const InputRoutingEndpoint>(
      InputRoutingEndpoint{resources->Endpoint()->Transport(), 103});
  auto recipient = PrepareInputRoutingRecipient(routing, 103, endpoint);
  assert(PublishInputRoutingRecipient(recipient).Published());
  PublishInputRoutingWmsFrame(routing, 0, 0, 360, 640, true);
  focus_receiver = std::make_shared<InputReceiver>();
  focus_receiver->weak_receiver = reinterpret_cast<jobject>(3);
  focus_receiver->routing_recipient = recipient;
  focus_receiver->channel = resources;
  ReceiverTransportPolicy policy{resources, recipient, nullptr, &env, focus_receiver};
  ReceiverInputConsumer focus_consumer(routing, recipient, nullptr, nullptr);
  policy.consumer = &focus_consumer;
  auto callbacks = ReceiverTransportCallbacks(&policy);
  const auto transport = resources->Endpoint()->Transport();
  AdoptRemoteInputTransport(transport.get(), 42);
  assert(CanDrainReceiverTransport(resources, 42, 0x0001 | 0x0008));
  assert(CanDrainReceiverTransport(resources, 42, 0x0008));
  assert(!CanDrainReceiverTransport(resources, transport->ReadFd(), 0x0008));
  assert(!CanDrainReceiverTransport(resources, 42, 0x0001 | 0x0004));
  assert(!CanDrainReceiverTransport(resources, 42, 0x0010));
  wire.clear();
  assert(SendInputTransportFocus(transport.get(), 1000, true) == InputTransportStatus::kAccepted);
  incoming = wire;
  wire.clear();
  focus_pending = true;
  assert(PumpInputTransport(transport.get(), 42, callbacks) == InputTransportStatus::kBackpressured);
  assert(focus_calls == 0);
  focus_pending = false;
  fail_focus_lookup = true;
  assert(PumpInputTransport(transport.get(), 42, callbacks) == InputTransportStatus::kBackpressured);
  assert(focus_pending && focus_calls == 0);
  fail_focus_lookup = focus_pending = false;
  assert(PumpInputTransport(transport.get(), 42, callbacks) == InputTransportStatus::kAccepted);
  assert(focus_calls == 1 && focused_value);
  assert(callbacks.on_focus(callbacks.context, 1000, true) == FocusControlCallbackResult::kConsumed);
  assert(focus_calls == 1);
  assert(PublishInputRoutingWmsFrame(routing, 0, 0, 360, 640, false));
  fail_focus_lookup = true;
  assert(callbacks.on_focus(callbacks.context, 1001, false) == FocusControlCallbackResult::kDeferred);
  assert(focus_pending && focus_calls == 1);
  fail_focus_lookup = focus_pending = false;
  assert(callbacks.on_focus(callbacks.context, 1001, false) == FocusControlCallbackResult::kConsumed);
  assert(focus_calls == 2 && !focused_value);
  (void)PublishInputRoutingWmsFrame(routing, 0, 0, 360, 640, true);
  assert(callbacks.on_focus(callbacks.context, 1000, true) == FocusControlCallbackResult::kConsumed);
  assert(focus_calls == 2); // Show alone cannot resurrect the old cached grant.
  focus_throw = true;
  assert(callbacks.on_focus(callbacks.context, 1002, true) == FocusControlCallbackResult::kConsumedStop);
  assert(focus_calls == 3 && focus_pending);
  focus_throw = focus_pending = false;
  // Invoked Java exceptions still consume the notification exactly once.
  assert(callbacks.on_focus(callbacks.context, 1002, true) == FocusControlCallbackResult::kConsumed);
  assert(focus_calls == 3);
  focus_dispose = true;
  assert(callbacks.on_focus(callbacks.context, 1003, false) == FocusControlCallbackResult::kConsumedStop);
  assert(focus_calls == 4 && focus_receiver->disposed && focus_receiver->refs_cleaned);
  assert(callbacks.on_focus(callbacks.context, 1004, true) == FocusControlCallbackResult::kConsumedStop);
  assert(focus_calls == 4);
  focus_dispose = false;
  focus_receiver.reset();
  std::puts("receiver focus consumer: original callback/exactly-once/exception/disposal PASS");
}

static void TestPacketPrecedesFocusDispatch() {
  using namespace darwin_art;
  using namespace darwin_art::input;
  wire.clear();
  incoming.clear();
  JNINativeInterface table{};
  table.ExceptionCheck = [](JNIEnv*) -> jboolean { return JNI_FALSE; };
  table.GetObjectClass = [](JNIEnv*, jobject) {
    return reinterpret_cast<jclass>(1);
  };
  table.GetMethodID = [](JNIEnv*, jclass, const char*, const char*) {
    return reinterpret_cast<jmethodID>(1);
  };
  table.CallObjectMethodV = [](JNIEnv*, jobject, jmethodID, va_list) {
    return reinterpret_cast<jobject>(2);
  };
  table.FindClass = [](JNIEnv*, const char*) { return reinterpret_cast<jclass>(1); };
  table.CallVoidMethodV = [](JNIEnv*, jobject, jmethodID, va_list) {
    ++packet_focus_calls;
  };
  table.DeleteLocalRef = [](JNIEnv*, jobject) {};
  table.DeleteGlobalRef = [](JNIEnv*, jobject) {};
  JNIEnv env{&table};

  auto endpoint = ChannelEndpoint::CreateLocal();
  assert(endpoint);
  const auto transport = endpoint->Transport();
  assert(transport && AdoptRemoteInputTransport(transport.get(), 42));
  auto resources = std::make_shared<InputChannelResources>(
      "packet-before-focus", endpoint);
  const auto routing = resources->Routing();
  const auto recipient = PrepareInputRoutingRecipient(
      routing, 104,
      std::make_shared<const InputRoutingEndpoint>(
          InputRoutingEndpoint{transport, 104}));
  assert(PublishInputRoutingRecipient(recipient).Published());
  PublishInputRoutingWmsFrame(routing, 0, 0, 360, 640, true);
  assert(SetInputRoutingFocus(routing, 104));

  auto receiver = std::make_shared<InputReceiver>();
  receiver->weak_receiver = reinterpret_cast<jobject>(3);
  receiver->channel = resources;
  receiver->routing_recipient = recipient;
  ReceiverTransportPolicy policy{resources, recipient, nullptr, &env, receiver};
  struct Invocation {
    bool ready = false;
    std::vector<uint64_t> sequences;
  } invocation;
  const auto invoke_packet =
      [](void* opaque, const InputRoutingRecipientHandle&, const DarwinArtInputPacket& packet,
         ReceiverPacketOrigin origin, InputRoutingPacketLease*) {
        auto& state = *static_cast<Invocation*>(opaque);
        if (!state.ready) return InputTransportConsumptionResult::kDeferred;
        assert(packet_focus_calls == (packet.key.sequence == 72 ? 1 : 0));
        assert(origin == (packet.key.sequence == 70
                              ? ReceiverPacketOrigin::kLocalQueue
                              : ReceiverPacketOrigin::kImportedChannel));
        state.sequences.push_back(packet.key.sequence);
        return InputTransportConsumptionResult::kConsumed;
      };
  ReceiverInputConsumer consumer(routing, recipient, invoke_packet, &invocation);
  policy.consumer = &consumer;
  const auto callbacks = ReceiverTransportCallbacks(&policy);

  DarwinArtInputPacket packet{};
  packet.kind = DarwinArtInputPacketKind::kKey;
  packet.key.version = 1;
  packet.key.size = sizeof(DarwinArtKeyEventV1);
  packet.key.action = 0;
  packet.key.sequence = 71;
  packet.key.key_code = 29;
  auto earlier_local = packet;
  earlier_local.key.sequence = 70;
  assert(EnqueueInputRoutingPacket(routing, earlier_local, recipient->id));
  assert(SendInputTransportPacket(transport.get(), packet) ==
         InputTransportStatus::kAccepted);
  assert(SendInputTransportWindow(transport.get(), 0, 0, 360, 640, true) ==
         InputTransportStatus::kAccepted);
  assert(SendInputTransportFocus(transport.get(), 10001, true) ==
         InputTransportStatus::kAccepted);
  packet.key.sequence = 72;
  assert(SendInputTransportPacket(transport.get(), packet) ==
         InputTransportStatus::kAccepted);

  packet_focus_calls = 0;
  incoming = wire;
  incoming_eof = true;
  wire.clear();
  const auto first = PumpInputTransport(transport.get(), 42, callbacks);

  // Earlier local invocation and the remote FIFO head are both deferred.
  // A following focus cannot overtake either, despite complete wire bytes.
  assert(first == InputTransportStatus::kBackpressured);
  assert(InputRoutingHasPending(routing));
  assert(packet_focus_calls == 0);

  assert(invocation.sequences.empty());
  invocation.ready = true;
  incoming.clear();
  ReceiverInputConsumer second_turn(routing, recipient, invoke_packet, &invocation, 1);
  policy.consumer = &second_turn;
  assert(PumpInputTransport(transport.get(), 42, callbacks) ==
         InputTransportStatus::kBackpressured);
  assert((invocation.sequences == std::vector<uint64_t>{70}));
  assert(second_turn.BudgetRetryNeeded() && packet_focus_calls == 0 && !transport->IsRxTerminal());
  ReceiverInputConsumer third_turn(routing, recipient, invoke_packet, &invocation, 1);
  policy.consumer = &third_turn;
  assert(PumpInputTransport(transport.get(), 42, callbacks) ==
         InputTransportStatus::kBackpressured);
  assert(packet_focus_calls == 1);
  assert((invocation.sequences == std::vector<uint64_t>{70, 71}));
  assert(third_turn.BudgetRetryNeeded() && !transport->IsRxTerminal());
  ReceiverInputConsumer fourth_turn(routing, recipient, invoke_packet, &invocation, 1);
  policy.consumer = &fourth_turn;
  assert(PumpInputTransport(transport.get(), 42, callbacks) == InputTransportStatus::kTerminal);
  assert((invocation.sequences == std::vector<uint64_t>{70, 71, 72}));
  assert(packet_focus_calls == 1 && transport->IsRxTerminal());
  assert(!InputRoutingHasPending(routing));
  incoming_eof = false;
  receiver.reset();
  std::puts("ordered receiver: retained local -> packet/window/focus/packet -> EOF, budget retry PASS");
}

static void TestReceiverTransportPolicy() {
  using namespace darwin_art::input;
  auto resources = std::make_shared<InputChannelResources>("policy", ChannelEndpoint::CreateLocal());
  auto routing = resources->Routing();
  auto original = PrepareInputRoutingRecipient(routing, 101);
  assert(PublishInputRoutingRecipient(original).Published());
  ReceiverTransportPolicy policy{resources, original, [] { ++policy_wakes; }, nullptr, {}};
  struct PolicyInvocation {
    InputRoutingHandle routing;
    int calls = 0;
  } invocation{routing};
  ReceiverInputConsumer consumer(routing, original,
      [](void* opaque, const InputRoutingRecipientHandle& recipient,
         const darwin_art::DarwinArtInputPacket&, ReceiverPacketOrigin,
         InputRoutingPacketLease*) {
        auto& state = *static_cast<PolicyInvocation*>(opaque);
        if (!InputRoutingConsumerMatches(state.routing, recipient->id))
          return InputTransportConsumptionResult::kConsumedStop;
        ++state.calls;
        return InputTransportConsumptionResult::kConsumed;
      }, &invocation);
  policy.consumer = &consumer;
  auto callbacks = ReceiverTransportCallbacks(&policy);
  assert(callbacks.on_window_consumption(callbacks.context, 10, 20, 110, 220, true) ==
         InputTransportConsumptionResult::kConsumed);
  assert(policy_wakes == 1);
  darwin_art::DarwinArtInputPacket packet{};
  packet.kind = darwin_art::DarwinArtInputPacketKind::kKey;
  assert(callbacks.on_packet_consumption(callbacks.context, packet) ==
         InputTransportConsumptionResult::kConsumed);
  assert(invocation.calls == 1 && !InputRoutingHasPending(routing));
  auto replacement = PrepareInputRoutingRecipient(routing, 102);
  assert(PublishInputRoutingRecipient(replacement).Published());
  // An old admitted callback must not redirect its packet to the new owner.
  assert(callbacks.on_packet_consumption(callbacks.context, packet) ==
         InputTransportConsumptionResult::kConsumedStop);
  assert(invocation.calls == 1);
  resources->Endpoint()->RegisterFinish(23);
  assert(callbacks.on_ack == nullptr && callbacks.on_ack64 == nullptr);
  bool handled = false;
  assert(!resources->Endpoint()->TakeFinish(23, &handled));
  assert(resources->Endpoint()->CancelFinish(23));
  policy.recipient.reset();
  assert(callbacks.on_packet_consumption(callbacks.context, packet) ==
         InputTransportConsumptionResult::kDeferred);
  std::puts("receiver transport policy: original token/window/ack/replacement PASS");
}

int main(int argc, char** argv) {
  if (argc == 2 && std::strcmp(argv[1], "--ordered-receiver-regression") == 0) {
    TestPacketPrecedesFocusDispatch();
    return 0;
  }
  TestReceiverInitializationUnwind();
  using namespace darwin_art::input;
  {
    auto transport = std::make_shared<InputTransport>();
    assert(OpenLocalInputTransport(transport.get()));
    AdoptRemoteInputTransport(transport.get(), 42);
    ChannelEndpoint endpoint(transport);
    assert(endpoint.Transport() == transport);
    bool handled = true;
    assert(!endpoint.TakeFinish(1, &handled) && handled);
    endpoint.RegisterFinish(1);
    assert(!endpoint.TakeFinish(1, &handled));
    assert(endpoint.RecordFinish(1, false));
    assert(!endpoint.RecordFinish(1, true));  // first ACK is immutable
    assert(endpoint.TakeFinish(1, &handled) && !handled);
    assert(!endpoint.TakeFinish(1, &handled));
    for (uint32_t seq = 1; seq <= 256; ++seq) assert(endpoint.TryRegisterFinish(seq));
    assert(!endpoint.TryRegisterFinish(256));  // duplicate is not capacity
    assert(endpoint.OverflowCount() == 0);
    assert(!endpoint.TryRegisterFinish(257));
    assert(endpoint.OverflowCount() == 1);
    assert(!endpoint.RecordFinish(257, true));  // never admitted
    for (uint32_t seq = 1; seq <= 256; ++seq) {
      assert(endpoint.RecordFinish(seq, (seq & 1) != 0));
      assert(endpoint.TakeFinish(seq, &handled));
      assert(handled == ((seq & 1) != 0));
    }
    assert(endpoint.TryRegisterFinish(257));
    assert(endpoint.CancelFinish(257));
    for (uint32_t seq = 1000; seq < 1300; ++seq) {
      assert(endpoint.TryRegisterFinish(seq));
      assert(!endpoint.CloseFinishObservation(seq, &handled));
      assert(endpoint.RecordFinish(seq, true));
      assert(!endpoint.TryRegisterFinish(seq));
      assert(endpoint.MarkFinishAckAccepted(seq));
      assert(endpoint.TryRegisterFinish(seq));
      assert(endpoint.CancelFinish(seq));
    }
    assert(endpoint.TryRegisterFinish(1400));
    assert(endpoint.RecordFinish(1400, false));
    assert(endpoint.MarkFinishAckAccepted(1400));
    assert(!endpoint.TryRegisterFinish(1400));
    assert(endpoint.CloseFinishObservation(1400, &handled) && !handled);
    assert(endpoint.WakeLocal() && wake_pending);
    endpoint.DrainLocalWake();
    assert(!wake_pending);
    blocked = true;
    assert(!endpoint.WakeLocal());
    blocked = false;
    assert(endpoint.SendAck(500, true) == InputTransportStatus::kAccepted);
    assert(!wire.empty());
  }
  assert(close_count == 3);
  open_failure = true;
  ChannelEndpointCreationError creation_error;
  assert(!ChannelEndpoint::CreateLocal(&creation_error));
  assert(creation_error == ChannelEndpointCreationError::kTransportUnavailable);
  int prior_remote = remote_closes;
  assert(!ChannelEndpoint::AdoptRemote(77, &creation_error));
  assert(creation_error == ChannelEndpointCreationError::kTransportUnavailable);
  assert(remote_closes == prior_remote + 1);
  open_failure = false;
  assert(!ChannelEndpoint::AdoptRemote(-1, &creation_error));
  assert(creation_error == ChannelEndpointCreationError::kInvalidDescriptor);
  int failures = 0;
  for (int position = 0; position < 16; ++position) {
    prior_remote = remote_closes;
    allocation_failed = false;
    allocation_failure_after = position;
    auto endpoint = ChannelEndpoint::AdoptRemote(77, &creation_error);
    const bool failed = allocation_failed;
    allocation_failure_after = -1;
    if (failed) {
      ++failures;
      assert(!endpoint);
      assert(creation_error == ChannelEndpointCreationError::kOutOfMemory);
      assert(remote_closes == prior_remote + 1);
    } else {
      assert(endpoint);
      assert(creation_error == ChannelEndpointCreationError::kNone);
      assert(endpoint->Transport()->RemoteEndpointFd() == 77);
      endpoint.reset();
      assert(remote_closes == prior_remote + 1);
      break;
    }
  }
  assert(failures >= 4);
  {
    auto endpoint = ChannelEndpoint::AdoptRemote(77);
    InputTransportPumpLease failed_lease;
    provider_allocation_failure = true;
    assert(!failed_lease.Register(reinterpret_cast<void*>(1), endpoint->Transport(), 77, 1, {}));
    assert(fd_context == nullptr && failed_lease.Retire());
    assert(failed_lease.IsQuiescent());
    assert(!failed_lease.Register(reinterpret_cast<void*>(1), endpoint->Transport(), 77, 1, {}));
    InputTransportPumpLease lease;
    callback_before_publication = true;
    pump_refresh_during_add = &lease;
    assert(lease.Register(reinterpret_cast<void*>(1), endpoint->Transport(), 77, 1, {}));
    assert((fd_events & 2) != 0);
    assert(lease.Retire() && fd_context == nullptr);
  }
  {
    auto endpoint = ChannelEndpoint::AdoptRemote(77);
    InputTransportPumpCallbacks callbacks{};
    InputTransportPumpLease rejected;
    retire_during_add = &rejected;
    assert(!rejected.Register(reinterpret_cast<void*>(1), endpoint->Transport(),
                              77, 1, callbacks));
    assert(fd_context == nullptr &&
           rejected.SetWritableResult(true) ==
               InputTransportWritableResult::kTerminal);
    InputTransportPumpLease lease;
    assert(lease.Register(reinterpret_cast<void*>(1), endpoint->Transport(),
                          77, 1, callbacks));
    blocked = true;
    assert(endpoint->SendAck(900, false) == InputTransportStatus::kAccepted);
    assert(lease.SetWritableResult(true) ==
           InputTransportWritableResult::kApplied);
    blocked = false;
    assert(lease.Retire() && fd_context == nullptr);
    // Fail the pump's first owned allocation; no provider registration may be
    // published and the lease remains safely retireable.
    allocation_failure_after = 0;
    InputTransportPumpLease allocation_failed_lease;
    assert(!allocation_failed_lease.Register(reinterpret_cast<void*>(1),
                                              endpoint->Transport(), 77, 1,
                                              callbacks));
    allocation_failure_after = -1;
    assert(fd_context == nullptr);
    assert(allocation_failed_lease.Retire());
  }
  {
    auto endpoint = ChannelEndpoint::AdoptRemote(77);
    assert(endpoint && endpoint->BorrowParcelFd(true) == 77 &&
           endpoint->BorrowParcelFd(false) == 77);
    struct Progress {
      std::shared_ptr<ChannelEndpoint>* endpoint;
      bool destroy = false;
      int calls = 0;
    } progress{&endpoint};
    const InputTransportPumpCallbacks callbacks{
        .on_ack = [](void* value, uint32_t sequence, bool handled) {
          auto* progress = static_cast<Progress*>(value);
          assert(*progress->endpoint != nullptr);
          assert((*progress->endpoint)->RecordFinish(sequence, handled));
        },
        .on_progress = [](void* value, InputTransportStatus) {
          auto* progress = static_cast<Progress*>(value);
          ++progress->calls;
          if (progress->destroy) progress->endpoint->reset();
        },
        .context = &progress};
    InputTransportPumpLease lease;
    assert(lease.Register(reinterpret_cast<void*>(1), endpoint->Transport(),
                          77, 1, callbacks));
    endpoint->RegisterFinish(44);
    wire.clear();
    assert(endpoint->SendAck(44, true) == InputTransportStatus::kAccepted);
    incoming = wire;
    auto callback = fd_callback;
    void* context = fd_context;
    assert(callback(77, 1, context) == 1);
    bool handled = false;
    assert(endpoint->TakeFinish(44, &handled) && handled);
    blocked = true;
    assert(endpoint->SendAck(45, false) == InputTransportStatus::kAccepted);
    assert(endpoint->Transport()->HasPendingTx());
    assert(lease.SetWritableResult(true) == InputTransportWritableResult::kApplied &&
           (fd_events & 2) != 0);
    blocked = false;
    callback = fd_callback; context = fd_context;
    assert(callback(77, 2, context) == 1);
    assert(!endpoint->Transport()->HasPendingTx());
    assert(lease.Retire() && fd_context == nullptr);

    // Re-register with a fresh lease for the deferred cleanup-lifetime check.
    InputTransportPumpLease cleanup_lease;
    assert(cleanup_lease.Register(reinterpret_cast<void*>(1),
                                  endpoint->Transport(), 77, 1, callbacks));
    auto weak = std::weak_ptr<ChannelEndpoint>(endpoint);
    progress.destroy = true;
    callback = fd_callback; context = fd_context;
    (void)callback(77, 1, context);
    assert(progress.calls >= 3 && weak.expired());
    assert(cleanup_lease.Retire() && fd_context == nullptr);
  }
  {
    auto endpoint = ChannelEndpoint::AdoptRemote(77);
    auto routing = CreateInputRoutingState();
    auto target = std::make_shared<const InputRoutingEndpoint>(
        InputRoutingEndpoint{endpoint->Transport(), 501});
    assert(SetInputRoutingConsumer(routing, 501, target) == 0);
    PublishInputRoutingWmsFrame(routing, 0, 0, 100, 100, true);
    assert(SetInputRoutingFocus(routing, 501));
    auto reserve = [&](uint64_t sequence) {
      DarwinArtPointerEventV2 pointer{};
      pointer.version = 2;
      pointer.size = sizeof(pointer);
      pointer.action = DARWIN_ART_POINTER_DOWN;
      pointer.pointer_count = 1;
      pointer.x = pointer.y = pointer.raw_x = pointer.raw_y = 20;
      pointer.sequence = sequence;
      InputRoutingAdmission admission;
      assert(RouteFrameworkPointerPacket(pointer, &admission) ==
             darwin_art::DarwinArtInputEnqueueResult::kQueued);
      InputRoutingInflightLease reservation;
      assert(ReserveInputRoutingPacket(std::move(admission), false, &reservation));
    };
    for (uint64_t sequence = 1; sequence <= 10; ++sequence) reserve(sequence);
    wire.clear();
    auto drained = DrainInputRoutingTransport(routing, 3);
    assert(drained.settled == 3 && drained.remote_admitted && !drained.local_queued);
    drained = DrainInputRoutingTransport(routing);
    assert(drained.settled == 7 && !drained.backpressured);
    struct Decoded { uint64_t expected = 1; size_t cancels = 0; } decoded;
    const InputTransportPumpCallbacks callbacks{
        .on_packet = [](void* value, const darwin_art::DarwinArtInputPacket& packet) {
          auto* decoded = static_cast<Decoded*>(value);
          if (packet.pointer.action == DARWIN_ART_POINTER_CANCEL) ++decoded->cancels;
          else assert(packet.pointer.sequence == decoded->expected++);
          return true;
        }, .context = &decoded};
    incoming = wire;
    assert(PumpInputTransport(endpoint->Transport().get(), 77, callbacks) !=
           InputTransportStatus::kTerminal);
    assert(decoded.expected == 11);
    assert(ClearInputRoutingFocus(routing, 501));
    wire.clear();
    drained = DrainInputRoutingTransport(routing);
    assert(drained.settled == 1 && drained.remote_admitted);
    incoming = wire;
    (void)PumpInputTransport(endpoint->Transport().get(), 77, callbacks);
    assert(decoded.cancels == 1);
    assert(SetInputRoutingFocus(routing, 501));
    reserve(11); reserve(12);
    wire.clear();
    terminal_send = true;
    drained = DrainInputRoutingTransport(routing);
    terminal_send = false;
    assert(drained.settled == 1 && !drained.remote_admitted && wire.empty());
    assert(endpoint->Transport()->IsTxTerminal() &&
           !endpoint->Transport()->IsRxTerminal() && !InputRoutingHasPending(routing));
    assert(!SetInputRoutingFocus(routing, 501));
  }
  {
    auto endpoint = ChannelEndpoint::CreateLocal();
    assert(endpoint);
    auto resources = std::make_shared<InputChannelResources>("resource-only", endpoint);
    InputChannelIdentityCatalog catalog;
    JNINativeInterface identity_table{};
    identity_table.ExceptionCheck = [](JNIEnv*) -> jboolean {
      ++identity_exception_checks;
      return JNI_FALSE;
    };
    identity_table.NewWeakGlobalRef = [](JNIEnv* env, jobject token) -> jweak {
      ++created_weak_tokens;
      if (auto canonical = std::exchange(bind_during_weak_creation, nullptr))
        assert(reentrant_catalog->Bind(env, token, std::move(canonical)).status ==
               ChannelIdentityStatus::kResolved);
      return reinterpret_cast<jweak>(token);
    };
    identity_table.DeleteWeakGlobalRef = [](JNIEnv*, jweak) {
      ++deleted_weak_tokens;
      if (auto* owner = std::exchange(destroy_catalog_on_weak_delete, nullptr))
        owner->reset();
    };
    identity_table.IsSameObject = [](JNIEnv* env, jobject first, jobject second) -> jboolean {
      if (close_during_identity) {
        close_during_identity = false;
        reentrant_close_ready = reentrant_catalog->CloseAdmission();
        reentrant_clear_ready = reentrant_catalog->Clear(env);
      }
      return first == second ? JNI_TRUE : JNI_FALSE;
    };
    JNIEnv identity_env{&identity_table};
    const auto java_token = reinterpret_cast<jobject>(0x1000);
    assert(catalog.Bind(&identity_env, java_token, resources).core == resources);
    assert(catalog.Bind(&identity_env, java_token, resources).core == resources);
    assert(created_weak_tokens == 2 && deleted_weak_tokens == 1);
    RegisterChannelResources(resources);
    RegisterChannelResources(resources);
    const auto routing = resources->Routing();
    std::weak_ptr<InputChannelResources> weak_resources = resources;
    auto receiver = std::make_shared<InputReceiver>();
    receiver->channel = resources;
    resources.reset();
    // Only receiver owns core; token identity still resolves after wrapper loss.
    assert(catalog.Find(&identity_env, java_token).core == receiver->channel);
    assert(!weak_resources.expired());
    assert(receiver->channel->Endpoint() == endpoint);
    assert(receiver->channel->Name() == "resource-only");
    assert(FindChannelResourcesForRouting(routing) == receiver->channel);
    auto snapshot = SnapshotChannelResources();
    assert(snapshot.size() == 1 && snapshot.front() == receiver->channel);
    snapshot.clear();
    reentrant_catalog = &catalog;
    close_during_identity = true;
    assert(catalog.Find(&identity_env, java_token).core == receiver->channel);
    assert(!reentrant_close_ready && !reentrant_clear_ready);
    assert(catalog.Find(&identity_env, java_token).status == ChannelIdentityStatus::kClosed);
    assert(catalog.CloseAdmission() && catalog.Clear(&identity_env));
    assert(created_weak_tokens == deleted_weak_tokens);
    assert(catalog.Clear(&identity_env));
    assert(created_weak_tokens == deleted_weak_tokens);
    const auto checks_after_clear = identity_exception_checks;
    assert(catalog.Find(&identity_env, java_token).status == ChannelIdentityStatus::kClosed);
    assert(catalog.Bind(&identity_env, java_token, receiver->channel).status ==
           ChannelIdentityStatus::kClosed);
    assert(identity_exception_checks == checks_after_clear);
    auto doomed_catalog = std::make_unique<InputChannelIdentityCatalog>();
    assert(doomed_catalog->Bind(&identity_env, java_token, receiver->channel).core ==
           receiver->channel);
    assert(doomed_catalog->CloseAdmission());
    destroy_catalog_on_weak_delete = &doomed_catalog;
    auto* doomed = doomed_catalog.get();
    assert(doomed->Clear(&identity_env));
    assert(!doomed_catalog && created_weak_tokens == deleted_weak_tokens);
    InputChannelIdentityCatalog conflicting_catalog;
    reentrant_catalog = &conflicting_catalog;
    auto candidate_a = std::make_shared<InputChannelResources>("name-A", endpoint);
    auto canonical_b = std::make_shared<InputChannelResources>("name-B", endpoint);
    bind_during_weak_creation = canonical_b;
    assert(conflicting_catalog.Bind(&identity_env, java_token, candidate_a).status ==
           ChannelIdentityStatus::kConflict);
    assert(conflicting_catalog.Find(&identity_env, java_token).core == canonical_b);
    assert(conflicting_catalog.CloseAdmission() && conflicting_catalog.Clear(&identity_env));
    assert(created_weak_tokens == deleted_weak_tokens);
    reentrant_catalog = nullptr;
    reentrant_catalog = nullptr;
    receiver.reset();
    assert(weak_resources.expired());
    assert(!FindChannelResourcesForRouting(routing));
    assert(SnapshotChannelResources().empty());
  }
  TestWmsPublisherRegistration();
  TestNativeParcelIdentity();
  TestPreparedReceiverLifecycleRollback();
  TestReceiverTransportPolicy();
  TestReceiverFocusConsumer();
  TestPacketPrecedesFocusDispatch();
  std::puts("channel endpoint: ACK/wake/FD/pump/retirement/remote-FIFO/terminal/resource-independence/native-Parcel PASS");
}
