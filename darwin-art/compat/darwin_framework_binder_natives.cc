#include "darwin_framework_natives.h"
#include "darwin_binder_wire.h"
#include "diagnostics/view_resources.h"
#include "binder/calling_identity.h"
#include "binder/context_manager.h"
#include "binder/peer_credentials.h"
#include "binder/process_registry.h"
#include "binder/wire_dispatch_policy.h"
#include "darwin_android_platform.h"
#include "darwin_android_time.h"
#include "../tools/bionic-socket-broker-adapter/include/darwin_art_bionic_socket_broker.h"

#include <cstdint>
#include <algorithm>
#include <atomic>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <iterator>
#include <iostream>
#include <limits>
#include <memory>
#include <mutex>
#include <new>
#include <optional>
#include <string>
#include <thread>
#include <type_traits>
#include <unordered_map>
#include <utility>
#include <vector>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

extern "C" int darwin_art_bionic_socket_broker_dup(int);
extern "C" int darwin_art_bionic_socket_broker_close(int);
extern "C" int darwin_art_bionic_fd_export_for_scm(int);
extern "C" int darwin_art_bionic_fd_import_from_scm(int);

#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
#include <binder/Parcel.h>
#include <utils/String16.h>
#include <utils/String8.h>

#include "android_os_Parcel.h"
#include "android_util_Binder.h"

int register_android_os_Binder(JNIEnv* env);
namespace android {
int register_android_os_Parcel(JNIEnv* env);
}  // namespace android
#endif

namespace {

JavaVM* g_framework_vm = nullptr;

struct DarwinParcel {
  std::vector<uint8_t> data;
  size_t position = 0;
  bool allow_fds = true;
  std::vector<jobject> binders;
  std::vector<int> file_descriptors;
};

struct DarwinInputChannelState;

struct DarwinInputReceiver {
  jobject weak_receiver = nullptr;
  jobject view_root = nullptr;
  std::shared_ptr<DarwinInputChannelState> channel;
  jint next_sequence = 1;
  jint last_finished_sequence = 0;
  bool last_finished_handled = false;
  bool focused = false;
  bool touch_mode = false;
  void* looper = nullptr;
  std::atomic<bool> transport_registered{false};
  std::atomic<bool> remote_transport_registered{false};
  std::atomic<bool> disposed{false};
  std::atomic<bool> dispose_requested{false};
  std::atomic<bool> refs_cleaned{false};
  std::atomic<uint32_t> active_callbacks{0};
};

// The Java WindowInputEventReceiver stores this native address as a jlong.
// Keep an independent strong lease so a Java dispose performed during
// dispatch cannot free the receiver before the native call returns.
std::mutex g_input_receiver_registry_mutex;
std::unordered_map<DarwinInputReceiver*,
                   std::shared_ptr<DarwinInputReceiver>>
    g_input_receiver_registry;

std::shared_ptr<DarwinInputReceiver> FindInputReceiverLease(
    DarwinInputReceiver* receiver) {
  if (receiver == nullptr) return nullptr;
  std::lock_guard<std::mutex> lock(g_input_receiver_registry_mutex);
  auto it = g_input_receiver_registry.find(receiver);
  return it == g_input_receiver_registry.end() ? nullptr : it->second;
}

void ClearParcel(JNIEnv* env, DarwinParcel* parcel) {
  if (parcel == nullptr) return;
  for (jobject binder : parcel->binders) env->DeleteGlobalRef(binder);
  for (int descriptor : parcel->file_descriptors)
    (void)darwin_art_bionic_socket_broker_close(descriptor);
  parcel->data.clear();
  parcel->position = 0;
  parcel->binders.clear();
  parcel->file_descriptors.clear();
}

DarwinParcel* Parcel(jlong pointer) {
  return reinterpret_cast<DarwinParcel*>(static_cast<std::uintptr_t>(pointer));
}

DarwinParcel* JavaParcel(JNIEnv* env, jobject parcel);

template <typename T>
jint ParcelWriteScalar(jlong pointer, T value) {
  auto* parcel = Parcel(pointer);
  if (parcel == nullptr) return -1;
  const size_t end = parcel->position + sizeof(T);
  if (end > parcel->data.size()) parcel->data.resize(end);
  std::memcpy(parcel->data.data() + parcel->position, &value, sizeof(T));
  parcel->position = end;
  return 0;
}

template <typename T>
T ParcelReadScalar(jlong pointer) {
  auto* parcel = Parcel(pointer);
  T value{};
  if (parcel == nullptr || parcel->position + sizeof(T) > parcel->data.size()) {
    return value;
  }
  std::memcpy(&value, parcel->data.data() + parcel->position, sizeof(T));
  parcel->position += sizeof(T);
  return value;
}

jlong ParcelCreate(JNIEnv*, jclass) {
  return static_cast<jlong>(reinterpret_cast<std::uintptr_t>(
      new (std::nothrow) DarwinParcel()));
}

void ParcelDestroy(JNIEnv* env, jclass, jlong pointer) {
  auto* parcel = Parcel(pointer);
  if (parcel == nullptr) return;
  ClearParcel(env, parcel);
  delete parcel;
}

void ParcelFreeBuffer(JNIEnv* env, jclass, jlong pointer) {
  if (auto* parcel = Parcel(pointer); parcel != nullptr) {
    ClearParcel(env, parcel);
  }
}

void ParcelMarkSensitive(jlong) {}
void ParcelMarkForBinder(JNIEnv*, jclass, jlong, jobject) {}
jboolean ParcelIsForRpc(jlong) { return JNI_FALSE; }
jint ParcelDataSize(jlong p) { return Parcel(p) == nullptr ? 0 : Parcel(p)->data.size(); }
jint ParcelDataAvail(jlong p) {
  auto* parcel = Parcel(p);
  return parcel == nullptr || parcel->position >= parcel->data.size()
      ? 0 : static_cast<jint>(parcel->data.size() - parcel->position);
}
jint ParcelDataPosition(jlong p) {
  return Parcel(p) == nullptr ? 0 : static_cast<jint>(Parcel(p)->position);
}
jint ParcelDataCapacity(jlong p) {
  return Parcel(p) == nullptr ? 0 : static_cast<jint>(Parcel(p)->data.capacity());
}
void ParcelSetDataSize(JNIEnv*, jclass, jlong p, jint size) {
  if (auto* parcel = Parcel(p); parcel != nullptr && size >= 0) {
    parcel->data.resize(static_cast<size_t>(size));
    if (parcel->position > parcel->data.size()) parcel->position = parcel->data.size();
  }
}
void ParcelSetDataPosition(jlong p, jint position) {
  if (auto* parcel = Parcel(p); parcel != nullptr && position >= 0) {
    parcel->position = std::min(static_cast<size_t>(position), parcel->data.size());
  }
}
void ParcelSetDataCapacity(JNIEnv*, jclass, jlong p, jint capacity) {
  if (auto* parcel = Parcel(p); parcel != nullptr && capacity >= 0) {
    parcel->data.reserve(static_cast<size_t>(capacity));
  }
}
jboolean ParcelPushAllowFds(jlong p, jboolean allow) {
  auto* parcel = Parcel(p);
  if (parcel == nullptr) return JNI_FALSE;
  const bool previous = parcel->allow_fds;
  parcel->allow_fds = allow == JNI_TRUE;
  return previous ? JNI_TRUE : JNI_FALSE;
}
void ParcelRestoreAllowFds(jlong p, jboolean allow) {
  if (auto* parcel = Parcel(p); parcel != nullptr) parcel->allow_fds = allow == JNI_TRUE;
}
jint ParcelWriteInt(jlong p, jint v) { return ParcelWriteScalar(p, v); }
jint ParcelWriteLong(jlong p, jlong v) { return ParcelWriteScalar(p, v); }
jint ParcelWriteFloat(jlong p, jfloat v) { return ParcelWriteScalar(p, v); }
jint ParcelWriteDouble(jlong p, jdouble v) { return ParcelWriteScalar(p, v); }
jint ParcelReadInt(jlong p) { return ParcelReadScalar<jint>(p); }
jlong ParcelReadLong(jlong p) { return ParcelReadScalar<jlong>(p); }
jfloat ParcelReadFloat(jlong p) { return ParcelReadScalar<jfloat>(p); }
jdouble ParcelReadDouble(jlong p) { return ParcelReadScalar<jdouble>(p); }

void ParcelWriteBytes(JNIEnv* env, jclass, jlong p, jbyteArray bytes,
                      jint offset, jint length) {
  auto* parcel = Parcel(p);
  if (parcel == nullptr || bytes == nullptr || offset < 0 || length < 0) return;
  ParcelWriteInt(p, length);
  const size_t end = parcel->position + static_cast<size_t>(length);
  if (end > parcel->data.size()) parcel->data.resize(end);
  env->GetByteArrayRegion(bytes, offset, length,
                          reinterpret_cast<jbyte*>(parcel->data.data() + parcel->position));
  parcel->position = end;
}

jbyteArray ParcelCreateByteArray(JNIEnv* env, jclass, jlong p) {
  const jint length = ParcelReadInt(p);
  auto* parcel = Parcel(p);
  if (parcel == nullptr || length < 0 ||
      parcel->position + static_cast<size_t>(length) > parcel->data.size()) return nullptr;
  jbyteArray result = env->NewByteArray(length);
  if (result != nullptr) {
    env->SetByteArrayRegion(result, 0, length,
        reinterpret_cast<const jbyte*>(parcel->data.data() + parcel->position));
    parcel->position += static_cast<size_t>(length);
  }
  return result;
}

jboolean ParcelReadByteArray(JNIEnv* env, jclass, jlong p, jbyteArray output,
                             jint length) {
  auto* parcel = Parcel(p);
  if (parcel == nullptr || output == nullptr || length < 0 ||
      parcel->position + static_cast<size_t>(length) > parcel->data.size()) {
    return JNI_FALSE;
  }
  env->SetByteArrayRegion(output, 0, length,
      reinterpret_cast<const jbyte*>(parcel->data.data() + parcel->position));
  parcel->position += static_cast<size_t>(length);
  return JNI_TRUE;
}

void ParcelWriteString(JNIEnv* env, jclass, jlong p, jstring value) {
  if (value == nullptr) { ParcelWriteInt(p, -1); return; }
  const char* utf = env->GetStringUTFChars(value, nullptr);
  if (utf == nullptr) { ParcelWriteInt(p, -1); return; }
  const jint length = static_cast<jint>(std::strlen(utf));
  ParcelWriteInt(p, length);
  auto* parcel = Parcel(p);
  const size_t end = parcel->position + static_cast<size_t>(length);
  if (end > parcel->data.size()) parcel->data.resize(end);
  std::memcpy(parcel->data.data() + parcel->position, utf, length);
  parcel->position = end;
  env->ReleaseStringUTFChars(value, utf);
}

jstring ParcelReadString(JNIEnv* env, jclass, jlong p) {
  const jint length = ParcelReadInt(p);
  auto* parcel = Parcel(p);
  if (parcel == nullptr || length < 0 ||
      parcel->position + static_cast<size_t>(length) > parcel->data.size()) return nullptr;
  std::string value(reinterpret_cast<const char*>(parcel->data.data() + parcel->position),
                    static_cast<size_t>(length));
  parcel->position += static_cast<size_t>(length);
  if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr &&
      (value.find("IChildProcessService") != std::string::npos ||
       value.find("base.apk") != std::string::npos ||
       value.find("partial-raster") != std::string::npos ||
       value.find("type=gpu") != std::string::npos)) {
    std::cerr << "ART Binder parcel: read string length=" << length
              << " value=" << value << " remaining="
              << (parcel->data.size() - parcel->position) << "\n";
  }
  return env->NewStringUTF(value.c_str());
}

void ParcelEnforceInterface(JNIEnv* env, jclass, jlong p, jstring expected) {
  jstring actual = ParcelReadString(env, nullptr, p);
  const char* expected_utf =
      expected == nullptr ? nullptr : env->GetStringUTFChars(expected, nullptr);
  const char* actual_utf =
      actual == nullptr ? nullptr : env->GetStringUTFChars(actual, nullptr);
  const bool matches = expected_utf != nullptr && actual_utf != nullptr &&
                       std::strcmp(expected_utf, actual_utf) == 0;
  if (actual_utf != nullptr) env->ReleaseStringUTFChars(actual, actual_utf);
  if (expected_utf != nullptr) env->ReleaseStringUTFChars(expected, expected_utf);
  env->DeleteLocalRef(actual);
  if (!matches && !env->ExceptionCheck()) {
    jclass security_exception = env->FindClass("java/lang/SecurityException");
    if (security_exception != nullptr) {
      env->ThrowNew(security_exception, "Binder interface token mismatch");
    }
    env->DeleteLocalRef(security_exception);
  }
}

void ParcelWriteStrongBinder(JNIEnv* env, jclass, jlong p, jobject binder) {
  auto* parcel = Parcel(p);
  if (parcel == nullptr) return;
  if (binder == nullptr) { ParcelWriteInt(p, -1); return; }
  parcel->binders.push_back(env->NewGlobalRef(binder));
  ParcelWriteInt(p, static_cast<jint>(parcel->binders.size() - 1));
}

jobject ParcelReadStrongBinder(JNIEnv* env, jclass, jlong p) {
  auto* parcel = Parcel(p);
  const jint index = ParcelReadInt(p);
  return parcel == nullptr || index < 0 ||
      static_cast<size_t>(index) >= parcel->binders.size()
      ? nullptr : env->NewLocalRef(parcel->binders[static_cast<size_t>(index)]);
}

bool ParcelWriteGuestFileDescriptor(jlong p, int source) {
  auto* parcel = Parcel(p);
  if (parcel == nullptr || !parcel->allow_fds) return false;
  const int duplicate =
      source < 0 ? -1 : darwin_art_bionic_socket_broker_dup(source);
  if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
    std::cerr << "ART Binder parcel: write fd source=" << source
              << " duplicate=" << duplicate << " errno=" << errno << "\n";
  }
  if (duplicate < 0) {
    ParcelWriteInt(p, -1);
    return false;
  }
  parcel->file_descriptors.push_back(duplicate);
  ParcelWriteInt(p, static_cast<jint>(parcel->file_descriptors.size() - 1));
  return true;
}

int ParcelReadGuestFileDescriptor(jlong p) {
  auto* parcel = Parcel(p);
  const jint index = ParcelReadInt(p);
  if (parcel == nullptr || index < 0 ||
      static_cast<size_t>(index) >= parcel->file_descriptors.size()) {
    return -1;
  }
  return darwin_art_bionic_socket_broker_dup(
      parcel->file_descriptors[static_cast<size_t>(index)]);
}

void ParcelWriteFileDescriptor(JNIEnv* env, jclass, jlong p,
                               jobject file_descriptor) {
  auto* parcel = Parcel(p);
  if (parcel == nullptr || file_descriptor == nullptr || !parcel->allow_fds) {
    return;
  }
  jclass descriptor_class = env->GetObjectClass(file_descriptor);
  jfieldID descriptor_field =
      descriptor_class == nullptr
          ? nullptr
          : env->GetFieldID(descriptor_class, "descriptor", "I");
  const int source = descriptor_field == nullptr
                         ? -1
                         : env->GetIntField(file_descriptor, descriptor_field);
  env->DeleteLocalRef(descriptor_class);
  (void)ParcelWriteGuestFileDescriptor(p, source);
}

jobject ParcelReadFileDescriptor(JNIEnv* env, jclass, jlong p) {
  auto* parcel = Parcel(p);
  const size_t descriptor_count =
      parcel == nullptr ? 0 : parcel->file_descriptors.size();
  const size_t position = parcel == nullptr ? 0 : parcel->position;
  const int duplicate = ParcelReadGuestFileDescriptor(p);
  if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
    std::cerr << "ART Binder parcel: read fd duplicate=" << duplicate
              << " count=" << descriptor_count
              << " position=" << position
              << "\n";
  }
  if (duplicate < 0) return nullptr;
  jclass descriptor_class = env->FindClass("java/io/FileDescriptor");
  jmethodID constructor = descriptor_class == nullptr
                              ? nullptr
                              : env->GetMethodID(descriptor_class, "<init>", "()V");
  jfieldID descriptor_field =
      descriptor_class == nullptr
          ? nullptr
          : env->GetFieldID(descriptor_class, "descriptor", "I");
  jobject result = constructor == nullptr || descriptor_field == nullptr
                       ? nullptr
                       : env->NewObject(descriptor_class, constructor);
  if (result != nullptr) env->SetIntField(result, descriptor_field, duplicate);
  if (result == nullptr)
    (void)darwin_art_bionic_socket_broker_close(duplicate);
  env->DeleteLocalRef(descriptor_class);
  return result;
}

jbyteArray ParcelMarshall(JNIEnv* env, jclass, jlong p) {
  auto* parcel = Parcel(p);
  if (parcel == nullptr) return nullptr;
  jbyteArray result = env->NewByteArray(static_cast<jsize>(parcel->data.size()));
  if (result != nullptr && !parcel->data.empty()) {
    env->SetByteArrayRegion(result, 0, static_cast<jsize>(parcel->data.size()),
        reinterpret_cast<const jbyte*>(parcel->data.data()));
  }
  return result;
}

void ParcelUnmarshall(JNIEnv* env, jclass, jlong p, jbyteArray bytes,
                      jint offset, jint length) {
  auto* parcel = Parcel(p);
  if (parcel == nullptr || bytes == nullptr || offset < 0 || length < 0) return;
  ClearParcel(env, parcel);
  parcel->data.resize(static_cast<size_t>(length));
  env->GetByteArrayRegion(bytes, offset, length,
                          reinterpret_cast<jbyte*>(parcel->data.data()));
  parcel->position = 0;
}

jint ParcelCompareData(JNIEnv*, jclass, jlong left, jlong right) {
  auto* a = Parcel(left);
  auto* b = Parcel(right);
  if (a == nullptr || b == nullptr) return a == b ? 0 : a == nullptr ? -1 : 1;
  if (a->data == b->data) return 0;
  return std::lexicographical_compare(a->data.begin(), a->data.end(),
                                      b->data.begin(), b->data.end()) ? -1 : 1;
}

void ParcelAppendFrom(JNIEnv*, jclass, jlong destination, jlong source,
                      jint offset, jint length) {
  auto* to = Parcel(destination);
  auto* from = Parcel(source);
  if (to == nullptr || from == nullptr || offset < 0 || length < 0 ||
      static_cast<size_t>(offset) + static_cast<size_t>(length) > from->data.size()) return;
  to->data.insert(to->data.end(), from->data.begin() + offset,
                  from->data.begin() + offset + length);
  to->position = to->data.size();
}

void ParcelSignalException(JNIEnv*, jclass, jint) {}

jboolean ParcelHasBinders(JNIEnv*, jclass, jlong p) {
  return Parcel(p) != nullptr && !Parcel(p)->binders.empty() ? JNI_TRUE : JNI_FALSE;
}
jboolean ParcelHasBindersRange(JNIEnv* env, jclass cls, jlong p, jint, jint) {
  return ParcelHasBinders(env, cls, p);
}
jboolean ParcelHasFileDescriptors(jlong p) {
  return Parcel(p) != nullptr && !Parcel(p)->file_descriptors.empty()
             ? JNI_TRUE
             : JNI_FALSE;
}
jboolean ParcelHasFileDescriptorsRange(JNIEnv*, jclass, jlong p, jint, jint) {
  return ParcelHasFileDescriptors(p);
}
jboolean ParcelReplaceWorkSource(jlong, jint) { return JNI_FALSE; }
jint ParcelReadWorkSource(jlong) { return -1; }
jlong ParcelOpenAshmemSize(jlong) { return 0; }

struct DarwinBinderHolder {};

void BinderHolderFinalizer(void* holder) {
  delete static_cast<DarwinBinderHolder*>(holder);
}

jlong BinderGetNativeHolder(JNIEnv*, jclass) {
  return reinterpret_cast<std::uintptr_t>(new DarwinBinderHolder());
}

jlong BinderGetNativeFinalizer(JNIEnv*, jclass) {
  return reinterpret_cast<std::uintptr_t>(&BinderHolderFinalizer);
}

jint BinderGetCallingUid() {
  return darwin_art::binder::CurrentIdentity().uid;
}

jint BinderGetCallingPid() { return darwin_art::binder::CurrentIdentity().pid; }

jboolean BinderIsDirectlyHandlingTransactionNative() {
  return darwin_art::binder::CurrentIdentity().remote ? JNI_TRUE : JNI_FALSE;
}

// These identity operations are @CriticalNative in Android 16, so their
// callback ABI intentionally has no JNIEnv/jclass pair.
jlong BinderClearCallingIdentity() { return darwin_art::binder::ClearIdentity(); }
void BinderRestoreCallingIdentity(jlong token) { darwin_art::binder::RestoreIdentity(token); }
void BinderFlushPendingCommands() {}

thread_local jint g_binder_thread_strict_mode_policy = 0;

jint BinderGetThreadStrictModePolicy() {
  return g_binder_thread_strict_mode_policy;
}

void BinderSetThreadStrictModePolicy(jint policy) {
  g_binder_thread_strict_mode_policy = policy;
}

// Android's socket flag values differ from Darwin's libc constants.
constexpr int kAndroidMsgDontWait = 0x40;
constexpr int kAndroidMsgNoSignal = 0x4000;

struct DarwinInputChannelState {
  explicit DarwinInputChannelState(JNIEnv* env, std::string channel_name)
      : name(std::move(channel_name)) {
    jclass binder_class = env->FindClass("android/os/Binder");
    jmethodID binder_constructor =
        binder_class == nullptr
            ? nullptr
            : env->GetMethodID(binder_class, "<init>", "()V");
    jobject local_token = binder_constructor == nullptr
                              ? nullptr
                              : env->NewObject(binder_class,
                                               binder_constructor);
    if (local_token != nullptr) {
      connection_token = env->NewGlobalRef(local_token);
    }
    env->DeleteLocalRef(local_token);
    env->DeleteLocalRef(binder_class);
    OpenLocalWakePair();
  }

  DarwinInputChannelState(JNIEnv* env, std::string channel_name, jobject token,
                          int endpoint_fd)
      : name(std::move(channel_name)), remote_endpoint_fd(endpoint_fd) {
    if (token != nullptr) connection_token = env->NewGlobalRef(token);
    // AppKit events originate in the APK process even when WindowManager sent
    // this channel through Binder. Keep their process-local queue wakeup
    // separate from the transferred InputTransport endpoint.
    OpenLocalWakePair();
  }

  void OpenLocalWakePair() {
    int fds[2] = {-1, -1};
    // These are guest descriptors consumed by the broker-backed ALooper.
    // Host libc socketpair/fcntl would produce unrelated descriptor numbers
    // and makes the transport look invalid to the poll namespace.
    constexpr int kAndroidAfUnix = 1;
    constexpr int kAndroidSockStreamNonblockCloexec = 0x80801;
    if (darwin_art_bionic_socket_broker_socketpair(
            kAndroidAfUnix, kAndroidSockStreamNonblockCloexec, 0, fds) == 0) {
      read_fd = fds[0];
      write_fd = fds[1];
    }
  }

  ~DarwinInputChannelState() {
    if (read_fd >= 0) darwin_art_bionic_socket_broker_close(read_fd);
    if (write_fd >= 0 && write_fd != read_fd)
      darwin_art_bionic_socket_broker_close(write_fd);
    if (remote_endpoint_fd >= 0)
      darwin_art_bionic_socket_broker_close(remote_endpoint_fd);
    if (connection_token != nullptr && g_framework_vm != nullptr) {
      JNIEnv* env = nullptr;
      bool detach = false;
      jint status = g_framework_vm->GetEnv(reinterpret_cast<void**>(&env),
                                           JNI_VERSION_1_6);
      if (status == JNI_EDETACHED &&
          g_framework_vm->AttachCurrentThread(&env, nullptr) == JNI_OK) {
        detach = true;
        status = JNI_OK;
      }
      if (status == JNI_OK && env != nullptr) {
        env->DeleteGlobalRef(connection_token);
      }
      if (detach) g_framework_vm->DetachCurrentThread();
    }
  }

  std::string name;
  // Android's InputTransport creates one BBinder connection token for an
  // input-channel pair. Client/server wrappers and dup() all expose the same
  // object identity; a different pair receives a different token.
  jobject connection_token = nullptr;
  // Pending input belongs to the channel, not to the process. AppKit only
  // publishes this release/acquire bit after queueing a packet; the focused
  // InputEventReceiver reads it from Chromium's owner Looper.
  std::atomic<bool> pending_input{false};
  // Finish acknowledgements are channel state, not receiver-local state. The
  // client/server wrappers therefore observe one sequence result even while
  // payload migration is still in progress.
  std::atomic<jint> last_finished_sequence{0};
  std::atomic<bool> last_finished_handled{false};
  std::mutex packet_mutex;
  std::deque<darwin_art::DarwinArtInputPacket> packets;
  bool pointer_active = false;
  DarwinArtPointerEventV2 last_pointer{};
  int32_t input_left = 0;
  int32_t input_top = 0;
  int32_t input_right = 0;
  int32_t input_bottom = 0;
  bool input_geometry_valid = false;
  uint64_t focus_order = 0;
  struct FinishAck {
    jint sequence = 0;
    bool handled = false;
  };
  std::mutex finish_mutex;
  std::condition_variable finish_condition;
  std::deque<FinishAck> finish_acks;
  std::deque<jint> pending_finish_sequences;
  std::unordered_set<jint> pending_finish_set;
  uint64_t finish_ack_overflows = 0;
  std::atomic<bool> looper_consumer{false};
  std::shared_ptr<DarwinInputReceiver> consumer;
  int read_fd = -1;
  int write_fd = -1;
  // A parcel-imported endpoint is one full-duplex descriptor. It is not used
  // as a one-byte local wake pipe: framed payload/ACK decoding must own it.
  int remote_endpoint_fd = -1;
  std::vector<uint8_t> remote_rx;
  std::mutex remote_tx_mutex;
};

constexpr uint32_t kInputFrameMagic = 0x44414950;  // DAIP
constexpr uint32_t kInputFrameVersion = 1;
constexpr uint32_t kInputWindowFrameMagic = 0x44415747;  // DAWG
struct DarwinInputFrame {
  uint32_t magic = kInputFrameMagic;
  uint32_t version = kInputFrameVersion;
  uint32_t kind = 0;
  uint32_t payload_size = sizeof(darwin_art::DarwinArtInputPacket);
  darwin_art::DarwinArtInputPacket payload{};
};
static_assert(std::is_trivially_copyable_v<DarwinInputFrame>);

struct DarwinInputWindowFrame {
  uint32_t magic = kInputWindowFrameMagic;
  uint32_t version = kInputFrameVersion;
  int32_t left = 0;
  int32_t top = 0;
  int32_t right = 0;
  int32_t bottom = 0;
  uint32_t visible = 0;
  uint32_t reserved = 0;
};
static_assert(std::is_trivially_copyable_v<DarwinInputWindowFrame>);

bool IsValidInputPacket(const darwin_art::DarwinArtInputPacket& packet) {
  switch (packet.kind) {
    case darwin_art::DarwinArtInputPacketKind::kPointer:
      return packet.pointer.version == 2 &&
             packet.pointer.size >= sizeof(DarwinArtPointerEventV2) &&
             packet.pointer.action <= DARWIN_ART_POINTER_CANCEL &&
             packet.pointer.pointer_count > 0;
    case darwin_art::DarwinArtInputPacketKind::kKey:
      return packet.key.version == 1 &&
             packet.key.size >= sizeof(DarwinArtKeyEventV1) &&
             packet.key.action <= 1;
  }
  return false;
}

bool SendRemoteInputFrame(DarwinInputChannelState* channel,
                          const darwin_art::DarwinArtInputPacket& packet) {
  if (channel == nullptr || channel->remote_endpoint_fd < 0 ||
      !IsValidInputPacket(packet)) {
    return false;
  }
  DarwinInputFrame frame;
  frame.kind = static_cast<uint32_t>(packet.kind);
  frame.payload = packet;
  const auto* bytes = reinterpret_cast<const uint8_t*>(&frame);
  size_t offset = 0;
  // The imported endpoint is SOCK_STREAM. Serialize writers and finish a
  // frame before another producer may append one so partial writes cannot
  // interleave two input packets.
  std::lock_guard<std::mutex> lock(channel->remote_tx_mutex);
  while (offset < sizeof(frame)) {
    const intptr_t sent = darwin_art_bionic_socket_broker_send(
        channel->remote_endpoint_fd, bytes + offset, sizeof(frame) - offset,
        kAndroidMsgNoSignal);
    if (sent <= 0) return false;
    offset += static_cast<size_t>(sent);
  }
  return true;
}

size_t DecodeRemoteInputFramesLocked(DarwinInputChannelState* channel) {
  if (channel == nullptr) return 0;
  constexpr size_t kMaxPackets = 256;
  size_t consumed_bytes = 0;
  size_t decoded = 0;
  while (channel->remote_rx.size() - consumed_bytes >=
             sizeof(DarwinInputFrame) &&
         channel->packets.size() < kMaxPackets) {
    DarwinInputFrame frame{};
    std::memcpy(&frame, channel->remote_rx.data() + consumed_bytes,
                sizeof(frame));
    uint32_t leading_magic = 0;
    std::memcpy(&leading_magic, channel->remote_rx.data() + consumed_bytes,
                sizeof(leading_magic));
    if (leading_magic == 0x4441414b ||
        leading_magic == kInputWindowFrameMagic) {
      // Control frames may be interleaved with input frames on the
      // full-duplex stream. Leave them at the front for their decoder instead
      // of treating them as corrupt input and dropping subsequent bytes.
      break;
    }
    if (frame.magic != kInputFrameMagic ||
        frame.version != kInputFrameVersion ||
        frame.payload_size != sizeof(frame.payload) ||
        frame.kind != static_cast<uint32_t>(frame.payload.kind) ||
        !IsValidInputPacket(frame.payload)) {
      // A version or size mismatch cannot be skipped safely on a stream.
      // Drop the buffered transport rather than interpreting arbitrary bytes
      // as framework input.
      channel->remote_rx.clear();
      return decoded;
    }
    channel->packets.push_back(frame.payload);
    if (frame.payload.kind ==
        darwin_art::DarwinArtInputPacketKind::kPointer) {
      channel->last_pointer = frame.payload.pointer;
      if (frame.payload.pointer.action == DARWIN_ART_POINTER_DOWN) {
        channel->pointer_active = true;
      } else if (frame.payload.pointer.action == DARWIN_ART_POINTER_UP ||
                 frame.payload.pointer.action == DARWIN_ART_POINTER_CANCEL) {
        channel->pointer_active = false;
      }
    }
    consumed_bytes += sizeof(frame);
    ++decoded;
  }
  if (consumed_bytes > 0) {
    channel->remote_rx.erase(channel->remote_rx.begin(),
                             channel->remote_rx.begin() +
                                 static_cast<ptrdiff_t>(consumed_bytes));
    channel->pending_input.store(true, std::memory_order_release);
  }
  return decoded;
}
struct DarwinInputAckFrame {
  uint32_t magic = 0x4441414b;  // DAAK
  uint32_t version = kInputFrameVersion;
  uint32_t sequence = 0;
  uint32_t handled = 0;
};
static_assert(std::is_trivially_copyable_v<DarwinInputAckFrame>);

bool DecodeRemoteInputWindowFrameLocked(DarwinInputChannelState* channel) {
  if (channel == nullptr ||
      channel->remote_rx.size() < sizeof(DarwinInputWindowFrame)) {
    return false;
  }
  uint32_t magic = 0;
  std::memcpy(&magic, channel->remote_rx.data(), sizeof(magic));
  if (magic != kInputWindowFrameMagic) return false;
  DarwinInputWindowFrame frame{};
  std::memcpy(&frame, channel->remote_rx.data(), sizeof(frame));
  if (frame.version != kInputFrameVersion || frame.visible > 1) {
    channel->remote_rx.clear();
    return true;
  }
  const bool valid = frame.visible != 0 && frame.right > frame.left &&
                     frame.bottom > frame.top;
  channel->input_left = frame.left;
  channel->input_top = frame.top;
  channel->input_right = frame.right;
  channel->input_bottom = frame.bottom;
  channel->input_geometry_valid = valid;
  channel->remote_rx.erase(
      channel->remote_rx.begin(),
      channel->remote_rx.begin() +
          static_cast<ptrdiff_t>(sizeof(DarwinInputWindowFrame)));
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android InputWindow geometry name=" << channel->name
              << " visible=" << (valid ? 1 : 0) << " frame=[" << frame.left
              << "," << frame.top << "," << frame.right << ","
              << frame.bottom << "]\n";
  }
  return true;
}

bool SendRemoteAckFrame(DarwinInputChannelState* channel, jint sequence,
                        bool handled) {
  if (channel == nullptr || channel->remote_endpoint_fd < 0) return false;
  DarwinInputAckFrame frame;
  frame.sequence = static_cast<uint32_t>(sequence);
  frame.handled = handled ? 1u : 0u;
  const auto* bytes = reinterpret_cast<const uint8_t*>(&frame);
  std::lock_guard<std::mutex> lock(channel->remote_tx_mutex);
  size_t offset = 0;
  while (offset < sizeof(frame)) {
    const intptr_t sent = darwin_art_bionic_socket_broker_send(
        channel->remote_endpoint_fd, bytes + offset, sizeof(frame) - offset,
        kAndroidMsgNoSignal);
    if (sent <= 0) return false;
    offset += static_cast<size_t>(sent);
  }
  return true;
}

void DecodeRemoteAckFramesLocked(DarwinInputChannelState* channel) {
  if (channel == nullptr) return;
  while (channel->remote_rx.size() >= sizeof(DarwinInputAckFrame)) {
    uint32_t magic = 0;
    std::memcpy(&magic, channel->remote_rx.data(), sizeof(magic));
    if (magic != 0x4441414b) break;
    DarwinInputAckFrame frame{};
    std::memcpy(&frame, channel->remote_rx.data(), sizeof(frame));
    if (frame.version != kInputFrameVersion || frame.handled > 1) {
      channel->remote_rx.clear();
      return;
    }
    bool queued = false;
    {
      std::lock_guard<std::mutex> lock(channel->finish_mutex);
      const jint sequence = static_cast<jint>(frame.sequence);
      if (channel->pending_finish_set.contains(sequence)) {
        auto existing = std::find_if(
            channel->finish_acks.begin(), channel->finish_acks.end(),
            [sequence](const auto& ack) { return ack.sequence == sequence; });
        if (existing != channel->finish_acks.end()) {
          existing->handled = frame.handled != 0;
        } else {
          channel->finish_acks.push_back(DarwinInputChannelState::FinishAck{
              .sequence = sequence, .handled = frame.handled != 0});
        }
        queued = true;
      }
    }
    channel->remote_rx.erase(
        channel->remote_rx.begin(),
        channel->remote_rx.begin() + static_cast<ptrdiff_t>(sizeof(frame)));
    if (queued) channel->finish_condition.notify_all();
  }
}

std::mutex g_focused_input_channel_mutex;
std::weak_ptr<DarwinInputChannelState> g_focused_input_channel;
std::weak_ptr<DarwinInputChannelState> g_touch_input_channel;
int32_t g_touch_offset_x = 0;
int32_t g_touch_offset_y = 0;
std::atomic<uint64_t> g_next_input_focus_order{1};

// AOSP's InputDispatcher sends CANCEL when a focused window changes while a
// pointer stream is active. Keep that boundary in the channel itself so the
// event cannot be stranded behind the global focused-channel pointer.
void QueueFocusLossCancel(
    const std::shared_ptr<DarwinInputChannelState>& channel) {
  if (channel == nullptr) return;
  bool wake = false;
  {
    std::lock_guard<std::mutex> lock(channel->packet_mutex);
    channel->packets.clear();
    if (channel->pointer_active) {
      DarwinArtPointerEventV2 cancel = channel->last_pointer;
      cancel.action = DARWIN_ART_POINTER_CANCEL;
      cancel.sequence = cancel.sequence == std::numeric_limits<uint64_t>::max()
                            ? cancel.sequence
                            : cancel.sequence + 1;
      cancel.pressure = 0.0f;
      cancel.size_value = 0.0f;
      darwin_art::DarwinArtInputPacket packet;
      packet.kind = darwin_art::DarwinArtInputPacketKind::kPointer;
      packet.pointer = cancel;
      channel->packets.push_back(packet);
      channel->pointer_active = false;
      wake = true;
    }
    // A registered receiver owns the payload callback; otherwise the ART
    // owner loop remains the consumer for the queued CANCEL.
    const bool receiver_alive =
        channel->consumer != nullptr &&
        channel->consumer->transport_registered.load(std::memory_order_acquire) &&
        !channel->consumer->disposed.load(std::memory_order_acquire);
    channel->looper_consumer.store(receiver_alive, std::memory_order_release);
    if (channel->packets.empty() && channel->read_fd >= 0) {
      uint8_t buffer[64];
      while (darwin_art_bionic_socket_broker_recv(
                 channel->read_fd, buffer, sizeof(buffer),
                 kAndroidMsgDontWait) > 0) {
      }
    }
    channel->pending_input.store(wake, std::memory_order_release);
  }
  if (wake && channel->write_fd >= 0) {
    const uint8_t token = 1;
    (void)darwin_art_bionic_socket_broker_send(
        channel->write_fd, &token, sizeof(token),
        kAndroidMsgDontWait | kAndroidMsgNoSignal);
  }
}

void SetFocusedInputChannel(
    const std::shared_ptr<DarwinInputChannelState>& channel) {
  std::shared_ptr<DarwinInputChannelState> previous;
  {
    std::lock_guard<std::mutex> lock(g_focused_input_channel_mutex);
    previous = g_focused_input_channel.lock();
    g_focused_input_channel = channel;
    if (channel != nullptr) {
      std::lock_guard<std::mutex> packet_lock(channel->packet_mutex);
      channel->focus_order =
          g_next_input_focus_order.fetch_add(1, std::memory_order_relaxed);
    }
  }
  if (previous != nullptr && previous != channel) {
    QueueFocusLossCancel(previous);
  }
}

void UpdateInputWindowGeometry(JNIEnv* env, DarwinInputReceiver* receiver) {
  if (env == nullptr || receiver == nullptr || receiver->view_root == nullptr ||
      receiver->channel == nullptr) {
    return;
  }
  jclass root_class = env->GetObjectClass(receiver->view_root);
  jfieldID frame_field =
      root_class == nullptr
          ? nullptr
          : env->GetFieldID(root_class, "mWinFrame", "Landroid/graphics/Rect;");
  jobject frame = frame_field == nullptr
                      ? nullptr
                      : env->GetObjectField(receiver->view_root, frame_field);
  jclass rect_class = frame == nullptr ? nullptr : env->GetObjectClass(frame);
  jfieldID left_field = rect_class == nullptr
                            ? nullptr
                            : env->GetFieldID(rect_class, "left", "I");
  jfieldID top_field = rect_class == nullptr
                           ? nullptr
                           : env->GetFieldID(rect_class, "top", "I");
  jfieldID right_field = rect_class == nullptr
                             ? nullptr
                             : env->GetFieldID(rect_class, "right", "I");
  jfieldID bottom_field = rect_class == nullptr
                              ? nullptr
                              : env->GetFieldID(rect_class, "bottom", "I");
  if (left_field != nullptr && top_field != nullptr && right_field != nullptr &&
      bottom_field != nullptr && !env->ExceptionCheck()) {
    const int32_t left = env->GetIntField(frame, left_field);
    const int32_t top = env->GetIntField(frame, top_field);
    const int32_t right = env->GetIntField(frame, right_field);
    const int32_t bottom = env->GetIntField(frame, bottom_field);
    std::lock_guard<std::mutex> lock(receiver->channel->packet_mutex);
    receiver->channel->input_left = left;
    receiver->channel->input_top = top;
    receiver->channel->input_right = right;
    receiver->channel->input_bottom = bottom;
    receiver->channel->input_geometry_valid = right > left && bottom > top;
  }
  if (env->ExceptionCheck()) env->ExceptionClear();
  if (rect_class != nullptr) env->DeleteLocalRef(rect_class);
  if (frame != nullptr) env->DeleteLocalRef(frame);
  if (root_class != nullptr) env->DeleteLocalRef(root_class);
}

void ClearFocusedInputChannel(
    const std::shared_ptr<DarwinInputChannelState>& channel) {
  std::shared_ptr<DarwinInputChannelState> focused;
  {
    std::lock_guard<std::mutex> lock(g_focused_input_channel_mutex);
    focused = g_focused_input_channel.lock();
    if (focused == nullptr || focused == channel) g_focused_input_channel.reset();
    const auto touch = g_touch_input_channel.lock();
    if (touch == nullptr || touch == channel) {
      g_touch_input_channel.reset();
      g_touch_offset_x = 0;
      g_touch_offset_y = 0;
    }
  }
  if (focused != nullptr && focused == channel) {
    QueueFocusLossCancel(focused);
  }
}

void NotifyFocusedInputChannel() {
  std::shared_ptr<DarwinInputChannelState> channel;
  {
    std::lock_guard<std::mutex> lock(g_focused_input_channel_mutex);
    channel = g_focused_input_channel.lock();
  }
  if (channel != nullptr) {
    channel->pending_input.store(true, std::memory_order_release);
    if (channel->write_fd >= 0) {
      const uint8_t token = 1;
      (void)darwin_art_bionic_socket_broker_send(
          channel->write_fd, &token, sizeof(token),
          kAndroidMsgDontWait | kAndroidMsgNoSignal);
    }
  }
}

darwin_art::DarwinArtInputEnqueueResult EnqueueFocusedPacket(
    const std::shared_ptr<DarwinInputChannelState>& channel,
    const darwin_art::DarwinArtInputPacket& packet) {
  if (channel == nullptr) {
    return darwin_art::DarwinArtInputEnqueueResult::kNoFocusedChannel;
  }
  bool has_local_receiver = false;
  {
    std::lock_guard<std::mutex> lock(channel->packet_mutex);
    has_local_receiver =
        channel->consumer != nullptr &&
        !channel->consumer->disposed.load(std::memory_order_acquire) &&
        channel->looper_consumer.load(std::memory_order_acquire);
  }
  // A parcel-imported client retains the full-duplex endpoint used by the
  // system InputDispatcher. AppKit is the platform InputReader boundary for
  // this process, so an attached ViewRoot receiver consumes through its local
  // bounded queue and ALooper wakeup. Server-only/import-only endpoints still
  // use the remote transport.
  if (channel->remote_endpoint_fd >= 0 && !has_local_receiver) {
    return SendRemoteInputFrame(channel.get(), packet)
               ? darwin_art::DarwinArtInputEnqueueResult::kQueued
               : darwin_art::DarwinArtInputEnqueueResult::kBackpressured;
  }
  {
    std::lock_guard<std::mutex> lock(channel->packet_mutex);
    constexpr size_t kMaxPackets = 256;
    if (channel->packets.size() >= kMaxPackets) {
      // Pointer MOVE packets are latest-wins at the AppKit boundary. If a
      // burst still fills the channel queue, replace only its newest MOVE;
      // gesture boundaries and key packets are never discarded.
      if (packet.kind == darwin_art::DarwinArtInputPacketKind::kPointer) {
        if (packet.pointer.action == DARWIN_ART_POINTER_MOVE &&
            !channel->packets.empty() &&
            channel->packets.back().kind ==
                darwin_art::DarwinArtInputPacketKind::kPointer &&
            channel->packets.back().pointer.action ==
                DARWIN_ART_POINTER_MOVE) {
          channel->packets.back() = packet;
          channel->last_pointer = packet.pointer;
          channel->pending_input.store(true, std::memory_order_release);
          return darwin_art::DarwinArtInputEnqueueResult::kQueued;
        }
      }
      return darwin_art::DarwinArtInputEnqueueResult::kBackpressured;
    }
    channel->packets.push_back(packet);
    if (packet.kind == darwin_art::DarwinArtInputPacketKind::kPointer) {
      channel->last_pointer = packet.pointer;
      if (packet.pointer.action == DARWIN_ART_POINTER_DOWN) {
        channel->pointer_active = true;
      } else if (packet.pointer.action == DARWIN_ART_POINTER_UP ||
                 packet.pointer.action == DARWIN_ART_POINTER_CANCEL) {
        channel->pointer_active = false;
      }
    }
  }
  channel->pending_input.store(true, std::memory_order_release);
  if (channel->write_fd >= 0) {
    const uint8_t token = 1;
    const intptr_t sent = darwin_art_bionic_socket_broker_send(
        channel->write_fd, &token, sizeof(token),
        kAndroidMsgDontWait | kAndroidMsgNoSignal);
    if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
      std::cerr << "ART Android InputChannel wake write_fd="
                << channel->write_fd << " read_fd=" << channel->read_fd
                << " sent=" << sent << " errno=" << errno << "\n";
    }
  }
  return darwin_art::DarwinArtInputEnqueueResult::kQueued;
}

bool DequeueChannelPacket(DarwinInputChannelState* channel,
                          darwin_art::DarwinArtInputPacket* packet,
                          darwin_art::DarwinArtInputPacketKind kind) {
  if (channel == nullptr || packet == nullptr) return false;
  std::lock_guard<std::mutex> lock(channel->packet_mutex);
  if (channel->packets.empty() || channel->packets.front().kind != kind) {
    return false;
  }
  *packet = channel->packets.front();
  channel->packets.pop_front();
  return true;
}

bool DequeueChannelPacketAny(DarwinInputChannelState* channel,
                             darwin_art::DarwinArtInputPacket* packet) {
  if (channel == nullptr || packet == nullptr) return false;
  std::lock_guard<std::mutex> lock(channel->packet_mutex);
  if (channel->packets.empty()) return false;
  *packet = channel->packets.front();
  channel->packets.pop_front();
  return true;
}

bool DequeueFocusedPacket(darwin_art::DarwinArtInputPacket* packet,
                          darwin_art::DarwinArtInputPacketKind kind) {
  if (packet == nullptr) return false;
  std::shared_ptr<DarwinInputChannelState> channel;
  {
    std::lock_guard<std::mutex> lock(g_focused_input_channel_mutex);
    channel = g_focused_input_channel.lock();
  }
  if (channel == nullptr) return false;
  if (channel->looper_consumer.load(std::memory_order_acquire)) return false;
  return DequeueChannelPacket(channel.get(), packet, kind);
}

bool FocusedChannelHasPackets() {
  std::shared_ptr<DarwinInputChannelState> channel;
  {
    std::lock_guard<std::mutex> lock(g_focused_input_channel_mutex);
    channel = g_focused_input_channel.lock();
  }
  if (channel == nullptr) return false;
  std::lock_guard<std::mutex> lock(channel->packet_mutex);
  return !channel->packets.empty();
}

void ClearFocusedInputChannelPending() {
  std::shared_ptr<DarwinInputChannelState> channel;
  {
    std::lock_guard<std::mutex> lock(g_focused_input_channel_mutex);
    channel = g_focused_input_channel.lock();
  }
  if (channel != nullptr) {
    std::lock_guard<std::mutex> packet_lock(channel->packet_mutex);
    if (!channel->packets.empty()) {
      return;
    }
    if (channel->read_fd >= 0) {
      uint8_t buffer[64];
      while (darwin_art_bionic_socket_broker_recv(
                 channel->read_fd, buffer, sizeof(buffer),
                 kAndroidMsgDontWait) > 0) {
      }
    }
    channel->pending_input.store(false, std::memory_order_release);
  }
}

void ClearInputChannelPending(DarwinInputChannelState* channel) {
  if (channel == nullptr) return;
  std::lock_guard<std::mutex> packet_lock(channel->packet_mutex);
  if (!channel->packets.empty()) return;
  if (channel->read_fd >= 0) {
    uint8_t buffer[64];
    while (darwin_art_bionic_socket_broker_recv(
               channel->read_fd, buffer, sizeof(buffer),
               kAndroidMsgDontWait) > 0) {
    }
  }
  channel->pending_input.store(false, std::memory_order_release);
}

// ALooper callbacks are bounded so a burst cannot monopolize the ART owner.
// If that bound is reached, the transport token must be re-armed after the
// callback drains its current token; otherwise packets left in the channel
// remain invisible until an unrelated future enqueue happens to send another
// token. The queue itself remains the sole source of payload ordering.
bool RearmInputChannelTransport(DarwinInputChannelState* channel) {
  if (channel == nullptr) return false;
  bool pending = false;
  {
    std::lock_guard<std::mutex> lock(channel->packet_mutex);
    pending = !channel->packets.empty();
    if (pending) channel->pending_input.store(true, std::memory_order_release);
  }
  if (!pending || channel->write_fd < 0) return false;
  const uint8_t token = 1;
  return darwin_art_bionic_socket_broker_send(
             channel->write_fd, &token, sizeof(token),
             kAndroidMsgDontWait | kAndroidMsgNoSignal) ==
         static_cast<intptr_t>(sizeof(token));
}

struct DarwinInputChannel {
  std::shared_ptr<DarwinInputChannelState> state;
  bool server = false;
  bool disposed = false;
};

std::mutex g_input_channel_registry_mutex;
std::vector<std::weak_ptr<DarwinInputChannelState>> g_input_channel_registry;

void RegisterInputChannelState(
    const std::shared_ptr<DarwinInputChannelState>& state) {
  std::lock_guard<std::mutex> lock(g_input_channel_registry_mutex);
  std::erase_if(g_input_channel_registry,
                [](const auto& candidate) { return candidate.expired(); });
  g_input_channel_registry.emplace_back(state);
}

std::shared_ptr<DarwinInputChannelState> FindInputChannelState(
    JNIEnv* env, jobject token) {
  if (env == nullptr || token == nullptr) return nullptr;
  std::lock_guard<std::mutex> lock(g_input_channel_registry_mutex);
  std::shared_ptr<DarwinInputChannelState> result;
  std::erase_if(g_input_channel_registry, [&](const auto& candidate) {
    const auto state = candidate.lock();
    if (state == nullptr) return true;
    if (result == nullptr && state->connection_token != nullptr &&
        env->IsSameObject(state->connection_token, token) == JNI_TRUE) {
      result = state;
    }
    return false;
  });
  return result;
}

void InputChannelFinalizer(void* pointer) {
  delete static_cast<DarwinInputChannel*>(pointer);
}

jlong InputChannelGetFinalizer(JNIEnv*, jclass) {
  return reinterpret_cast<std::uintptr_t>(&InputChannelFinalizer);
}

DarwinInputChannel* InputChannel(jlong pointer) {
  return reinterpret_cast<DarwinInputChannel*>(
      static_cast<std::uintptr_t>(pointer));
}

void InputChannelDispose(JNIEnv*, jclass, jlong pointer) {
  if (auto* channel = InputChannel(pointer); channel != nullptr) {
    channel->disposed = true;
    // Match NativeInputChannel::dispose(): release the underlying transport
    // immediately while retaining the tiny native wrapper for the registered
    // finalizer.  Delaying the shared state until finalization can make its
    // Binder global reference outlive the JavaVM during process shutdown.
    channel->state.reset();
  }
}

jlong InputChannelDup(JNIEnv*, jobject, jlong pointer) {
  const auto* channel = InputChannel(pointer);
  if (channel == nullptr || channel->disposed || channel->state == nullptr) {
    return 0;
  }
  auto* duplicate = new (std::nothrow)
      DarwinInputChannel{channel->state, channel->server, false};
  return static_cast<jlong>(
      reinterpret_cast<std::uintptr_t>(duplicate));
}

jstring InputChannelGetName(JNIEnv* env, jobject, jlong pointer) {
  const auto* channel = InputChannel(pointer);
  return channel == nullptr || channel->state == nullptr
             ? nullptr
             : env->NewStringUTF(channel->state->name.c_str());
}

jobject InputChannelGetToken(JNIEnv* env, jobject, jlong pointer) {
  const auto* channel = InputChannel(pointer);
  return channel == nullptr || channel->state == nullptr ||
                 channel->state->connection_token == nullptr
             ? nullptr
             : env->NewLocalRef(channel->state->connection_token);
}

jlongArray InputChannelOpenPair(JNIEnv* env, jclass, jstring name) {
  const char* utf = name == nullptr ? nullptr : env->GetStringUTFChars(name, nullptr);
  auto state = std::make_shared<DarwinInputChannelState>(
      env, utf == nullptr ? "darwin-art-input" : utf);
  if (utf != nullptr) env->ReleaseStringUTFChars(name, utf);
  if (state->connection_token == nullptr || env->ExceptionCheck()) {
    return nullptr;
  }
  RegisterInputChannelState(state);
  auto* client = new (std::nothrow) DarwinInputChannel{state, false, false};
  auto* server = new (std::nothrow) DarwinInputChannel{state, true, false};
  if (client == nullptr || server == nullptr) {
    delete client;
    delete server;
    return nullptr;
  }
  jlong values[] = {
      static_cast<jlong>(reinterpret_cast<std::uintptr_t>(client)),
      static_cast<jlong>(reinterpret_cast<std::uintptr_t>(server)),
  };
  jlongArray result = env->NewLongArray(2);
  if (result != nullptr) env->SetLongArrayRegion(result, 0, 2, values);
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android InputChannel open-pair name=" << state->name
              << " read_fd=" << state->read_fd
              << " write_fd=" << state->write_fd << "\n";
  }
  return result;
}

jlong InputChannelReadParcel(JNIEnv* env, jobject, jobject parcel_object) {
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
  android::Parcel* parcel = android::parcelForJavaObject(env, parcel_object);
  if (parcel == nullptr) return 0;
  const jint initialized = parcel->readInt32();
  if (initialized != 1) return 0;
  jobject token = android::javaObjectForIBinder(env, parcel->readStrongBinder());
  const android::String8 name_utf8(parcel->readString16());
  jstring name = env->NewStringUTF(name_utf8.c_str());
  const int borrowed_endpoint_fd = parcel->readFileDescriptor();
  const int endpoint_fd =
      borrowed_endpoint_fd < 0
          ? -1
          : darwin_art_bionic_socket_broker_dup(borrowed_endpoint_fd);
#else
  DarwinParcel* parcel = JavaParcel(env, parcel_object);
  if (parcel == nullptr) return 0;
  const jlong parcel_pointer = reinterpret_cast<jlong>(parcel);
  // android_view_InputChannel writes this wrapper marker before the
  // InputChannelCore tuple. No core fields follow an uninitialized channel.
  const jint initialized = ParcelReadInt(parcel_pointer);
  if (initialized == 0) return 0;
  if (initialized != 1) return 0;
  // Match android::InputChannel::readFromParcel: strong connection token,
  // UTF-16 name, then one unique full-duplex endpoint descriptor.
  jobject token = ParcelReadStrongBinder(env, nullptr, parcel_pointer);
  jstring name = ParcelReadString(env, nullptr, parcel_pointer);
  const int endpoint_fd = ParcelReadGuestFileDescriptor(parcel_pointer);
#endif
  if (name == nullptr || endpoint_fd < 0) {
    env->DeleteLocalRef(token);
    env->DeleteLocalRef(name);
    if (endpoint_fd >= 0) darwin_art_bionic_socket_broker_close(endpoint_fd);
    return 0;
  }
  const char* utf = env->GetStringUTFChars(name, nullptr);
  const std::string channel_name = utf == nullptr ? "" : utf;
  if (utf != nullptr) env->ReleaseStringUTFChars(name, utf);
  std::shared_ptr<DarwinInputChannelState> state =
      FindInputChannelState(env, token);
  if (state != nullptr && state->name != channel_name) state.reset();
  bool adopted_endpoint = false;
  if (state == nullptr && token != nullptr && !channel_name.empty() &&
      !env->ExceptionCheck()) {
    state = std::make_shared<DarwinInputChannelState>(
        env, channel_name, token, endpoint_fd);
    adopted_endpoint = true;
    if (state->connection_token == nullptr || state->read_fd < 0 ||
        state->write_fd < 0 || env->ExceptionCheck()) {
      state.reset();
    } else {
      RegisterInputChannelState(state);
    }
  } else if (state != nullptr) {
    // Same-process state already owns its endpoint pair; consume the parcel's
    // duplicated descriptor without changing the live pair identity.
    darwin_art_bionic_socket_broker_close(endpoint_fd);
  }
  env->DeleteLocalRef(token);
  env->DeleteLocalRef(name);
  if (state == nullptr || env->ExceptionCheck()) {
    if (!adopted_endpoint && endpoint_fd >= 0)
      darwin_art_bionic_socket_broker_close(endpoint_fd);
    return 0;
  }
  auto* channel = new (std::nothrow)
      DarwinInputChannel{state, false, false};
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android InputChannel parcel-import name=" << state->name
              << " endpoint_fd=" << state->remote_endpoint_fd
              << " local_read_fd=" << state->read_fd << "\n";
  }
  return static_cast<jlong>(reinterpret_cast<std::uintptr_t>(channel));
}

void InputChannelWriteParcel(JNIEnv* env, jobject, jobject parcel_object,
                             jlong pointer) {
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
  android::Parcel* parcel = android::parcelForJavaObject(env, parcel_object);
#else
  DarwinParcel* parcel = JavaParcel(env, parcel_object);
#endif
  if (parcel == nullptr) return;
  const auto* channel = InputChannel(pointer);
  if (channel == nullptr || channel->disposed || channel->state == nullptr ||
      channel->state->connection_token == nullptr) {
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
    (void)parcel->writeInt32(0);
#else
    const jlong parcel_pointer = reinterpret_cast<jlong>(parcel);
    ParcelWriteInt(parcel_pointer, 0);
#endif
    return;
  }
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
  android::status_t status = parcel->writeInt32(1);
  if (status == android::OK) {
    status = parcel->writeStrongBinder(
        android::ibinderForJavaObject(env,
                                     channel->state->connection_token));
  }
  if (status == android::OK) {
    status = parcel->writeString16(
        android::String16(channel->state->name.c_str()));
  }
#else
  const jlong parcel_pointer = reinterpret_cast<jlong>(parcel);
  ParcelWriteInt(parcel_pointer, 1);
  ParcelWriteStrongBinder(env, nullptr, parcel_pointer,
                          channel->state->connection_token);
  jstring name = env->NewStringUTF(channel->state->name.c_str());
  ParcelWriteString(env, nullptr, parcel_pointer, name);
#endif
  const int endpoint_fd =
      channel->state->remote_endpoint_fd >= 0
          ? channel->state->remote_endpoint_fd
          : (channel->server ? channel->state->read_fd
                             : channel->state->write_fd);
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
  if (status == android::OK) {
    status = parcel->writeDupFileDescriptor(endpoint_fd);
  }
  if (status != android::OK && !env->ExceptionCheck()) {
    jclass exception = env->FindClass("java/lang/IllegalStateException");
    if (exception != nullptr) {
      env->ThrowNew(exception, "Unable to write framework InputChannel");
    }
    env->DeleteLocalRef(exception);
  }
#else
  (void)ParcelWriteGuestFileDescriptor(parcel_pointer, endpoint_fd);
  env->DeleteLocalRef(name);
#endif
}

void InputWindowPublish(JNIEnv* env, jclass, jobject input_channel, jint left,
                        jint top, jint right, jint bottom, jboolean visible) {
  if (env == nullptr || input_channel == nullptr) return;
  jclass channel_class = env->GetObjectClass(input_channel);
  jfieldID pointer_field =
      channel_class == nullptr ? nullptr
                               : env->GetFieldID(channel_class, "mPtr", "J");
  const jlong pointer = pointer_field == nullptr
                            ? 0
                            : env->GetLongField(input_channel, pointer_field);
  if (channel_class != nullptr) env->DeleteLocalRef(channel_class);
  auto* channel = InputChannel(pointer);
  if (channel == nullptr || channel->disposed || !channel->server ||
      channel->state == nullptr || channel->state->read_fd < 0 ||
      env->ExceptionCheck()) {
    if (env->ExceptionCheck()) env->ExceptionClear();
    return;
  }
  DarwinInputWindowFrame frame;
  frame.left = left;
  frame.top = top;
  frame.right = right;
  frame.bottom = bottom;
  frame.visible = visible == JNI_TRUE ? 1u : 0u;
  const auto* bytes = reinterpret_cast<const uint8_t*>(&frame);
  size_t offset = 0;
  std::lock_guard<std::mutex> lock(channel->state->remote_tx_mutex);
  while (offset < sizeof(frame)) {
    const intptr_t sent = darwin_art_bionic_socket_broker_send(
        channel->state->read_fd, bytes + offset, sizeof(frame) - offset,
        kAndroidMsgNoSignal);
    if (sent <= 0) break;
    offset += static_cast<size_t>(sent);
  }
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android WMS InputWindow publish name="
              << channel->state->name << " visible="
              << (frame.visible != 0 ? 1 : 0) << " frame=[" << left << ","
              << top << "," << right << "," << bottom << "] sent="
              << offset << "/" << sizeof(frame) << "\n";
  }
}

// Build the same framework InputEvent objects used by the owner dispatch path.
// The callback supplies the stored ViewRoot so focus, touch-mode, and finish
// stages remain in ViewRootImpl rather than calling the receiver directly.
jobject CreateChannelMotionEvent(JNIEnv* env,
                                 const DarwinArtPointerEventV2& packet) {
  if (env == nullptr) return nullptr;
  jclass motion = env->FindClass("android/view/MotionEvent");
  jclass properties = env->FindClass("android/view/MotionEvent$PointerProperties");
  jclass coords = env->FindClass("android/view/MotionEvent$PointerCoords");
  jmethodID properties_init = properties == nullptr
                                  ? nullptr
                                  : env->GetMethodID(properties, "<init>", "()V");
  jmethodID coords_init = coords == nullptr
                              ? nullptr
                              : env->GetMethodID(coords, "<init>", "()V");
  jfieldID id_field = properties == nullptr
                          ? nullptr
                          : env->GetFieldID(properties, "id", "I");
  jfieldID tool_field = properties == nullptr
                            ? nullptr
                            : env->GetFieldID(properties, "toolType", "I");
  jfieldID x_field = coords == nullptr ? nullptr : env->GetFieldID(coords, "x", "F");
  jfieldID y_field = coords == nullptr ? nullptr : env->GetFieldID(coords, "y", "F");
  jfieldID pressure_field =
      coords == nullptr ? nullptr : env->GetFieldID(coords, "pressure", "F");
  jfieldID size_field = coords == nullptr ? nullptr : env->GetFieldID(coords, "size", "F");
  jmethodID obtain = motion == nullptr
                        ? nullptr
                        : env->GetStaticMethodID(
                              motion, "obtain",
                              "(JJII[Landroid/view/MotionEvent$PointerProperties;"
                              "[Landroid/view/MotionEvent$PointerCoords;IIFFIIIII)"
                              "Landroid/view/MotionEvent;");
  if (obtain == nullptr || properties_init == nullptr || coords_init == nullptr ||
      id_field == nullptr || tool_field == nullptr || x_field == nullptr ||
      y_field == nullptr || pressure_field == nullptr || size_field == nullptr ||
      env->ExceptionCheck()) {
    env->ExceptionClear();
    if (coords != nullptr) env->DeleteLocalRef(coords);
    if (properties != nullptr) env->DeleteLocalRef(properties);
    if (motion != nullptr) env->DeleteLocalRef(motion);
    return nullptr;
  }
  jobject pointer = env->NewObject(properties, properties_init);
  jobject point = env->NewObject(coords, coords_init);
  jobjectArray pointer_array = env->NewObjectArray(1, properties, nullptr);
  jobjectArray coords_array = env->NewObjectArray(1, coords, nullptr);
  if (pointer == nullptr || point == nullptr || pointer_array == nullptr ||
      coords_array == nullptr || env->ExceptionCheck()) {
    env->ExceptionClear();
    if (coords_array != nullptr) env->DeleteLocalRef(coords_array);
    if (pointer_array != nullptr) env->DeleteLocalRef(pointer_array);
    if (point != nullptr) env->DeleteLocalRef(point);
    if (pointer != nullptr) env->DeleteLocalRef(pointer);
    env->DeleteLocalRef(coords);
    env->DeleteLocalRef(properties);
    env->DeleteLocalRef(motion);
    return nullptr;
  }
  env->SetIntField(pointer, id_field, 0);
  env->SetIntField(pointer, tool_field, 1);
  env->SetFloatField(point, x_field, packet.x);
  env->SetFloatField(point, y_field, packet.y);
  env->SetFloatField(point, pressure_field,
                     packet.action == DARWIN_ART_POINTER_UP ||
                             packet.action == DARWIN_ART_POINTER_CANCEL
                         ? 0.0f
                         : 1.0f);
  env->SetFloatField(point, size_field, 1.0f);
  env->SetObjectArrayElement(pointer_array, 0, pointer);
  env->SetObjectArrayElement(coords_array, 0, point);
  const uint64_t event_nanos = packet.event_time_nanos;
  const uint64_t down_nanos = packet.down_time_nanos == 0
                                  ? event_nanos
                                  : packet.down_time_nanos;
  jobject event = env->CallStaticObjectMethod(
      motion, obtain, static_cast<jlong>(down_nanos / 1000000ULL),
      static_cast<jlong>(event_nanos / 1000000ULL),
      static_cast<jint>(packet.action), 1, pointer_array, coords_array, 0, 0,
      // deviceId, edgeFlags, source, displayId, flags. SOURCE_TOUCHSCREEN
      // belongs to source, not edgeFlags; source=0 bypasses touch dispatch.
      1.0f, 1.0f, 0, 0, 0x1002, 0, 0);
  if (env->ExceptionCheck()) {
    env->ExceptionClear();
    env->DeleteLocalRef(event);
    event = nullptr;
  }
  env->DeleteLocalRef(coords_array);
  env->DeleteLocalRef(pointer_array);
  env->DeleteLocalRef(point);
  env->DeleteLocalRef(pointer);
  env->DeleteLocalRef(coords);
  env->DeleteLocalRef(properties);
  env->DeleteLocalRef(motion);
  return event;
}

jobject CreateChannelKeyEvent(JNIEnv* env, const DarwinArtKeyEventV1& packet) {
  if (env == nullptr) return nullptr;
  jclass key = env->FindClass("android/view/KeyEvent");
  jmethodID init = key == nullptr
                       ? nullptr
                       : env->GetMethodID(key, "<init>", "(JJIIIIIIII)V");
  if (init == nullptr || env->ExceptionCheck()) {
    env->ExceptionClear();
    if (key != nullptr) env->DeleteLocalRef(key);
    return nullptr;
  }
  const uint64_t event_nanos = packet.event_time_nanos;
  const uint64_t down_nanos = packet.down_time_nanos == 0
                                  ? event_nanos
                                  : packet.down_time_nanos;
  jobject event = env->NewObject(
      key, init, static_cast<jlong>(down_nanos / 1000000ULL),
      static_cast<jlong>(event_nanos / 1000000ULL),
      static_cast<jint>(packet.action), static_cast<jint>(packet.key_code),
      static_cast<jint>(packet.repeat_count), static_cast<jint>(packet.meta_state),
      static_cast<jint>(packet.device_id), static_cast<jint>(packet.scan_code),
      static_cast<jint>(packet.flags), static_cast<jint>(packet.source));
  if (env->ExceptionCheck()) {
    env->ExceptionClear();
    env->DeleteLocalRef(event);
    event = nullptr;
  }
  env->DeleteLocalRef(key);
  return event;
}

bool DispatchChannelPacket(JNIEnv* env, DarwinInputReceiver* receiver,
                           const darwin_art::DarwinArtInputPacket& packet) {
  if (env == nullptr || receiver == nullptr || receiver->view_root == nullptr)
    return false;
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    const uint64_t event_time =
        packet.kind == darwin_art::DarwinArtInputPacketKind::kPointer
            ? packet.pointer.event_time_nanos
            : packet.key.event_time_nanos;
    const uint64_t now =
        static_cast<uint64_t>(std::max<int64_t>(0, darwin_art::AndroidUptimeNanos()));
    if (event_time != 0 && now >= event_time) {
      const uint64_t sequence =
          packet.kind == darwin_art::DarwinArtInputPacketKind::kPointer
              ? packet.pointer.sequence
              : packet.key.sequence;
      std::cerr << "ART Android InputChannel ingress->dispatch kind="
                << (packet.kind == darwin_art::DarwinArtInputPacketKind::kPointer
                        ? "pointer"
                        : "key")
                << " sequence=" << sequence
                << " age_us=" << ((now - event_time) / 1000) << "\n";
    }
  }
  jobject event = packet.kind == darwin_art::DarwinArtInputPacketKind::kPointer
                      ? CreateChannelMotionEvent(env, packet.pointer)
                      : CreateChannelKeyEvent(env, packet.key);
  if (event == nullptr) return false;
  bool handled = false;
  const bool delivered = darwin_art::DispatchFrameworkInputEvent(
      env, receiver->view_root, event, &handled);
  if (packet.kind == darwin_art::DarwinArtInputPacketKind::kPointer) {
    jclass motion = env->FindClass("android/view/MotionEvent");
    jmethodID recycle = motion == nullptr
                            ? nullptr
                            : env->GetMethodID(motion, "recycle", "()V");
    if (recycle != nullptr && !env->ExceptionCheck())
      env->CallVoidMethod(event, recycle);
    if (env->ExceptionCheck()) env->ExceptionClear();
    if (motion != nullptr) env->DeleteLocalRef(motion);
  }
  env->DeleteLocalRef(event);
  return delivered;
}

void CleanupReceiverRefs(JNIEnv* env, DarwinInputReceiver* receiver) {
  if (env == nullptr || receiver == nullptr) return;
  bool expected = false;
  if (!receiver->refs_cleaned.compare_exchange_strong(
          expected, true, std::memory_order_acq_rel)) {
    return;
  }
  if (receiver->weak_receiver != nullptr) {
    env->DeleteGlobalRef(receiver->weak_receiver);
    receiver->weak_receiver = nullptr;
  }
  if (receiver->view_root != nullptr) {
    env->DeleteGlobalRef(receiver->view_root);
    receiver->view_root = nullptr;
  }
}

int InputChannelTransportCallback(int fd, int events, void* data) {
  auto* channel = static_cast<DarwinInputChannelState*>(data);
  if (channel == nullptr || fd < 0) return 0;
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android InputChannel callback-enter fd=" << fd
              << " events=0x" << std::hex << events << std::dec
              << " read_fd=" << channel->read_fd
              << " remote_fd=" << channel->remote_endpoint_fd << "\n";
  }
  // A closed peer makes poll(2) report HUP/ERR forever on a registered fd.
  // Remove the receiver at that boundary instead of returning 1 and waking
  // the ART owner in a tight callback loop with no payload.
  constexpr int kLooperEventError = 0x0004;
  constexpr int kLooperEventHangup = 0x0008;
  constexpr int kLooperEventInvalid = 0x0010;
  if ((events & (kLooperEventError | kLooperEventHangup |
                kLooperEventInvalid)) != 0) {
    std::lock_guard<std::mutex> lock(channel->packet_mutex);
    if (channel->consumer != nullptr) {
      if (fd == channel->remote_endpoint_fd) {
        channel->consumer->remote_transport_registered.store(
            false, std::memory_order_release);
      } else {
        channel->consumer->transport_registered.store(
            false, std::memory_order_release);
      }
    }
    // Keep this state transition in the same critical section as the
    // transport flag. QueueFocusLossCancel must not resurrect a registration
    // after this callback has observed a terminal broker error.
    const bool has_transport =
        channel->consumer != nullptr &&
        (channel->consumer->transport_registered.load(
             std::memory_order_acquire) ||
         channel->consumer->remote_transport_registered.load(
             std::memory_order_acquire));
    channel->looper_consumer.store(has_transport, std::memory_order_release);
    return 0;
  }
  uint8_t buffer[4096];
  if (fd == channel->remote_endpoint_fd) {
    constexpr size_t kMaxRemoteRxBytes = sizeof(DarwinInputFrame) * 512;
    for (;;) {
      const intptr_t received = darwin_art_bionic_socket_broker_recv(
          fd, buffer, sizeof(buffer), kAndroidMsgDontWait);
      if (received <= 0) break;
      std::lock_guard<std::mutex> lock(channel->packet_mutex);
      if (channel->remote_rx.size() + static_cast<size_t>(received) >
          kMaxRemoteRxBytes) {
        channel->remote_rx.clear();
        break;
      }
      channel->remote_rx.insert(channel->remote_rx.end(), buffer,
                                buffer + received);
    }
  } else {
    while (darwin_art_bionic_socket_broker_recv(
               fd, buffer, sizeof(buffer), kAndroidMsgDontWait) > 0) {
    }
  }
  {
    std::lock_guard<std::mutex> lock(channel->packet_mutex);
    // Both frame types share one byte stream. Iterate until neither decoder
    // advances so any number of alternating input/ACK frames is preserved.
    for (;;) {
      const size_t before = channel->remote_rx.size();
      DecodeRemoteAckFramesLocked(channel);
      (void)DecodeRemoteInputWindowFrameLocked(channel);
      (void)DecodeRemoteInputFramesLocked(channel);
      if (channel->remote_rx.size() == before) break;
    }
  }
  std::shared_ptr<DarwinInputReceiver> receiver;
  {
    std::lock_guard<std::mutex> lock(channel->packet_mutex);
    receiver = channel->consumer;
  }
  JNIEnv* env = nullptr;
  if (receiver == nullptr || receiver->disposed.load(std::memory_order_acquire) ||
      receiver->view_root == nullptr ||
      g_framework_vm == nullptr ||
      g_framework_vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) !=
          JNI_OK || env == nullptr) {
    return 1;
  }
  receiver->active_callbacks.fetch_add(1, std::memory_order_acq_rel);
  if (receiver->disposed.load(std::memory_order_acquire)) {
    const uint32_t previous =
        receiver->active_callbacks.fetch_sub(1, std::memory_order_acq_rel);
    if (previous == 1 && receiver->dispose_requested.load(std::memory_order_acquire))
      CleanupReceiverRefs(env, receiver.get());
    return 1;
  }
  constexpr size_t kMaxPacketsPerCallback = 64;
  size_t consumed = 0;
  darwin_art::DarwinArtInputPacket packet;
  while (consumed < kMaxPacketsPerCallback &&
         DequeueChannelPacketAny(channel, &packet)) {
    if (!DispatchChannelPacket(env, receiver.get(), packet)) {
      // Keep a failed payload queued for the next owner turn. This avoids
      // losing a boundary event when ViewRoot is being attached or a
      // transient Java exception interrupts dispatch.
      std::lock_guard<std::mutex> lock(channel->packet_mutex);
      channel->packets.push_front(packet);
      break;
    }
    ++consumed;
  }
  if (consumed > 0) ClearInputChannelPending(channel);
  const bool rearmed = RearmInputChannelTransport(channel);
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android InputChannel callback consumed=" << consumed
              << " pending="
              << (channel->pending_input.load(std::memory_order_acquire) ? 1 : 0)
              << " rearmed=" << (rearmed ? 1 : 0)
              << "\n";
  }
  const uint32_t previous =
      receiver->active_callbacks.fetch_sub(1, std::memory_order_acq_rel);
  if (previous == 1 && receiver->dispose_requested.load(std::memory_order_acquire))
    CleanupReceiverRefs(env, receiver.get());
  return 1;
}

jboolean InputReceiverConsume(JNIEnv*, jclass, jlong, jlong) {
  return JNI_FALSE;
}
void InputReceiverDispose(JNIEnv* env, jclass, jlong pointer) {
  auto* receiver = reinterpret_cast<DarwinInputReceiver*>(
      static_cast<std::uintptr_t>(pointer));
  if (receiver == nullptr) return;
  auto receiver_lease = FindInputReceiverLease(receiver);
  if (receiver_lease == nullptr) return;
  std::shared_ptr<DarwinInputReceiver> keep_alive;
  receiver->dispose_requested.store(true, std::memory_order_release);
  receiver->disposed.store(true, std::memory_order_release);
  if (receiver->channel != nullptr) {
    auto channel = receiver->channel;
    {
      std::lock_guard<std::mutex> lock(channel->packet_mutex);
      if (channel->consumer.get() == receiver) keep_alive = channel->consumer;
    }
    channel->looper_consumer.store(false, std::memory_order_release);
    {
      std::lock_guard<std::mutex> lock(channel->packet_mutex);
      if (channel->consumer.get() == receiver) channel->consumer.reset();
    }
  }
  if (receiver->transport_registered.load(std::memory_order_acquire) &&
      receiver->looper != nullptr &&
      receiver->channel != nullptr && receiver->channel->read_fd >= 0) {
    (void)darwin_art_android_platform_remove_fd(
        receiver->looper, receiver->channel->read_fd);
  }
  if (receiver->remote_transport_registered.load(std::memory_order_acquire) &&
      receiver->looper != nullptr && receiver->channel != nullptr &&
      receiver->channel->remote_endpoint_fd >= 0) {
    (void)darwin_art_android_platform_remove_fd(
        receiver->looper, receiver->channel->remote_endpoint_fd);
  }
  if (receiver->active_callbacks.load(std::memory_order_acquire) == 0) {
    CleanupReceiverRefs(env, receiver);
  }
  if (receiver->channel == nullptr) {
    std::lock_guard<std::mutex> lock(g_input_receiver_registry_mutex);
    g_input_receiver_registry.erase(receiver);
  }
  // A channel-owned shared_ptr (or the callback's strong snapshot) performs
  // the actual object destruction after any in-flight callback returns.
  if (keep_alive == nullptr && receiver->channel == nullptr) delete receiver;
}
jstring InputReceiverDump(JNIEnv* env, jclass, jlong, jstring) {
  return env->NewStringUTF("");
}
void InputReceiverFinish(JNIEnv*, jclass, jlong pointer, jint sequence,
                         jboolean handled) {
  auto* receiver = reinterpret_cast<DarwinInputReceiver*>(
      static_cast<std::uintptr_t>(pointer));
  if (receiver != nullptr) {
    receiver->last_finished_sequence = sequence;
    receiver->last_finished_handled = handled == JNI_TRUE;
    if (receiver->channel != nullptr) {
      bool queued = false;
      {
        std::lock_guard<std::mutex> lock(receiver->channel->finish_mutex);
        if (receiver->channel->pending_finish_set.contains(sequence)) {
          auto existing = std::find_if(
              receiver->channel->finish_acks.begin(),
              receiver->channel->finish_acks.end(),
              [sequence](const auto& ack) { return ack.sequence == sequence; });
          if (existing != receiver->channel->finish_acks.end()) {
            existing->handled = handled == JNI_TRUE;
          } else {
            receiver->channel->finish_acks.push_back(
                DarwinInputChannelState::FinishAck{
                    .sequence = sequence, .handled = handled == JNI_TRUE});
          }
          queued = true;
        }
      }
      if (queued) receiver->channel->finish_condition.notify_all();
      receiver->channel->last_finished_sequence.store(
          sequence, std::memory_order_release);
      receiver->channel->last_finished_handled.store(
          handled == JNI_TRUE, std::memory_order_release);
      // A remote InputChannel peer waits on the same sequence ACK that local
      // callers observe through finish_acks. Keep the wire acknowledgement
      // outside finish_mutex so a short broker backpressure cannot block the
      // Java callback's local completion path.
      (void)SendRemoteAckFrame(receiver->channel.get(), sequence,
                               handled == JNI_TRUE);
    }
  }
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android InputEvent finish sequence=" << sequence
              << " handled=" << (handled == JNI_TRUE ? 1 : 0) << "\n";
  }
}

void RetireFinishSequenceLocked(DarwinInputChannelState* channel,
                                jint sequence) {
  if (channel == nullptr) return;
  channel->pending_finish_set.erase(sequence);
  auto pending = std::find(channel->pending_finish_sequences.begin(),
                           channel->pending_finish_sequences.end(), sequence);
  if (pending != channel->pending_finish_sequences.end()) {
    channel->pending_finish_sequences.erase(pending);
  }
}

void RegisterFinishSequence(
    const std::shared_ptr<DarwinInputChannelState>& channel, jint sequence) {
  if (channel == nullptr) return;
  std::lock_guard<std::mutex> lock(channel->finish_mutex);
  constexpr size_t kMaxPendingFinishSequences = 256;
  while (channel->pending_finish_sequences.size() >=
         kMaxPendingFinishSequences) {
    const jint evicted = channel->pending_finish_sequences.front();
    channel->pending_finish_sequences.pop_front();
    channel->pending_finish_set.erase(evicted);
    std::erase_if(channel->finish_acks,
                  [evicted](const auto& ack) { return ack.sequence == evicted; });
    ++channel->finish_ack_overflows;
    if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
      std::cerr << "ART Android InputEvent ACK overflow evicted sequence="
                << evicted << " total=" << channel->finish_ack_overflows
                << "\n";
    }
  }
  channel->pending_finish_sequences.push_back(sequence);
  channel->pending_finish_set.insert(sequence);
}

bool TakeFinishedAckLocked(DarwinInputChannelState* channel, jint sequence,
                           bool* handled) {
  if (channel == nullptr) return false;
  for (auto it = channel->finish_acks.begin();
       it != channel->finish_acks.end(); ++it) {
    if (it->sequence != sequence) continue;
    if (handled != nullptr) *handled = it->handled;
    channel->finish_acks.erase(it);
    RetireFinishSequenceLocked(channel, sequence);
    return true;
  }
  return false;
}

bool TakeFinishedAck(const std::shared_ptr<DarwinInputChannelState>& channel,
                     jint sequence, bool* handled) {
  if (channel == nullptr) return false;
  std::lock_guard<std::mutex> lock(channel->finish_mutex);
  return TakeFinishedAckLocked(channel.get(), sequence, handled);
}

// ViewRoot normally finishes an event inline, but application code may defer
// finishInputEvent() to another callback. Give that ACK a small bounded window
// without holding the finish mutex across any Java call; a missing ACK still
// falls back to the legacy atomic result below.
bool WaitForFinishedAck(const std::shared_ptr<DarwinInputChannelState>& channel,
                        jint sequence, bool* handled) {
  if (TakeFinishedAck(channel, sequence, handled)) return true;
  if (channel == nullptr) return false;
  constexpr auto kAckRetryBudget = std::chrono::milliseconds(1);
  const auto deadline = std::chrono::steady_clock::now() + kAckRetryBudget;
  std::unique_lock<std::mutex> lock(channel->finish_mutex);
  for (;;) {
    if (TakeFinishedAckLocked(channel.get(), sequence, handled)) return true;
    if (channel->finish_condition.wait_until(lock, deadline) ==
        std::cv_status::timeout) {
      return false;
    }
  }
}
jlong InputReceiverInit(JNIEnv* env, jclass, jobject weak_receiver,
                        jobject input_channel, jobject) {
  jclass channel_class = env->GetObjectClass(input_channel);
  jfieldID pointer_field = channel_class == nullptr
                               ? nullptr
                               : env->GetFieldID(channel_class, "mPtr", "J");
  const jlong pointer = pointer_field == nullptr
                            ? 0
                            : env->GetLongField(input_channel, pointer_field);
  const auto* channel = InputChannel(pointer);
  env->DeleteLocalRef(channel_class);
  if (channel == nullptr || channel->disposed || channel->state == nullptr ||
      weak_receiver == nullptr || env->ExceptionCheck()) {
    return 0;
  }
  std::shared_ptr<DarwinInputReceiver> receiver_shared(
      new (std::nothrow) DarwinInputReceiver());
  if (receiver_shared == nullptr) return 0;
  receiver_shared->weak_receiver = env->NewGlobalRef(weak_receiver);
  receiver_shared->channel = channel->state;
  if (receiver_shared->weak_receiver == nullptr) {
    return 0;
  }
  auto* receiver = receiver_shared.get();
  {
    std::lock_guard<std::mutex> lock(g_input_receiver_registry_mutex);
    g_input_receiver_registry.emplace(receiver, receiver_shared);
  }
  {
    std::lock_guard<std::mutex> lock(receiver->channel->packet_mutex);
    receiver->channel->consumer = receiver_shared;
  }
  // The framework constructs this native receiver for
  // ViewRootImpl.WindowInputEventReceiver. Capture the enclosing ViewRoot at
  // that standard ownership boundary; no presenter or probe should have to
  // discover and inject it later.
  jclass weak_class = env->GetObjectClass(weak_receiver);
  jmethodID weak_get = weak_class == nullptr
                           ? nullptr
                           : env->GetMethodID(weak_class, "get",
                                              "()Ljava/lang/Object;");
  jobject java_receiver = weak_get == nullptr
                              ? nullptr
                              : env->CallObjectMethod(weak_receiver, weak_get);
  jclass java_receiver_class = java_receiver == nullptr
                                   ? nullptr
                                   : env->GetObjectClass(java_receiver);
  jfieldID view_root_field =
      java_receiver_class == nullptr
          ? nullptr
          : env->GetFieldID(java_receiver_class, "this$0",
                            "Landroid/view/ViewRootImpl;");
  jobject view_root = view_root_field == nullptr
                          ? nullptr
                          : env->GetObjectField(java_receiver, view_root_field);
  if (view_root != nullptr && !env->ExceptionCheck()) {
    darwin_art::diagnostics::LogViewRootResources(env, view_root);
    receiver->view_root = env->NewGlobalRef(view_root);
    UpdateInputWindowGeometry(env, receiver);
  }
  jmethodID focus_event =
      java_receiver_class == nullptr
          ? nullptr
          : env->GetMethodID(java_receiver_class, "onFocusEvent", "(Z)V");
  jmethodID touch_mode_event =
      java_receiver_class == nullptr
          ? nullptr
          : env->GetMethodID(java_receiver_class, "onTouchModeChanged", "(Z)V");
  if (java_receiver != nullptr && focus_event != nullptr &&
      touch_mode_event != nullptr && !env->ExceptionCheck()) {
    env->CallVoidMethod(java_receiver, focus_event, JNI_TRUE);
    env->CallVoidMethod(java_receiver, touch_mode_event, JNI_TRUE);
    if (!env->ExceptionCheck()) {
      receiver->focused = true;
      receiver->touch_mode = true;
      SetFocusedInputChannel(receiver->channel);
    }
  }
  if (env->ExceptionCheck()) env->ExceptionClear();
  if (view_root != nullptr) env->DeleteLocalRef(view_root);
  if (java_receiver_class != nullptr) env->DeleteLocalRef(java_receiver_class);
  if (java_receiver != nullptr) env->DeleteLocalRef(java_receiver);
  if (weak_class != nullptr) env->DeleteLocalRef(weak_class);
  receiver->looper = darwin_art_android_platform_prepare_current_looper();
  if (receiver->looper != nullptr && receiver->channel->read_fd >= 0) {
    // ALOOPER_EVENT_INPUT is the NDK value used by InputEventReceiver's
    // local transport. The callback drains wake tokens; framework event
    // payloads remain owned by the channel queue and are dispatched in order.
    receiver->transport_registered.store(
        darwin_art_android_platform_add_fd(
            receiver->looper, receiver->channel->read_fd, 0, 0x0001,
            &InputChannelTransportCallback, receiver->channel.get()) == 1,
        std::memory_order_release);
  }
  if (receiver->looper != nullptr &&
      receiver->channel->remote_endpoint_fd >= 0 &&
      receiver->channel->remote_endpoint_fd != receiver->channel->read_fd) {
    receiver->remote_transport_registered.store(
        darwin_art_android_platform_add_fd(
            receiver->looper, receiver->channel->remote_endpoint_fd, 0, 0x0001,
            &InputChannelTransportCallback, receiver->channel.get()) == 1,
        std::memory_order_release);
  }
  receiver->channel->looper_consumer.store(
      receiver->transport_registered.load(std::memory_order_acquire) ||
          receiver->remote_transport_registered.load(
              std::memory_order_acquire),
      std::memory_order_release);
  // Registration is the InputDispatcher focus-selection boundary for the
  // single key macOS application window. Focus callbacks above preserve the
  // framework state; channel selection must not depend on reflection success.
  SetFocusedInputChannel(receiver->channel);
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android InputChannel receiver-init name="
              << receiver->channel->name
              << " view_root=" << (receiver->view_root != nullptr ? 1 : 0)
              << " local_transport="
              << (receiver->transport_registered.load(std::memory_order_acquire)
                      ? 1
                      : 0)
              << " remote_transport="
              << (receiver->remote_transport_registered.load(
                      std::memory_order_acquire)
                      ? 1
                      : 0)
              << "\n";
  }
  return static_cast<jlong>(
      reinterpret_cast<std::uintptr_t>(receiver));
}
jboolean InputReceiverProbablyHasInput(JNIEnv*, jclass pointer_class,
                                       jlong pointer) {
  (void)pointer_class;
  auto* receiver = reinterpret_cast<DarwinInputReceiver*>(
      static_cast<std::uintptr_t>(pointer));
  if (receiver == nullptr || receiver->channel == nullptr ||
      receiver->weak_receiver == nullptr) {
    return JNI_FALSE;
  }
  return receiver->channel->pending_input.load(std::memory_order_acquire)
             ? JNI_TRUE
             : JNI_FALSE;
}
void InputReceiverReportTimeline(JNIEnv*, jclass, jlong, jint, jlong, jlong) {}

struct DarwinKeyCharacterMap {
  jint device_id;
};

DarwinKeyCharacterMap* KeyMap(jlong pointer) {
  return reinterpret_cast<DarwinKeyCharacterMap*>(
      static_cast<std::uintptr_t>(pointer));
}

void KeyMapApplyOverlay(JNIEnv*, jclass, jlong, jstring, jstring) {}
void KeyMapDispose(JNIEnv*, jclass, jlong pointer) { delete KeyMap(pointer); }
jboolean KeyMapEquals(JNIEnv*, jclass, jlong left, jlong right) {
  const auto* lhs = KeyMap(left);
  const auto* rhs = KeyMap(right);
  return lhs != nullptr && rhs != nullptr && lhs->device_id == rhs->device_id
             ? JNI_TRUE
             : JNI_FALSE;
}
jchar MappedKeyCharacter(jint key_code, jint meta_state) {
  const bool shift = (meta_state & 0x1) != 0;
  if (key_code >= 29 && key_code <= 54) {
    const char base = static_cast<char>('a' + (key_code - 29));
    return static_cast<jchar>(shift ? base - ('a' - 'A') : base);
  }
  if (key_code >= 7 && key_code <= 16) {
    static constexpr char kPlain[] = "0123456789";
    static constexpr char kShifted[] = ")!@#$%^&*(";
    const size_t index = static_cast<size_t>(key_code - 7);
    return static_cast<jchar>(shift ? kShifted[index] : kPlain[index]);
  }
  switch (key_code) {
    case 55: return shift ? '<' : ',';
    case 56: return shift ? '>' : '.';
    case 62: return ' ';
    case 68: return shift ? '~' : '`';
    case 69: return shift ? '_' : '-';
    case 70: return shift ? '+' : '=';
    case 71: return shift ? '{' : '[';
    case 72: return shift ? '}' : ']';
    case 73: return shift ? '|' : '\\';
    case 74: return shift ? ':' : ';';
    case 75: return shift ? '"' : '\'';
    case 76: return shift ? '?' : '/';
    default: return 0;
  }
}
jchar KeyMapGetCharacter(JNIEnv*, jclass, jlong, jint key_code,
                         jint meta_state) {
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android KeyCharacterMap: character key=" << key_code
              << " meta=" << meta_state << "\n";
  }
  return MappedKeyCharacter(key_code, meta_state);
}
jchar KeyMapGetDisplayLabel(JNIEnv* env, jclass klass, jlong pointer,
                            jint key_code) {
  // Android's physical KeyCharacterMap exposes an unmodified printable label
  // in addition to the meta-state-dependent character. KeyEvent.isPrintingKey
  // (and Chromium's hardware keyboard path) relies on this value.
  return KeyMapGetCharacter(env, klass, pointer, key_code, 0);
}
struct DarwinMappedKeyStroke {
  jint key_code;
  bool shift;
};

bool FindMappedKeyStroke(jchar character, DarwinMappedKeyStroke* result) {
  if (character == 0 || result == nullptr) return false;
  constexpr jint kKeyCodeScanLimit = 512;
  for (jint meta_state : {0, 1}) {
    for (jint key_code = 1; key_code <= kKeyCodeScanLimit; ++key_code) {
      if (MappedKeyCharacter(key_code, meta_state) == character) {
        *result = DarwinMappedKeyStroke{key_code, meta_state != 0};
        return true;
      }
    }
  }
  return false;
}

jobjectArray KeyMapGetEvents(JNIEnv* env, jclass, jlong pointer,
                             jcharArray characters) {
  const auto* map = KeyMap(pointer);
  jclass event_class = env->FindClass("android/view/KeyEvent");
  if (event_class == nullptr) return nullptr;
  if (map == nullptr) {
    jobjectArray empty = env->NewObjectArray(0, event_class, nullptr);
    env->DeleteLocalRef(event_class);
    return empty;
  }
  if (characters == nullptr) {
    env->DeleteLocalRef(event_class);
    return nullptr;
  }
  const jsize character_count = env->GetArrayLength(characters);
  std::vector<jchar> values(static_cast<size_t>(character_count));
  if (character_count > 0) {
    env->GetCharArrayRegion(characters, 0, character_count, values.data());
    if (env->ExceptionCheck()) {
      env->DeleteLocalRef(event_class);
      return nullptr;
    }
  }
  std::vector<DarwinMappedKeyStroke> strokes;
  strokes.reserve(values.size());
  size_t event_count = 0;
  for (jchar value : values) {
    DarwinMappedKeyStroke stroke{};
    if (!FindMappedKeyStroke(value, &stroke)) {
      env->DeleteLocalRef(event_class);
      return nullptr;
    }
    strokes.push_back(stroke);
    event_count += stroke.shift ? 4 : 2;
  }
  if (event_count > static_cast<size_t>(std::numeric_limits<jsize>::max())) {
    env->DeleteLocalRef(event_class);
    return nullptr;
  }
  jobjectArray result = env->NewObjectArray(
      static_cast<jsize>(event_count), event_class, nullptr);
  jmethodID constructor = env->GetMethodID(
      event_class, "<init>", "(JJIIIIIIII)V");
  if (result == nullptr || constructor == nullptr) {
    env->DeleteLocalRef(event_class);
    return nullptr;
  }
  constexpr jint kActionDown = 0;
  constexpr jint kActionUp = 1;
  constexpr jint kShiftLeftKeyCode = 59;
  constexpr jint kShiftLeftMetaState = 0x41;
  constexpr jint kKeyboardSource = 0x101;
  const jlong now = static_cast<jlong>(
      std::chrono::duration_cast<std::chrono::milliseconds>(
          std::chrono::steady_clock::now().time_since_epoch()).count());
  jsize index = 0;
  auto append_event = [&](jint action, jint key_code, jint meta_state) {
    jobject event = env->NewObject(
        event_class, constructor, now, now, action, key_code, 0, meta_state,
        map->device_id, 0, 0, kKeyboardSource);
    if (event == nullptr) return false;
    env->SetObjectArrayElement(result, index++, event);
    env->DeleteLocalRef(event);
    return !env->ExceptionCheck();
  };
  for (const DarwinMappedKeyStroke& stroke : strokes) {
    if (stroke.shift &&
        !append_event(kActionDown, kShiftLeftKeyCode,
                      kShiftLeftMetaState)) {
      env->DeleteLocalRef(event_class);
      return nullptr;
    }
    const jint meta_state = stroke.shift ? kShiftLeftMetaState : 0;
    if (!append_event(kActionDown, stroke.key_code, meta_state) ||
        !append_event(kActionUp, stroke.key_code, meta_state) ||
        (stroke.shift &&
         !append_event(kActionUp, kShiftLeftKeyCode, 0))) {
      env->DeleteLocalRef(event_class);
      return nullptr;
    }
  }
  env->DeleteLocalRef(event_class);
  return result;
}
jboolean KeyMapGetFallbackAction(JNIEnv*, jclass, jlong, jint, jint, jobject) {
  return JNI_FALSE;
}
jint KeyMapGetKeyboardType(JNIEnv*, jclass, jlong) {
  // KeyCharacterMap.FULL: macOS normally supplies a connected physical
  // keyboard, not Android's synthetic VIRTUAL_KEYBOARD device.
  return 4;
}
jint KeyMapGetMappedKey(JNIEnv*, jclass, jlong, jint) { return 0; }
jchar KeyMapGetMatch(JNIEnv* env, jclass, jlong pointer, jint key_code,
                     jcharArray candidates, jint meta_state) {
  if (KeyMap(pointer) == nullptr || candidates == nullptr) return 0;
  const jsize candidate_count = env->GetArrayLength(candidates);
  if (candidate_count <= 0) return 0;
  std::vector<jchar> values(static_cast<size_t>(candidate_count));
  env->GetCharArrayRegion(candidates, 0, candidate_count, values.data());
  if (env->ExceptionCheck()) return 0;
  const auto contains = [&](jchar value) {
    return value != 0 &&
           std::find(values.begin(), values.end(), value) != values.end();
  };

  // AOSP prefers a behavior compatible with the requested meta state before
  // returning another character generated by the same key. Our current US
  // physical-keyboard map has base and Shift behaviors, both backed by the
  // same mapping used by nativeGetCharacter/nativeGetEvents.
  const jchar preferred = MappedKeyCharacter(key_code, meta_state);
  if (contains(preferred)) return preferred;
  const jchar base = MappedKeyCharacter(key_code, 0);
  if (contains(base)) return base;
  const jchar shifted = MappedKeyCharacter(key_code, 1);
  return contains(shifted) ? shifted : 0;
}
jchar KeyMapGetNumber(JNIEnv*, jclass, jlong pointer, jint key_code) {
  if (KeyMap(pointer) == nullptr) return 0;
  const jchar base = MappedKeyCharacter(key_code, 0);
  // The Darwin map models Android's modern FULL physical keyboard. Preserve
  // the unmodified decimal row as its dial-pad character; alphabetic T9
  // numbers belong to legacy ALPHA/Virtual layouts and must not be invented
  // for a physical keyboard.
  return base >= '0' && base <= '9' ? base : 0;
}
jobject KeyMapObtainEmpty(JNIEnv* env, jclass klass, jint device_id) {
  auto* map = new (std::nothrow) DarwinKeyCharacterMap{device_id};
  if (map == nullptr) return nullptr;
  jmethodID constructor = env->GetMethodID(klass, "<init>", "(J)V");
  jobject result =
      constructor == nullptr
          ? nullptr
          : env->NewObject(klass, constructor,
                           static_cast<jlong>(reinterpret_cast<std::uintptr_t>(map)));
  if (result == nullptr || env->ExceptionCheck()) {
    delete map;
    return nullptr;
  }
  return result;
}
constexpr jint kDarwinKeyCharacterMapParcelMagic = 0x44414b4d;  // DAKM
constexpr jint kDarwinKeyCharacterMapParcelVersion = 1;

jlong KeyMapReadFromParcel(JNIEnv* env, jclass, jobject parcel_object) {
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
  android::Parcel* parcel = android::parcelForJavaObject(env, parcel_object);
  int32_t magic = 0;
  int32_t version = 0;
  int32_t device_id = 0;
  if (parcel == nullptr || parcel->readInt32(&magic) != android::NO_ERROR ||
      parcel->readInt32(&version) != android::NO_ERROR ||
      magic != kDarwinKeyCharacterMapParcelMagic ||
      version != kDarwinKeyCharacterMapParcelVersion ||
      parcel->readInt32(&device_id) != android::NO_ERROR) {
    return 0;
  }
  auto* map = new (std::nothrow) DarwinKeyCharacterMap{device_id};
  return static_cast<jlong>(reinterpret_cast<std::uintptr_t>(map));
#else
  DarwinParcel* parcel = JavaParcel(env, parcel_object);
  if (parcel == nullptr ||
      ParcelReadInt(reinterpret_cast<jlong>(parcel)) !=
          kDarwinKeyCharacterMapParcelMagic ||
      ParcelReadInt(reinterpret_cast<jlong>(parcel)) !=
          kDarwinKeyCharacterMapParcelVersion) {
    return 0;
  }
  const jint device_id = ParcelReadInt(reinterpret_cast<jlong>(parcel));
  auto* map = new (std::nothrow) DarwinKeyCharacterMap{device_id};
  return static_cast<jlong>(reinterpret_cast<std::uintptr_t>(map));
#endif
}
void KeyMapWriteToParcel(JNIEnv* env, jclass, jlong pointer,
                         jobject parcel_object) {
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
  android::Parcel* parcel = android::parcelForJavaObject(env, parcel_object);
  const auto* map = KeyMap(pointer);
  if (parcel == nullptr || map == nullptr) return;
  if (parcel->writeInt32(kDarwinKeyCharacterMapParcelMagic) != android::NO_ERROR ||
      parcel->writeInt32(kDarwinKeyCharacterMapParcelVersion) != android::NO_ERROR ||
      parcel->writeInt32(map->device_id) != android::NO_ERROR) {
    jclass type = env->FindClass("java/lang/IllegalStateException");
    if (type != nullptr) env->ThrowNew(type, "Could not parcel key character map");
    env->DeleteLocalRef(type);
  }
#else
  DarwinParcel* parcel = JavaParcel(env, parcel_object);
  const auto* map = KeyMap(pointer);
  if (parcel == nullptr || map == nullptr) return;
  const jlong parcel_pointer = reinterpret_cast<jlong>(parcel);
  ParcelWriteInt(parcel_pointer, kDarwinKeyCharacterMapParcelMagic);
  ParcelWriteInt(parcel_pointer, kDarwinKeyCharacterMapParcelVersion);
  ParcelWriteInt(parcel_pointer, map->device_id);
#endif
}

std::atomic<jint> g_next_key_event_id{1};

jint KeyEventNextId(JNIEnv*, jclass) {
  return g_next_key_event_id.fetch_add(1, std::memory_order_relaxed);
}

jint KeyEventCodeFromString(JNIEnv* env, jclass, jstring value) {
  if (value == nullptr) return 0;
  const char* utf = env->GetStringUTFChars(value, nullptr);
  if (utf == nullptr) return 0;
  jint result = 0;
  if (std::strncmp(utf, "KEYCODE_", 8) == 0) {
    char* end = nullptr;
    const long parsed = std::strtol(utf + 8, &end, 10);
    if (end != utf + 8 && *end == '\0' && parsed >= 0 && parsed <= INT32_MAX) {
      result = static_cast<jint>(parsed);
    }
  }
  env->ReleaseStringUTFChars(value, utf);
  return result;
}

jstring KeyEventCodeToString(JNIEnv* env, jclass, jint key_code) {
  const std::string value = std::to_string(key_code);
  return env->NewStringUTF(value.c_str());
}

jobject BinderInternalGetContextObject(JNIEnv* env, jclass) {
  return darwin_art::GetSystemContextObject(env);
}

void BinderInternalHandleGc(JNIEnv*, jclass) {
  // AOSP forwards BinderInternal.handleGc() to
  // IPCThreadState::flushCommands(). The process-local Darwin transport has no
  // kernel binder command buffer, so the equivalent flush boundary is the
  // same operation exposed by Binder.flushPendingCommands().
  BinderFlushPendingCommands();
}

jobject ServiceManagerProxyGetNativeServiceManager(JNIEnv* env, jobject) {
  return darwin_art::GetSystemContextObject(env);
}

bool Register(JNIEnv* env, const char* class_name, JNINativeMethod* methods,
              jint method_count) {
  jclass klass = env->FindClass(class_name);
  if (klass == nullptr) {
    return false;
  }
  const bool registered =
      env->RegisterNatives(klass, methods, method_count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return registered;
}

constexpr uint32_t kWireMagic = 0x44414252;  // DABR
constexpr uint32_t kWireVersion = 1;
constexpr uint32_t kWireReady = 1;
constexpr uint32_t kWireTransaction = 2;
constexpr uint32_t kWireReply = 3;
constexpr uint32_t kWireServiceBindIntent = 4;
constexpr uint32_t kWireBinderReturnsHome = 1;
constexpr uint32_t kBinderFlagOneWay = 1;
constexpr size_t kMaxWireBytes = 64 * 1024 * 1024;
constexpr size_t kMaxWireObjects = 1024;

struct WireHeader {
  uint32_t magic = kWireMagic;
  uint32_t version = kWireVersion;
  uint32_t type = 0;
  uint32_t sequence = 0;
  uint32_t target = 0;
  uint32_t code = 0;
  uint32_t flags = 0;
  int32_t status = 0;
  uint32_t data_size = 0;
  uint32_t binder_count = 0;
  uint32_t fd_count = 0;
  uint32_t reserved = 0;
};
static_assert(sizeof(WireHeader) == 48);

struct WireBinder {
  uint32_t target = 0;
  uint32_t flags = 0;
};

enum : uint32_t { kWireFdRegular = 0, kWireFdSharedMemory = 1 };

struct WireFd {
  uint32_t kind = kWireFdRegular;
  int32_t protection = 0;
  uint64_t size = 0;
};
static_assert(sizeof(WireFd) == 16);

struct WireMessage {
  WireHeader header;
  std::vector<uint8_t> data;
  std::vector<WireBinder> binders;
  std::vector<WireFd> fd_metadata;
  std::vector<int> file_descriptors;

  ~WireMessage() {
    for (int descriptor : file_descriptors)
      (void)darwin_art_bionic_socket_broker_close(descriptor);
  }
  WireMessage() = default;
  WireMessage(const WireMessage&) = delete;
  WireMessage& operator=(const WireMessage&) = delete;
};

struct WireConnection {
  std::optional<darwin_art::binder::PeerCredentials> peer;
  bool peer_checked = false;
  int32_t peer_android_uid = -1;
  uint64_t generation = 0;
  uint32_t next_sequence = 1;
  uint32_t next_local_target = 2;
  bool ready = false;
  bool dispatcher_active = false;
  std::thread::id dispatcher_thread;
  jobject class_loader = nullptr;
  std::unordered_map<uint32_t, jobject> local_binders;
  std::unordered_map<uint32_t, std::unique_ptr<WireMessage>> pending_replies;
};

std::recursive_mutex g_wire_mutex;
std::condition_variable_any g_wire_condition;
std::unordered_map<int, WireConnection> g_wire_connections;
std::atomic<uint64_t> g_next_wire_generation{1};

bool WriteAll(int fd, const uint8_t* bytes, size_t size) {
  while (size != 0) {
    const ssize_t written = write(fd, bytes, size);
    if (written > 0) {
      bytes += static_cast<size_t>(written);
      size -= static_cast<size_t>(written);
    } else if (written < 0 && errno == EINTR) {
      continue;
    } else {
      return false;
    }
  }
  return true;
}

bool ReadAll(int fd, uint8_t* bytes, size_t size) {
  while (size != 0) {
    const ssize_t received = read(fd, bytes, size);
    if (received > 0) {
      bytes += static_cast<size_t>(received);
      size -= static_cast<size_t>(received);
    } else if (received < 0 && errno == EINTR) {
      continue;
    } else {
      return false;
    }
  }
  return true;
}

bool SendWireMessage(int fd, const WireHeader& header,
                     const std::vector<WireBinder>& binders,
                     const std::vector<uint8_t>& data,
                     const std::vector<int>& file_descriptors) {
  const size_t binder_bytes = binders.size() * sizeof(WireBinder);
  const size_t fd_metadata_bytes = file_descriptors.size() * sizeof(WireFd);
  std::vector<uint8_t> bytes(sizeof(header) + binder_bytes +
                             fd_metadata_bytes + data.size());
  std::memcpy(bytes.data(), &header, sizeof(header));
  if (binder_bytes != 0) {
    std::memcpy(bytes.data() + sizeof(header), binders.data(), binder_bytes);
  }
  auto* fd_metadata = reinterpret_cast<WireFd*>(
      bytes.data() + sizeof(header) + binder_bytes);
  for (size_t index = 0; index < file_descriptors.size(); ++index) {
    size_t size = 0;
    int protection = 0;
    if (darwin_art_android_shared_memory_get_info(
            file_descriptors[index], &size, &protection) == 1) {
      fd_metadata[index] = {kWireFdSharedMemory, protection,
                            static_cast<uint64_t>(size)};
    }
  }
  if (!data.empty()) {
    std::memcpy(bytes.data() + sizeof(header) + binder_bytes +
                    fd_metadata_bytes,
                data.data(), data.size());
  }
  iovec vector{bytes.data(), bytes.size()};
  msghdr message{};
  message.msg_iov = &vector;
  message.msg_iovlen = 1;
  std::vector<uint8_t> control;
  std::vector<int> host_descriptors;
  if (!file_descriptors.empty()) {
    host_descriptors.reserve(file_descriptors.size());
    for (int guest_fd : file_descriptors) {
      const int host_fd = darwin_art_bionic_fd_export_for_scm(guest_fd);
      if (host_fd < 0) {
        if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
          std::cerr << "ART Binder wire: SCM export failed guest_fd="
                    << guest_fd << " errno=" << errno << "\n";
        }
        for (int exported : host_descriptors) (void)close(exported);
        return false;
      }
      host_descriptors.push_back(host_fd);
    }
    control.resize(CMSG_SPACE(file_descriptors.size() * sizeof(int)));
    message.msg_control = control.data();
    message.msg_controllen = control.size();
    cmsghdr* rights = CMSG_FIRSTHDR(&message);
    rights->cmsg_level = SOL_SOCKET;
    rights->cmsg_type = SCM_RIGHTS;
    rights->cmsg_len = CMSG_LEN(file_descriptors.size() * sizeof(int));
    std::memcpy(CMSG_DATA(rights), host_descriptors.data(),
                host_descriptors.size() * sizeof(int));
  }
  ssize_t sent;
  do {
    sent = sendmsg(fd, &message, 0);
  } while (sent < 0 && errno == EINTR);
  for (int exported : host_descriptors) (void)close(exported);
  if (sent <= 0) {
    if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr)
      std::cerr << "ART Binder wire: send failure pid=" << getpid()
                << " fd=" << fd << " errno=" << errno << "\n";
    return false;
  }
  const size_t prefix = static_cast<size_t>(sent);
  return prefix >= bytes.size() ||
         WriteAll(fd, bytes.data() + prefix, bytes.size() - prefix);
}

bool ReceiveWireMessage(int fd, WireMessage* out) {
  if (out == nullptr) return false;
  const auto fail = [&](const char* reason, ssize_t received = -1) {
    if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
      std::cerr << "ART Binder wire: receive failure fd=" << fd
                << " reason=" << reason << " received=" << received
                << " errno=" << errno << "\n";
    }
    return false;
  };
  WireHeader header{};
  iovec vector{&header, sizeof(header)};
  std::vector<uint8_t> control(CMSG_SPACE(kMaxWireObjects * sizeof(int)));
  msghdr message{};
  message.msg_iov = &vector;
  message.msg_iovlen = 1;
  message.msg_control = control.data();
  message.msg_controllen = control.size();
  ssize_t received;
  do {
    received = recvmsg(fd, &message, MSG_WAITALL);
  } while (received < 0 && errno == EINTR);
  if (received != static_cast<ssize_t>(sizeof(header)) ||
      header.magic != kWireMagic || header.version != kWireVersion ||
      header.data_size > kMaxWireBytes ||
      header.binder_count > kMaxWireObjects ||
      header.fd_count > kMaxWireObjects) {
    return fail(received == 0 ? "eof" : "short-or-invalid-header", received);
  }
  std::vector<int> host_descriptors;
  for (cmsghdr* item = CMSG_FIRSTHDR(&message); item != nullptr;
       item = CMSG_NXTHDR(&message, item)) {
    if (item->cmsg_level != SOL_SOCKET || item->cmsg_type != SCM_RIGHTS) {
      continue;
    }
    const size_t count =
        (item->cmsg_len - CMSG_LEN(0)) / sizeof(int);
    const auto* descriptors = reinterpret_cast<const int*>(CMSG_DATA(item));
    for (size_t index = 0; index < count; ++index) {
      host_descriptors.push_back(descriptors[index]);
    }
  }
  if (host_descriptors.size() != header.fd_count) {
    return fail("fd-count-mismatch", received);
  }
  out->header = header;
  out->binders.resize(header.binder_count);
  out->fd_metadata.resize(header.fd_count);
  out->data.resize(header.data_size);
  bool received_payload =
      (out->binders.empty() ||
       ReadAll(fd, reinterpret_cast<uint8_t*>(out->binders.data()),
               out->binders.size() * sizeof(WireBinder))) &&
      (out->fd_metadata.empty() ||
       ReadAll(fd, reinterpret_cast<uint8_t*>(out->fd_metadata.data()),
               out->fd_metadata.size() * sizeof(WireFd))) &&
      (out->data.empty() || ReadAll(fd, out->data.data(), out->data.size()));
  if (!received_payload) {
    for (int host_fd : host_descriptors) (void)close(host_fd);
    return fail("payload-eof-or-short", received);
  }
  for (size_t index = 0; index < host_descriptors.size(); ++index) {
    const int host_fd = host_descriptors[index];
    int guest_fd = -1;
    if (out->fd_metadata[index].kind == kWireFdSharedMemory) {
      const WireFd& metadata = out->fd_metadata[index];
      guest_fd = metadata.size <= std::numeric_limits<size_t>::max() &&
                         darwin_art_android_shared_memory_adopt(
                             host_fd, static_cast<size_t>(metadata.size),
                             metadata.protection) == 0
                     ? host_fd
                     : -1;
    } else {
      guest_fd = darwin_art_bionic_fd_import_from_scm(host_fd);
    }
    if (guest_fd < 0) {
      (void)close(host_fd);
      for (size_t remaining = index + 1; remaining < host_descriptors.size();
           ++remaining) {
        (void)close(host_descriptors[remaining]);
      }
      return fail("fd-import", received);
    }
    out->file_descriptors.push_back(guest_fd);
  }
  return true;
}

DarwinParcel* JavaParcel(JNIEnv* env, jobject parcel) {
  jclass parcel_class = parcel == nullptr ? nullptr : env->GetObjectClass(parcel);
  jfieldID native_pointer =
      parcel_class == nullptr ? nullptr : env->GetFieldID(parcel_class, "mNativePtr", "J");
  const jlong pointer = native_pointer == nullptr
                            ? 0
                            : env->GetLongField(parcel, native_pointer);
  env->DeleteLocalRef(parcel_class);
  return Parcel(pointer);
}

uint32_t RegisterLocalBinder(JNIEnv* env, int fd, jobject binder) {
  WireConnection& connection = g_wire_connections[fd];
  for (const auto& [target, candidate] : connection.local_binders) {
    if (env->IsSameObject(candidate, binder)) return target;
  }
  const uint32_t target = connection.next_local_target++;
  connection.local_binders.emplace(target, env->NewGlobalRef(binder));
  return target;
}

bool RemoteTarget(JNIEnv* env, int fd, jobject binder, uint32_t* target) {
  jclass binder_class = env->GetObjectClass(binder);
  jfieldID control =
      binder_class == nullptr ? nullptr : env->GetFieldID(binder_class, "controlFd", "I");
  if (control == nullptr) env->ExceptionClear();
  jfieldID remote_target = binder_class == nullptr
                               ? nullptr
                               : env->GetFieldID(binder_class, "targetId", "I");
  if (remote_target == nullptr) env->ExceptionClear();
  const bool matches = control != nullptr && remote_target != nullptr &&
                       env->GetIntField(binder, control) == fd;
  if (matches) {
    *target = static_cast<uint32_t>(env->GetIntField(binder, remote_target));
  }
  env->DeleteLocalRef(binder_class);
  return matches && !env->ExceptionCheck();
}

bool ExportParcel(JNIEnv* env, int fd, DarwinParcel* parcel,
                  WireHeader* header, std::vector<WireBinder>* binders,
                  std::vector<uint8_t>* data,
                  std::vector<int>* descriptors) {
  if (parcel == nullptr || header == nullptr || binders == nullptr ||
      data == nullptr || descriptors == nullptr ||
      parcel->data.size() > std::numeric_limits<uint32_t>::max()) {
    return false;
  }
  *data = parcel->data;
  binders->reserve(parcel->binders.size());
  for (jobject binder : parcel->binders) {
    uint32_t target = 0;
    if (RemoteTarget(env, fd, binder, &target)) {
      binders->push_back({target, kWireBinderReturnsHome});
    } else {
      if (env->ExceptionCheck()) env->ExceptionClear();
      binders->push_back({RegisterLocalBinder(env, fd, binder), 0});
    }
  }
  *descriptors = parcel->file_descriptors;
  header->data_size = static_cast<uint32_t>(data->size());
  header->binder_count = static_cast<uint32_t>(binders->size());
  header->fd_count = static_cast<uint32_t>(descriptors->size());
  return !env->ExceptionCheck();
}

jobject NewRemoteBinder(JNIEnv* env, int fd, uint32_t target) {
  jclass remote_class =
      env->FindClass("dev/darwinart/runtime/os/RemoteBinder");
  if (remote_class == nullptr) {
    env->ExceptionClear();
    jclass thread_class = env->FindClass("java/lang/Thread");
    jmethodID current_thread =
        thread_class == nullptr
            ? nullptr
            : env->GetStaticMethodID(thread_class, "currentThread",
                                     "()Ljava/lang/Thread;");
    jobject thread = current_thread == nullptr
                         ? nullptr
                         : env->CallStaticObjectMethod(thread_class,
                                                       current_thread);
    jmethodID get_loader =
        thread_class == nullptr
            ? nullptr
            : env->GetMethodID(thread_class, "getContextClassLoader",
                               "()Ljava/lang/ClassLoader;");
    jobject loader = get_loader == nullptr
                         ? nullptr
                         : env->CallObjectMethod(thread, get_loader);
    jclass loader_class = loader == nullptr ? nullptr : env->GetObjectClass(loader);
    jmethodID load_class =
        loader_class == nullptr
            ? nullptr
            : env->GetMethodID(loader_class, "loadClass",
                               "(Ljava/lang/String;)Ljava/lang/Class;");
    jstring name = env->NewStringUTF(
        "dev.darwinart.runtime.os.RemoteBinder");
    jobject loaded = load_class == nullptr
                         ? nullptr
                         : env->CallObjectMethod(loader, load_class, name);
    if (!env->ExceptionCheck()) {
      remote_class = static_cast<jclass>(loaded);
    }
    env->DeleteLocalRef(name);
    env->DeleteLocalRef(loader_class);
    env->DeleteLocalRef(loader);
    env->DeleteLocalRef(thread);
    env->DeleteLocalRef(thread_class);
  }
  jmethodID constructor =
      !darwin_art::RegisterRemoteBinderNatives(env, remote_class)
          ? nullptr
          : env->GetMethodID(remote_class, "<init>", "(II)V");
  jobject result = constructor == nullptr
                       ? nullptr
                       : env->NewObject(remote_class, constructor, fd,
                                        static_cast<jint>(target));
  env->DeleteLocalRef(remote_class);
  return result;
}

bool ImportParcel(JNIEnv* env, int fd, WireMessage* message,
                  DarwinParcel* parcel) {
  if (message == nullptr || parcel == nullptr) return false;
  if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
    std::cerr << "ART Binder parcel: import fd=" << fd
              << " bytes=" << message->data.size()
              << " binders=" << message->binders.size()
              << " descriptors=" << message->file_descriptors.size() << "\n";
  }
  ClearParcel(env, parcel);
  parcel->data = std::move(message->data);
  parcel->position = 0;
  for (const WireBinder& wire_binder : message->binders) {
    if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
      std::cerr << "ART Binder parcel: binder target=" << wire_binder.target
                << " flags=" << wire_binder.flags << "\n";
    }
    jobject binder = nullptr;
    if ((wire_binder.flags & kWireBinderReturnsHome) != 0) {
      auto connection = g_wire_connections.find(fd);
      auto local = connection == g_wire_connections.end()
                       ? decltype(connection->second.local_binders)::iterator{}
                       : connection->second.local_binders.find(wire_binder.target);
      if (connection != g_wire_connections.end() &&
          local != connection->second.local_binders.end()) {
        binder = env->NewLocalRef(local->second);
      }
    } else {
      binder = NewRemoteBinder(env, fd, wire_binder.target);
    }
    if (binder == nullptr || env->ExceptionCheck()) return false;
    parcel->binders.push_back(env->NewGlobalRef(binder));
    env->DeleteLocalRef(binder);
  }
  parcel->file_descriptors = std::move(message->file_descriptors);
  message->file_descriptors.clear();
  return !env->ExceptionCheck();
}

jobject ObtainJavaParcel(JNIEnv* env) {
  jclass parcel_class = env->FindClass("android/os/Parcel");
  jmethodID obtain = parcel_class == nullptr
                         ? nullptr
                         : env->GetStaticMethodID(parcel_class, "obtain",
                                                  "()Landroid/os/Parcel;");
  jobject result = obtain == nullptr
                       ? nullptr
                       : env->CallStaticObjectMethod(parcel_class, obtain);
  env->DeleteLocalRef(parcel_class);
  return result;
}

void RecycleJavaParcel(JNIEnv* env, jobject parcel) {
  jclass parcel_class = parcel == nullptr ? nullptr : env->GetObjectClass(parcel);
  jmethodID recycle = parcel_class == nullptr
                          ? nullptr
                          : env->GetMethodID(parcel_class, "recycle", "()V");
  if (recycle != nullptr) env->CallVoidMethod(parcel, recycle);
  env->DeleteLocalRef(parcel_class);
  env->DeleteLocalRef(parcel);
}

bool DispatchWireTransaction(JNIEnv* env, int fd, WireMessage* request) {
  if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
    std::cerr << "ART Binder wire: dispatch fd=" << fd
              << " target=" << request->header.target
              << " code=" << request->header.code
              << " flags=" << request->header.flags << "\n";
  }
  auto connection = g_wire_connections.find(fd);
  auto local = connection == g_wire_connections.end()
                   ? decltype(connection->second.local_binders)::iterator{}
                   : connection->second.local_binders.find(request->header.target);
  if (connection == g_wire_connections.end() ||
      local == connection->second.local_binders.end()) {
    return false;
  }
  if (!connection->second.peer_checked) {
    darwin_art::binder::PeerCredentials peer{};
    if (darwin_art::binder::ReadPeerCredentials(fd, &peer)) {
      connection->second.peer = peer;
      connection->second.peer_android_uid = darwin_art_runtime_registered_process_uid(peer.pid);
    }
    connection->second.peer_checked = true;
  }
  // Unknown/inherited channels must not impersonate this server. Android UID
  // requires package registry resolution; Darwin's host UID is not that UID.
  const auto& peer = connection->second.peer;
  const int32_t caller_pid = (request->header.flags & kBinderFlagOneWay) != 0 || !peer
                                ? 0 : static_cast<int32_t>(peer->pid);
  darwin_art::binder::IncomingIdentity caller(caller_pid, connection->second.peer_android_uid);
  jobject data = ObtainJavaParcel(env);
  jobject reply = ObtainJavaParcel(env);
  DarwinParcel* data_native = JavaParcel(env, data);
  DarwinParcel* reply_native = JavaParcel(env, reply);
  const bool request_imported =
      data != nullptr && reply != nullptr && data_native != nullptr &&
      reply_native != nullptr && ImportParcel(env, fd, request, data_native);
  if (!request_imported) {
    RecycleJavaParcel(env, reply);
    RecycleJavaParcel(env, data);
    return false;
  }
  bool transaction_handled = false;
  {
    jclass binder_class = env->FindClass("android/os/IBinder");
    jmethodID transact = binder_class == nullptr
                             ? nullptr
                             : env->GetMethodID(
                                   binder_class, "transact",
                                   "(ILandroid/os/Parcel;Landroid/os/Parcel;I)Z");
    const jboolean transacted =
        transact == nullptr
            ? JNI_FALSE
            : env->CallBooleanMethod(
                  local->second, transact,
                  static_cast<jint>(request->header.code), data, reply,
                  static_cast<jint>(request->header.flags));
    transaction_handled = transact != nullptr && transacted == JNI_TRUE &&
                          !env->ExceptionCheck();
    if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
      std::cerr << "ART Binder wire: dispatched fd=" << fd
                << " code=" << request->header.code
                << " transacted=" << (transacted == JNI_TRUE)
                << " exception=" << env->ExceptionCheck() << "\n";
    }
    env->DeleteLocalRef(binder_class);
  }
  if (env->ExceptionCheck()) {
    jthrowable exception = env->ExceptionOccurred();
    env->ExceptionClear();
    jclass exception_class =
        exception == nullptr ? nullptr : env->GetObjectClass(exception);
    jmethodID to_string =
        exception_class == nullptr
            ? nullptr
            : env->GetMethodID(exception_class, "toString",
                               "()Ljava/lang/String;");
    jstring description =
        to_string == nullptr
            ? nullptr
            : static_cast<jstring>(env->CallObjectMethod(exception, to_string));
    const char* description_utf =
        description == nullptr ? nullptr
                               : env->GetStringUTFChars(description, nullptr);
    std::cerr << "ART Binder wire: dispatch exception="
              << (description_utf == nullptr ? "<unavailable>" : description_utf)
              << "\n";
    if (description_utf != nullptr) {
      env->ReleaseStringUTFChars(description, description_utf);
    }
    env->DeleteLocalRef(description);
    env->DeleteLocalRef(exception_class);
    env->DeleteLocalRef(exception);
    if (env->ExceptionCheck()) env->ExceptionClear();
    transaction_handled = false;
  }
  // Even a Binder one-way call gets an internal transport ACK. Android's
  // driver can deliver nested callbacks while dispatching it; the ACK keeps
  // this socket's caller reading until those callbacks have been drained.
  WireHeader response;
  response.type = kWireReply;
  response.sequence = request->header.sequence;
  response.status = darwin_art::binder::DecideWireDispatch(
                        transaction_handled, false)
                        .response_status;
  std::vector<WireBinder> binders;
  std::vector<uint8_t> bytes;
  std::vector<int> descriptors;
  if (transaction_handled &&
      (request->header.flags & kBinderFlagOneWay) == 0) {
    transaction_handled = ExportParcel(env, fd, reply_native, &response,
                                       &binders, &bytes, &descriptors);
    response.status = darwin_art::binder::DecideWireDispatch(
                          transaction_handled, false)
                          .response_status;
  }
  const bool response_sent =
      SendWireMessage(fd, response, binders, bytes, descriptors);
  const auto decision = darwin_art::binder::DecideWireDispatch(
      transaction_handled, response_sent);
  if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
    std::cerr << "ART Binder wire: reply fd=" << fd
              << " sequence=" << response.sequence
              << " status=" << response.status
              << " bytes=" << response.data_size
              << " handled=" << transaction_handled
              << " sent=" << response_sent
              << "\n";
  }
  RecycleJavaParcel(env, reply);
  RecycleJavaParcel(env, data);
  return decision.keep_channel;
}

void CloseRemoteBinderChannelGeneration(JNIEnv* env, int control_fd,
                                        uint64_t generation) {
  std::lock_guard<std::recursive_mutex> lock(g_wire_mutex);
  auto connection = g_wire_connections.find(control_fd);
  if (connection == g_wire_connections.end() ||
      connection->second.generation != generation) {
    return;
  }
  for (const auto& [target, binder] : connection->second.local_binders) {
    static_cast<void>(target);
    env->DeleteGlobalRef(binder);
  }
  if (connection->second.class_loader != nullptr) {
    env->DeleteGlobalRef(connection->second.class_loader);
  }
  g_wire_connections.erase(connection);
  g_wire_condition.notify_all();
}

void RunRemoteBinderDispatcher(JavaVM* vm, int fd, uint64_t generation) {
  JNIEnv* env = nullptr;
  if (vm == nullptr || vm->AttachCurrentThread(&env, nullptr) != JNI_OK ||
      env == nullptr) {
    std::lock_guard<std::recursive_mutex> lock(g_wire_mutex);
    auto connection = g_wire_connections.find(fd);
    if (connection != g_wire_connections.end() &&
        connection->second.generation == generation) {
      g_wire_connections.erase(connection);
    }
    g_wire_condition.notify_all();
    return;
  }
  jobject class_loader = nullptr;
  {
    std::lock_guard<std::recursive_mutex> lock(g_wire_mutex);
    auto connection = g_wire_connections.find(fd);
    if (connection == g_wire_connections.end() ||
        connection->second.generation != generation) {
      vm->DetachCurrentThread();
      return;
    }
    connection->second.dispatcher_thread = std::this_thread::get_id();
    class_loader = env->NewLocalRef(connection->second.class_loader);
    g_wire_condition.notify_all();
  }
  if (class_loader != nullptr) {
    jclass thread_class = env->FindClass("java/lang/Thread");
    jmethodID current_thread = thread_class == nullptr
                                   ? nullptr
                                   : env->GetStaticMethodID(
                                         thread_class, "currentThread",
                                         "()Ljava/lang/Thread;");
    jmethodID set_context_loader =
        thread_class == nullptr
            ? nullptr
            : env->GetMethodID(thread_class, "setContextClassLoader",
                               "(Ljava/lang/ClassLoader;)V");
    jobject thread = current_thread == nullptr
                         ? nullptr
                         : env->CallStaticObjectMethod(thread_class,
                                                       current_thread);
    if (thread != nullptr && set_context_loader != nullptr &&
        !env->ExceptionCheck()) {
      env->CallVoidMethod(thread, set_context_loader, class_loader);
    }
    env->DeleteLocalRef(thread);
    env->DeleteLocalRef(thread_class);
    env->DeleteLocalRef(class_loader);
    if (env->ExceptionCheck()) env->ExceptionClear();
  }
  for (;;) {
    auto incoming = std::make_unique<WireMessage>();
    if (!ReceiveWireMessage(fd, incoming.get())) {
      if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
        std::cerr << "ART Binder wire: dispatcher closing fd=" << fd
                  << " generation=" << generation << " reason=receive\n";
      }
      break;
    }
    std::lock_guard<std::recursive_mutex> lock(g_wire_mutex);
    auto connection = g_wire_connections.find(fd);
    if (connection == g_wire_connections.end() ||
        connection->second.generation != generation) {
      break;
    }
    if (incoming->header.type == kWireReady) {
      connection->second.ready = true;
      g_wire_condition.notify_all();
      continue;
    }
    if (incoming->header.type == kWireReply) {
      connection->second.pending_replies.insert_or_assign(
          incoming->header.sequence, std::move(incoming));
      g_wire_condition.notify_all();
      continue;
    }
    if (incoming->header.type != kWireTransaction ||
        !DispatchWireTransaction(env, fd, incoming.get())) {
      if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
        std::cerr << "ART Binder wire: dispatcher closing fd=" << fd
                  << " generation=" << generation << " reason=dispatch\n";
      }
      break;
    }
  }
  CloseRemoteBinderChannelGeneration(env, fd, generation);
  vm->DetachCurrentThread();
}

}  // namespace

namespace darwin_art {

void NotifyFrameworkInputPending() {
  NotifyFocusedInputChannel();
}

void ClearFrameworkInputPending() {
  ClearFocusedInputChannelPending();
}

DarwinArtInputEnqueueResult EnqueueFrameworkPointerPacket(
    const DarwinArtPointerEventV2& packet) {
  std::shared_ptr<DarwinInputChannelState> channel;
  int32_t offset_x = 0;
  int32_t offset_y = 0;
  if (packet.action == DARWIN_ART_POINTER_DOWN) {
    std::vector<std::shared_ptr<DarwinInputChannelState>> candidates;
    {
      std::lock_guard<std::mutex> registry_lock(g_input_channel_registry_mutex);
      std::erase_if(g_input_channel_registry,
                    [](const auto& candidate) { return candidate.expired(); });
      candidates.reserve(g_input_channel_registry.size());
      for (const auto& candidate : g_input_channel_registry) {
        if (auto state = candidate.lock(); state != nullptr) {
          candidates.push_back(std::move(state));
        }
      }
    }
    uint64_t selected_order = 0;
    for (const auto& candidate : candidates) {
      std::lock_guard<std::mutex> packet_lock(candidate->packet_mutex);
      const bool receiver_alive =
          candidate->consumer != nullptr &&
          !candidate->consumer->disposed.load(std::memory_order_acquire) &&
          candidate->looper_consumer.load(std::memory_order_acquire);
      const bool inside = candidate->input_geometry_valid &&
          packet.x >= candidate->input_left &&
          packet.y >= candidate->input_top &&
          packet.x < candidate->input_right &&
          packet.y < candidate->input_bottom;
      if (receiver_alive && inside && candidate->focus_order >= selected_order) {
        channel = candidate;
        offset_x = candidate->input_left;
        offset_y = candidate->input_top;
        selected_order = candidate->focus_order;
      }
    }
    std::lock_guard<std::mutex> focus_lock(g_focused_input_channel_mutex);
    if (channel == nullptr) channel = g_focused_input_channel.lock();
    if (channel != nullptr && selected_order == 0) {
      std::lock_guard<std::mutex> packet_lock(channel->packet_mutex);
      if (channel->input_geometry_valid) {
        offset_x = channel->input_left;
        offset_y = channel->input_top;
      }
    }
    g_touch_input_channel = channel;
    g_touch_offset_x = offset_x;
    g_touch_offset_y = offset_y;
  } else {
    std::lock_guard<std::mutex> lock(g_focused_input_channel_mutex);
    channel = g_touch_input_channel.lock();
    offset_x = g_touch_offset_x;
    offset_y = g_touch_offset_y;
  }
  if (channel == nullptr) {
    return DarwinArtInputEnqueueResult::kNoFocusedChannel;
  }
  DarwinArtInputPacket input;
  input.kind = DarwinArtInputPacketKind::kPointer;
  input.pointer = packet;
  input.pointer.x -= static_cast<float>(offset_x);
  input.pointer.y -= static_cast<float>(offset_y);
  const DarwinArtInputEnqueueResult result =
      EnqueueFocusedPacket(channel, input);
  if (packet.action == DARWIN_ART_POINTER_UP ||
      packet.action == DARWIN_ART_POINTER_CANCEL) {
    std::lock_guard<std::mutex> lock(g_focused_input_channel_mutex);
    const auto active = g_touch_input_channel.lock();
    if (active == nullptr || active == channel) {
      g_touch_input_channel.reset();
      g_touch_offset_x = 0;
      g_touch_offset_y = 0;
    }
  }
  return result;
}

bool DequeueFrameworkPointerPacket(DarwinArtPointerEventV2* packet) {
  if (packet == nullptr) return false;
  DarwinArtInputPacket input;
  if (!DequeueFocusedPacket(&input, DarwinArtInputPacketKind::kPointer)) {
    return false;
  }
  *packet = input.pointer;
  return true;
}

DarwinArtInputEnqueueResult EnqueueFrameworkKeyPacket(
    const DarwinArtKeyEventV1& packet) {
  std::shared_ptr<DarwinInputChannelState> channel;
  {
    std::lock_guard<std::mutex> lock(g_focused_input_channel_mutex);
    channel = g_focused_input_channel.lock();
  }
  if (channel == nullptr) {
    return DarwinArtInputEnqueueResult::kNoFocusedChannel;
  }
  DarwinArtInputPacket input;
  input.kind = DarwinArtInputPacketKind::kKey;
  input.key = packet;
  return EnqueueFocusedPacket(channel, input);
}

bool DequeueFrameworkKeyPacket(DarwinArtKeyEventV1* packet) {
  if (packet == nullptr) return false;
  DarwinArtInputPacket input;
  if (!DequeueFocusedPacket(&input, DarwinArtInputPacketKind::kKey)) {
    return false;
  }
  *packet = input.key;
  return true;
}

bool StartRemoteBinderDispatcher(JNIEnv* env, jint control_fd) {
  if (env == nullptr || control_fd < 0) return false;
  JavaVM* vm = nullptr;
  if (env->GetJavaVM(&vm) != JNI_OK || vm == nullptr) {
    if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr)
      std::cerr << "ART Binder wire: dispatcher GetJavaVM failure pid="
                << getpid() << " fd=" << control_fd << "\n";
    return false;
  }
  uint64_t generation = 0;
  {
    std::lock_guard<std::recursive_mutex> lock(g_wire_mutex);
    WireConnection& connection = g_wire_connections[control_fd];
    if (connection.dispatcher_active) return true;
    if (connection.generation == 0) {
      connection.generation =
          g_next_wire_generation.fetch_add(1, std::memory_order_relaxed);
    }
    generation = connection.generation;
    connection.dispatcher_active = true;
  }
  try {
    std::thread(RunRemoteBinderDispatcher, vm, control_fd, generation).detach();
  } catch (...) {
    if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr)
      std::cerr << "ART Binder wire: dispatcher thread failure pid="
                << getpid() << " fd=" << control_fd
                << " generation=" << generation << "\n";
    std::lock_guard<std::recursive_mutex> lock(g_wire_mutex);
    auto connection = g_wire_connections.find(control_fd);
    if (connection != g_wire_connections.end() &&
        connection->second.generation == generation) {
      g_wire_connections.erase(connection);
    }
    return false;
  }
  return true;
}

bool StartServingRemoteBinder(JNIEnv* env, jint control_fd,
                              jobject local_binder) {
  if (env == nullptr || control_fd < 0 || local_binder == nullptr) return false;
  {
    std::lock_guard<std::recursive_mutex> lock(g_wire_mutex);
    WireConnection& connection = g_wire_connections[control_fd];
    if (connection.generation == 0) {
      connection.generation =
          g_next_wire_generation.fetch_add(1, std::memory_order_relaxed);
    }
    if (!connection.local_binders.contains(1)) {
      jobject published = env->NewGlobalRef(local_binder);
      if (published == nullptr) {
        if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr)
          std::cerr << "ART Binder wire: NewGlobalRef failure pid=" << getpid()
                    << " fd=" << control_fd
                    << " generation=" << connection.generation << "\n";
        return false;
      }
      connection.local_binders.emplace(1, published);
    }
    if (connection.class_loader == nullptr) {
      jclass binder_class = env->GetObjectClass(local_binder);
      jclass class_class = env->FindClass("java/lang/Class");
      jmethodID get_class_loader =
          class_class == nullptr
              ? nullptr
              : env->GetMethodID(class_class, "getClassLoader",
                                 "()Ljava/lang/ClassLoader;");
      jobject loader = binder_class == nullptr || get_class_loader == nullptr
                           ? nullptr
                           : env->CallObjectMethod(binder_class,
                                                   get_class_loader);
      if (loader != nullptr && !env->ExceptionCheck()) {
        connection.class_loader = env->NewGlobalRef(loader);
      }
      env->DeleteLocalRef(loader);
      env->DeleteLocalRef(class_class);
      env->DeleteLocalRef(binder_class);
      if (env->ExceptionCheck()) env->ExceptionClear();
    }
    WireHeader ready;
    ready.type = kWireReady;
    if (!SendWireMessage(control_fd, ready, {}, {}, {})) {
      if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr)
        std::cerr << "ART Binder wire: ready send failure pid=" << getpid()
                  << " fd=" << control_fd
                  << " generation=" << connection.generation << "\n";
      for (const auto& [target, binder] : connection.local_binders) {
        static_cast<void>(target);
        env->DeleteGlobalRef(binder);
      }
      if (connection.class_loader != nullptr) {
        env->DeleteGlobalRef(connection.class_loader);
      }
      g_wire_connections.erase(control_fd);
      return false;
    }
    connection.ready = true;
  }
  const bool dispatcher = StartRemoteBinderDispatcher(env, control_fd);
  if (!dispatcher && std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr)
    std::cerr << "ART Binder wire: dispatcher start failure pid=" << getpid()
              << " fd=" << control_fd << "\n";
  return dispatcher;
}

bool SendServiceBindIntent(JNIEnv* env, jint control_fd, jobject intent) {
  if (env == nullptr || control_fd < 0 || intent == nullptr) return false;
  std::lock_guard<std::recursive_mutex> lock(g_wire_mutex);
  jobject parcel = ObtainJavaParcel(env);
  jclass intent_class = env->GetObjectClass(intent);
  jmethodID write_to_parcel =
      intent_class == nullptr
          ? nullptr
          : env->GetMethodID(intent_class, "writeToParcel",
                             "(Landroid/os/Parcel;I)V");
  if (parcel == nullptr || write_to_parcel == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(intent_class);
    if (parcel != nullptr) RecycleJavaParcel(env, parcel);
    return false;
  }
  env->CallVoidMethod(intent, write_to_parcel, parcel, 0);
  env->DeleteLocalRef(intent_class);
  WireHeader message;
  message.type = kWireServiceBindIntent;
  std::vector<WireBinder> binders;
  std::vector<uint8_t> bytes;
  std::vector<int> descriptors;
  const bool success =
      !env->ExceptionCheck() &&
      ExportParcel(env, control_fd, JavaParcel(env, parcel), &message, &binders,
                   &bytes, &descriptors) &&
      SendWireMessage(control_fd, message, binders, bytes, descriptors);
  RecycleJavaParcel(env, parcel);
  return success && !env->ExceptionCheck();
}

jobject ReceiveServiceBindIntent(JNIEnv* env, jint control_fd) {
  if (env == nullptr || control_fd < 0) return nullptr;
  std::lock_guard<std::recursive_mutex> lock(g_wire_mutex);
  WireMessage message;
  jobject parcel = nullptr;
  jobject intent = nullptr;
  if (!ReceiveWireMessage(control_fd, &message) ||
      message.header.type != kWireServiceBindIntent ||
      (parcel = ObtainJavaParcel(env)) == nullptr ||
      !ImportParcel(env, control_fd, &message, JavaParcel(env, parcel))) {
    if (parcel != nullptr) RecycleJavaParcel(env, parcel);
    return nullptr;
  }
  jclass intent_class = env->FindClass("android/content/Intent");
  jfieldID creator_field =
      intent_class == nullptr
          ? nullptr
          : env->GetStaticFieldID(intent_class, "CREATOR",
                                  "Landroid/os/Parcelable$Creator;");
  jobject creator = creator_field == nullptr
                        ? nullptr
                        : env->GetStaticObjectField(intent_class, creator_field);
  jclass creator_class = creator == nullptr ? nullptr : env->GetObjectClass(creator);
  jmethodID create_from_parcel =
      creator_class == nullptr
          ? nullptr
          : env->GetMethodID(creator_class, "createFromParcel",
                             "(Landroid/os/Parcel;)Ljava/lang/Object;");
  if (create_from_parcel != nullptr && !env->ExceptionCheck()) {
    intent = env->CallObjectMethod(creator, create_from_parcel, parcel);
  }
  env->DeleteLocalRef(creator_class);
  env->DeleteLocalRef(creator);
  env->DeleteLocalRef(intent_class);
  RecycleJavaParcel(env, parcel);
  return env->ExceptionCheck() ? nullptr : intent;
}

jboolean TransactRemoteBinder(JNIEnv* env, jint control_fd, jint target_id,
                              jint code, jobject data, jobject reply,
                              jint flags) {
  if (env == nullptr || control_fd < 0 || target_id <= 0 || data == nullptr) {
    return JNI_FALSE;
  }
  std::unique_lock<std::recursive_mutex> lock(g_wire_mutex);
  const auto debug_failure = [&](const char* phase) {
    if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
      std::cerr << "ART Binder wire: transact failure fd=" << control_fd
                << " target=" << target_id << " code=" << code
                << " phase=" << phase << " (channel state changed)\n";
    }
  };
  if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
    std::cerr << "ART Binder wire: transact fd=" << control_fd
              << " target=" << target_id << " code=" << code
              << " flags=" << flags << "\n";
  }
  WireConnection& connection = g_wire_connections[control_fd];
  if (!connection.ready) {
    if (connection.dispatcher_active &&
        connection.dispatcher_thread != std::this_thread::get_id()) {
      g_wire_condition.wait(lock, [&] {
        auto current = g_wire_connections.find(control_fd);
        return current == g_wire_connections.end() || current->second.ready;
      });
      auto current = g_wire_connections.find(control_fd);
      if (current == g_wire_connections.end() || !current->second.ready) {
        debug_failure("wait-ready-dispatcher");
        return JNI_FALSE;
      }
    } else {
      WireMessage ready;
      if (!ReceiveWireMessage(control_fd, &ready) ||
          ready.header.type != kWireReady) {
        debug_failure("wait-ready");
        return JNI_FALSE;
      }
      connection.ready = true;
    }
  }
  WireHeader request;
  request.type = kWireTransaction;
  request.sequence = connection.next_sequence++;
  request.target = static_cast<uint32_t>(target_id);
  request.code = static_cast<uint32_t>(code);
  request.flags = static_cast<uint32_t>(flags);
  std::vector<WireBinder> binders;
  std::vector<uint8_t> bytes;
  std::vector<int> descriptors;
  if (!ExportParcel(env, control_fd, JavaParcel(env, data), &request, &binders,
                    &bytes, &descriptors) ||
      !SendWireMessage(control_fd, request, binders, bytes, descriptors)) {
    debug_failure("send-request");
    return JNI_FALSE;
  }
  // The service owner thread is the sole socket reader. Android Binder calls
  // may originate from any Chromium thread after IChildProcessService.setup()
  // returns (notably IGpuProcessCallback.getViewSurface()). Let that owner
  // dispatch the reply instead of racing two recvmsg() calls on one stream.
  if (connection.dispatcher_active &&
      connection.dispatcher_thread != std::this_thread::get_id()) {
    const uint32_t sequence = request.sequence;
    g_wire_condition.wait(lock, [&] {
      auto current = g_wire_connections.find(control_fd);
      return current == g_wire_connections.end() ||
             current->second.pending_replies.contains(sequence);
    });
    auto current = g_wire_connections.find(control_fd);
    if (current == g_wire_connections.end()) {
      debug_failure("wait-reply-channel-closed");
      return JNI_FALSE;
    }
    auto pending = current->second.pending_replies.find(sequence);
    if (pending == current->second.pending_replies.end()) {
      debug_failure("wait-reply-missing");
      return JNI_FALSE;
    }
    std::unique_ptr<WireMessage> incoming = std::move(pending->second);
    current->second.pending_replies.erase(pending);
    if (incoming->header.type != kWireReply ||
        incoming->header.status != 0) {
      debug_failure("wait-reply-status");
      return JNI_FALSE;
    }
    return (flags & kBinderFlagOneWay) != 0 || reply == nullptr ||
                   ImportParcel(env, control_fd, incoming.get(),
                                JavaParcel(env, reply))
               ? JNI_TRUE
               : JNI_FALSE;
  }
  // Wait for the transport ACK even though Java observes one-way semantics.
  // Nested callback transactions are dispatched by the loop before the ACK.
  for (;;) {
    WireMessage incoming;
    if (!ReceiveWireMessage(control_fd, &incoming)) {
      debug_failure("receive-reply");
      return JNI_FALSE;
    }
    if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
      std::cerr << "ART Binder wire: response fd=" << control_fd
                << " type=" << incoming.header.type
                << " sequence=" << incoming.header.sequence
                << " status=" << incoming.header.status
                << " bytes=" << incoming.header.data_size << "\n";
    }
    if (incoming.header.type == kWireTransaction) {
      if (!DispatchWireTransaction(env, control_fd, &incoming)) return JNI_FALSE;
      continue;
    }
    if (incoming.header.type != kWireReply ||
        incoming.header.sequence != request.sequence ||
        incoming.header.status != 0) {
      debug_failure("receive-reply-status");
      return JNI_FALSE;
    }
    return (flags & kBinderFlagOneWay) != 0 || reply == nullptr ||
                   ImportParcel(env, control_fd, &incoming,
                                JavaParcel(env, reply))
               ? JNI_TRUE
               : JNI_FALSE;
  }
}

int ServeRemoteBinder(JNIEnv* env, jint control_fd, jobject local_binder) {
  if (env == nullptr || control_fd < 0 || local_binder == nullptr) return -1;
  {
    std::lock_guard<std::recursive_mutex> lock(g_wire_mutex);
    WireConnection& connection = g_wire_connections[control_fd];
    connection.dispatcher_active = true;
    connection.dispatcher_thread = std::this_thread::get_id();
    connection.local_binders.emplace(1, env->NewGlobalRef(local_binder));
    WireHeader ready;
    ready.type = kWireReady;
    if (!SendWireMessage(control_fd, ready, {}, {}, {})) return -1;
    connection.ready = true;
  }
  for (;;) {
    auto incoming = std::make_unique<WireMessage>();
    if (!ReceiveWireMessage(control_fd, incoming.get())) break;
    if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
      std::cerr << "ART Binder wire: received fd=" << control_fd
                << " type=" << incoming->header.type
                << " code=" << incoming->header.code << "\n";
    }
    std::lock_guard<std::recursive_mutex> lock(g_wire_mutex);
    if (incoming->header.type == kWireReply) {
      auto connection = g_wire_connections.find(control_fd);
      if (connection == g_wire_connections.end()) return -1;
      connection->second.pending_replies.insert_or_assign(
          incoming->header.sequence, std::move(incoming));
      g_wire_condition.notify_all();
      continue;
    }
    if (incoming->header.type != kWireTransaction ||
        !DispatchWireTransaction(env, control_fd, incoming.get())) {
      CloseRemoteBinderChannel(env, control_fd);
      return -1;
    }
  }
  CloseRemoteBinderChannel(env, control_fd);
  return 0;
}

void CloseRemoteBinderChannel(JNIEnv* env, jint control_fd) {
  std::lock_guard<std::recursive_mutex> lock(g_wire_mutex);
  auto connection = g_wire_connections.find(control_fd);
  if (connection == g_wire_connections.end()) return;
  for (const auto& [target, binder] : connection->second.local_binders) {
    static_cast<void>(target);
    env->DeleteGlobalRef(binder);
  }
  if (connection->second.class_loader != nullptr) {
    env->DeleteGlobalRef(connection->second.class_loader);
  }
  g_wire_connections.erase(connection);
  g_wire_condition.notify_all();
}

jobject ConnectSystemBinder(JNIEnv* env, const char* socket_path) {
  if (env == nullptr || env->ExceptionCheck() || socket_path == nullptr) return nullptr;
  sockaddr_un address{};
  address.sun_family = AF_UNIX;
  if (*socket_path == '\0' || std::strlen(socket_path) >= sizeof(address.sun_path)) return nullptr;
  std::memcpy(address.sun_path, socket_path, std::strlen(socket_path) + 1);
  const int fd = socket(AF_UNIX, SOCK_STREAM, 0);
  if (fd < 0) return nullptr;
  if (connect(fd, reinterpret_cast<sockaddr*>(&address), sizeof(address)) != 0) {
    close(fd);
    return nullptr;
  }
  jobject root = NewRemoteBinder(env, fd, 1);
  if (root == nullptr || env->ExceptionCheck() || !StartRemoteBinderDispatcher(env, fd)) {
    env->DeleteLocalRef(root);
    CloseRemoteBinderChannel(env, fd);
    close(fd);
    return nullptr;
  }
  // Process-lifetime manager capability. The dispatcher releases Binder refs
  // at EOF; retain the fd until process exit so stale Java endpoints cannot
  // alias a newly reused descriptor. SystemServices creates only one channel.
  return root;
}

std::string QuerySystemPackageRecord(JNIEnv* env, const char* socket_path,
                                     const char* package_name) {
  if (env == nullptr || socket_path == nullptr || *socket_path == '\0' ||
      package_name == nullptr || *package_name == '\0') {
    return {};
  }
  const int fd = socket(AF_UNIX, SOCK_STREAM, 0);
  if (fd < 0) return {};
  sockaddr_un address{};
  address.sun_family = AF_UNIX;
  if (std::strlen(socket_path) >= sizeof(address.sun_path)) {
    close(fd);
    return {};
  }
  std::memcpy(address.sun_path, socket_path, std::strlen(socket_path) + 1);
  if (connect(fd, reinterpret_cast<sockaddr*>(&address), sizeof(address)) != 0) {
    close(fd);
    return {};
  }
  jobject manager = NewRemoteBinder(env, fd, 1);
  jclass services = manager == nullptr ? nullptr
      : env->FindClass("dev/darwinart/runtime/os/SystemServices");
  jmethodID lookup = services == nullptr ? nullptr : env->GetStaticMethodID(
      services, "lookup", "(Landroid/os/IBinder;Ljava/lang/String;)Landroid/os/IBinder;");
  jstring service_name = lookup == nullptr ? nullptr
      : env->NewStringUTF("darwin.package_registry");
  jobject service = service_name == nullptr ? nullptr
      : env->CallStaticObjectMethod(services, lookup, manager, service_name);
  env->DeleteLocalRef(service_name);
  env->DeleteLocalRef(services);
  env->DeleteLocalRef(manager);
  if (service == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(service);
    CloseRemoteBinderChannel(env, fd);
    close(fd);
    return {};
  }
  jclass service_class = env->GetObjectClass(service);
  jmethodID transact = service_class == nullptr ? nullptr : env->GetMethodID(
      service_class, "transact", "(ILandroid/os/Parcel;Landroid/os/Parcel;I)Z");
  jobject data = ObtainJavaParcel(env);
  jobject reply = ObtainJavaParcel(env);
  jclass parcel_class = data == nullptr ? nullptr : env->GetObjectClass(data);
  jmethodID write_token = parcel_class == nullptr
                              ? nullptr
                              : env->GetMethodID(parcel_class, "writeInterfaceToken",
                                                 "(Ljava/lang/String;)V");
  jmethodID write_string = parcel_class == nullptr
                               ? nullptr
                               : env->GetMethodID(parcel_class, "writeString",
                                                  "(Ljava/lang/String;)V");
  jmethodID read_exception = parcel_class == nullptr
                                 ? nullptr
                                 : env->GetMethodID(parcel_class, "readException", "()V");
  jmethodID read_string = parcel_class == nullptr
                              ? nullptr
                              : env->GetMethodID(parcel_class, "readString",
                                                 "()Ljava/lang/String;");
  std::string result;
  if (data != nullptr && reply != nullptr && write_token != nullptr &&
      write_string != nullptr && read_exception != nullptr &&
      read_string != nullptr && !env->ExceptionCheck()) {
    jstring descriptor = env->NewStringUTF(
        "dev.darwinart.system.IPackageRegistry");
    jstring package = env->NewStringUTF(package_name);
    env->CallVoidMethod(data, write_token, descriptor);
    env->CallVoidMethod(data, write_string, package);
    if (transact != nullptr && !env->ExceptionCheck() &&
        env->CallBooleanMethod(service, transact, 1, data, reply, 0) == JNI_TRUE) {
      env->CallVoidMethod(reply, read_exception);
      jstring record = env->ExceptionCheck()
                           ? nullptr
                           : static_cast<jstring>(
                                 env->CallObjectMethod(reply, read_string));
      if (record != nullptr && !env->ExceptionCheck()) {
        const char* utf = env->GetStringUTFChars(record, nullptr);
        if (utf != nullptr) {
          result = utf;
          env->ReleaseStringUTFChars(record, utf);
        }
      }
      env->DeleteLocalRef(record);
    }
    env->DeleteLocalRef(package);
    env->DeleteLocalRef(descriptor);
  }
  jthrowable failure = env->ExceptionOccurred();
  if (failure != nullptr) env->ExceptionClear();
  env->DeleteLocalRef(parcel_class);
  env->DeleteLocalRef(service_class);
  env->DeleteLocalRef(service);
  if (data != nullptr) RecycleJavaParcel(env, data);
  if (reply != nullptr) RecycleJavaParcel(env, reply);
  CloseRemoteBinderChannel(env, fd);
  close(fd);
  if (failure != nullptr) {
    if (env->ExceptionCheck()) env->ExceptionClear();
    env->Throw(failure);
    env->DeleteLocalRef(failure);
  }
  return result;
}

bool SetFrameworkViewRootFocus(JNIEnv* env, jobject view_root, bool focused) {
  if (env == nullptr || view_root == nullptr) return false;
  jclass view_root_class = env->GetObjectClass(view_root);
  jfieldID receiver_field =
      view_root_class == nullptr
          ? nullptr
          : env->GetFieldID(
                view_root_class, "mInputEventReceiver",
                "Landroid/view/ViewRootImpl$WindowInputEventReceiver;");
  jobject java_receiver = receiver_field == nullptr
                              ? nullptr
                              : env->GetObjectField(view_root, receiver_field);
  jclass receiver_class = java_receiver == nullptr
                              ? nullptr
                              : env->GetObjectClass(java_receiver);
  jfieldID pointer_field =
      receiver_class == nullptr
          ? nullptr
          : env->GetFieldID(receiver_class, "mReceiverPtr", "J");
  const jlong pointer = pointer_field == nullptr
                            ? 0
                            : env->GetLongField(java_receiver, pointer_field);
  auto* receiver = reinterpret_cast<DarwinInputReceiver*>(
      static_cast<std::uintptr_t>(pointer));
  jclass weak_class =
      receiver == nullptr || receiver->weak_receiver == nullptr
          ? nullptr
          : env->GetObjectClass(receiver->weak_receiver);
  jmethodID weak_get =
      weak_class == nullptr
          ? nullptr
          : env->GetMethodID(weak_class, "get", "()Ljava/lang/Object;");
  jobject target = weak_get == nullptr
                       ? nullptr
                       : env->CallObjectMethod(receiver->weak_receiver, weak_get);
  jclass input_receiver_class =
      env->FindClass("android/view/InputEventReceiver");
  jmethodID focus =
      input_receiver_class == nullptr
          ? nullptr
          : env->GetMethodID(input_receiver_class, "onFocusEvent", "(Z)V");
  jmethodID touch_mode =
      input_receiver_class == nullptr
          ? nullptr
          : env->GetMethodID(input_receiver_class, "onTouchModeChanged", "(Z)V");
  if (target != nullptr && focus != nullptr && !env->ExceptionCheck()) {
    env->CallVoidMethod(target, focus, focused ? JNI_TRUE : JNI_FALSE);
  }
  // WindowManager.addWindow() normally returns ADD_FLAG_IN_TOUCH_MODE and
  // ViewRootImpl applies it before the first pointer packet. Our local window
  // session has no system_server result parcel, so publish the same initial
  // state through WindowInputEventReceiver during attachment. Without it the
  // first tap only moved focus into Chrome's focusable tab button and Android
  // intentionally deferred performClick until the second tap.
  if (focused && target != nullptr && touch_mode != nullptr &&
      !env->ExceptionCheck()) {
    env->CallVoidMethod(target, touch_mode, JNI_TRUE);
  }
  // WindowInputEventReceiver.onFocusEvent is the AOSP boundary. It calls
  // ViewRootImpl.windowFocusChanged(), which posts MSG_WINDOW_FOCUS_CHANGED to
  // the main queue. Do not invoke ViewRootImpl a second time here; the owner
  // Looper drains the posted message before host input is admitted.
  if (receiver != nullptr && receiver->view_root == nullptr &&
      !env->ExceptionCheck()) {
    // The fd callback is registered before the host has a ViewRoot reference.
    // Capture it at the same focus boundary that selects the channel so a
    // future receiver-owned consumer can re-enter the complete ViewRoot path.
    receiver->view_root = env->NewGlobalRef(view_root);
  }
  const bool changed = target != nullptr && focus != nullptr &&
                       (!focused || touch_mode != nullptr) &&
                       !env->ExceptionCheck();
  if (receiver != nullptr && changed) {
    UpdateInputWindowGeometry(env, receiver);
    receiver->focused = focused;
    if (focused) {
      receiver->touch_mode = true;
      SetFocusedInputChannel(receiver->channel);
    } else {
      ClearFocusedInputChannel(receiver->channel);
    }
  }
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android window focus=" << (focused ? 1 : 0)
              << " changed=" << (changed ? 1 : 0) << "\n";
  }
  if (input_receiver_class != nullptr) {
    env->DeleteLocalRef(input_receiver_class);
  }
  if (target != nullptr) env->DeleteLocalRef(target);
  if (weak_class != nullptr) env->DeleteLocalRef(weak_class);
  if (receiver_class != nullptr) env->DeleteLocalRef(receiver_class);
  if (java_receiver != nullptr) env->DeleteLocalRef(java_receiver);
  if (view_root_class != nullptr) env->DeleteLocalRef(view_root_class);
  return changed;
}

bool FocusFrameworkViewRoot(JNIEnv* env, jobject view_root) {
  return SetFrameworkViewRootFocus(env, view_root, true);
}

bool DispatchFrameworkInputEvent(JNIEnv* env, jobject view_root, jobject event,
                                 bool* handled) {
  if (env == nullptr || view_root == nullptr || event == nullptr) return false;
  if (handled != nullptr) *handled = false;
  jclass view_root_class = env->GetObjectClass(view_root);
  jfieldID receiver_field =
      view_root_class == nullptr
          ? nullptr
          : env->GetFieldID(
                view_root_class, "mInputEventReceiver",
                "Landroid/view/ViewRootImpl$WindowInputEventReceiver;");
  jobject java_receiver =
      receiver_field == nullptr
          ? nullptr
          : env->GetObjectField(view_root, receiver_field);
  jclass receiver_class = java_receiver == nullptr
                              ? nullptr
                              : env->GetObjectClass(java_receiver);
  jfieldID pointer_field =
      receiver_class == nullptr
          ? nullptr
          : env->GetFieldID(receiver_class, "mReceiverPtr", "J");
  const jlong pointer = pointer_field == nullptr
                            ? 0
                            : env->GetLongField(java_receiver, pointer_field);
  auto* receiver = reinterpret_cast<DarwinInputReceiver*>(
      static_cast<std::uintptr_t>(pointer));
  auto receiver_lease = FindInputReceiverLease(receiver);
  receiver = receiver_lease.get();
  if (receiver == nullptr || receiver->disposed.load(std::memory_order_acquire) ||
      receiver->channel == nullptr ||
      receiver->weak_receiver == nullptr || env->ExceptionCheck()) {
    env->ExceptionClear();
    env->DeleteLocalRef(receiver_class);
    env->DeleteLocalRef(java_receiver);
    env->DeleteLocalRef(view_root_class);
    return false;
  }
  receiver->active_callbacks.fetch_add(1, std::memory_order_acq_rel);
  jclass weak_class = env->GetObjectClass(receiver->weak_receiver);
  jmethodID weak_get =
      weak_class == nullptr
          ? nullptr
          : env->GetMethodID(weak_class, "get", "()Ljava/lang/Object;");
  jobject target = weak_get == nullptr
                       ? nullptr
                       : env->CallObjectMethod(receiver->weak_receiver, weak_get);
  jclass input_receiver_class = env->FindClass("android/view/InputEventReceiver");
  jclass motion_event_class = env->FindClass("android/view/MotionEvent");
  const bool is_motion =
      motion_event_class != nullptr && env->IsInstanceOf(event, motion_event_class);
  jmethodID get_source =
      motion_event_class == nullptr
          ? nullptr
          : env->GetMethodID(motion_event_class, "getSource", "()I");
  const jint source =
      is_motion && get_source != nullptr && !env->ExceptionCheck()
          ? env->CallIntMethod(event, get_source)
          : 0;
  const bool is_touchscreen = source == 0x1002;
  if (is_touchscreen && !receiver->touch_mode && target != nullptr &&
      input_receiver_class != nullptr && !env->ExceptionCheck()) {
    jmethodID touch_mode = env->GetMethodID(
        input_receiver_class, "onTouchModeChanged", "(Z)V");
    jmethodID handle_touch_mode = env->GetMethodID(
        view_root_class, "handleWindowTouchModeChanged", "()V");
    if (touch_mode != nullptr && handle_touch_mode != nullptr &&
        !env->ExceptionCheck()) {
      env->CallVoidMethod(target, touch_mode, JNI_TRUE);
      // The native InputDispatcher sends touch-mode state before the following
      // MotionEvent. Since this bridge delivers both on one owner-thread call,
      // apply the state posted by WindowInputEventReceiver before dispatching
      // the pointer; its queued MSG_WINDOW_TOUCH_MODE_CHANGED remains a benign
      // idempotent confirmation on the next Looper iteration.
      env->CallVoidMethod(view_root, handle_touch_mode);
      receiver->touch_mode = !env->ExceptionCheck();
    }
  }
  jmethodID dispatch =
      input_receiver_class == nullptr
          ? nullptr
          : env->GetMethodID(input_receiver_class, "dispatchInputEvent",
                             "(ILandroid/view/InputEvent;)V");
  if (target != nullptr && dispatch != nullptr && !env->ExceptionCheck()) {
    const jint sequence = receiver->next_sequence++;
    receiver->last_finished_sequence = 0;
    receiver->last_finished_handled = false;
    receiver->channel->last_finished_sequence.store(
        0, std::memory_order_release);
    receiver->channel->last_finished_handled.store(
        false, std::memory_order_release);
    RegisterFinishSequence(receiver->channel, sequence);
    env->CallVoidMethod(target, dispatch, sequence, event);
    bool finished = false;
    if (handled != nullptr) {
      finished = WaitForFinishedAck(receiver->channel, sequence, handled);
    } else {
      bool ignored_handled = false;
      finished = WaitForFinishedAck(receiver->channel, sequence,
                                    &ignored_handled);
    }
    // Preserve the legacy atomic fast path for receivers built against an
    // older finish implementation that has not populated the channel queue.
    if (!finished && receiver->channel->last_finished_sequence.load(
                         std::memory_order_acquire) == sequence) {
      if (handled != nullptr) {
        *handled = receiver->channel->last_finished_handled.load(
            std::memory_order_acquire);
      }
    }
  }
  const bool delivered = target != nullptr && dispatch != nullptr &&
                         !env->ExceptionCheck();
  env->DeleteLocalRef(input_receiver_class);
  env->DeleteLocalRef(motion_event_class);
  env->DeleteLocalRef(target);
  env->DeleteLocalRef(weak_class);
  env->DeleteLocalRef(receiver_class);
  env->DeleteLocalRef(java_receiver);
  env->DeleteLocalRef(view_root_class);
  const uint32_t previous =
      receiver->active_callbacks.fetch_sub(1, std::memory_order_acq_rel);
  if (previous == 1 && receiver->dispose_requested.load(std::memory_order_acquire)) {
    CleanupReceiverRefs(env, receiver);
  }
  return delivered;
}

bool RegisterFrameworkBinderNatives(JNIEnv* env) {
  if (env == nullptr || env->GetJavaVM(&g_framework_vm) != JNI_OK ||
      g_framework_vm == nullptr) {
    return false;
  }
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
  // Binder, BinderInternal, BinderProxy and Parcel share native object layouts.
  // Switch their AOSP owners as one registration unit; mixing either side's
  // Parcel pointer with the other side's Binder conversion is invalid.
  if (::register_android_os_Binder(env) < 0 ||
      android::register_android_os_Parcel(env) < 0) {
    return false;
  }
#else
  JNINativeMethod parcel_methods[] = {
      {const_cast<char*>("nativeMarkSensitive"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&ParcelMarkSensitive)},
      {const_cast<char*>("nativeMarkForBinder"),
       const_cast<char*>("(JLandroid/os/IBinder;)V"),
       reinterpret_cast<void*>(&ParcelMarkForBinder)},
      {const_cast<char*>("nativeIsForRpc"), const_cast<char*>("(J)Z"),
       reinterpret_cast<void*>(&ParcelIsForRpc)},
      {const_cast<char*>("nativeDataSize"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&ParcelDataSize)},
      {const_cast<char*>("nativeDataAvail"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&ParcelDataAvail)},
      {const_cast<char*>("nativeDataPosition"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&ParcelDataPosition)},
      {const_cast<char*>("nativeDataCapacity"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&ParcelDataCapacity)},
      {const_cast<char*>("nativeSetDataSize"), const_cast<char*>("(JI)V"),
       reinterpret_cast<void*>(&ParcelSetDataSize)},
      {const_cast<char*>("nativeSetDataPosition"), const_cast<char*>("(JI)V"),
       reinterpret_cast<void*>(&ParcelSetDataPosition)},
      {const_cast<char*>("nativeSetDataCapacity"), const_cast<char*>("(JI)V"),
       reinterpret_cast<void*>(&ParcelSetDataCapacity)},
      {const_cast<char*>("nativePushAllowFds"), const_cast<char*>("(JZ)Z"),
       reinterpret_cast<void*>(&ParcelPushAllowFds)},
      {const_cast<char*>("nativeRestoreAllowFds"), const_cast<char*>("(JZ)V"),
       reinterpret_cast<void*>(&ParcelRestoreAllowFds)},
      {const_cast<char*>("nativeWriteByteArray"), const_cast<char*>("(J[BII)V"),
       reinterpret_cast<void*>(&ParcelWriteBytes)},
      {const_cast<char*>("nativeWriteBlob"), const_cast<char*>("(J[BII)V"),
       reinterpret_cast<void*>(&ParcelWriteBytes)},
      {const_cast<char*>("nativeWriteInt"), const_cast<char*>("(JI)I"),
       reinterpret_cast<void*>(&ParcelWriteInt)},
      {const_cast<char*>("nativeWriteLong"), const_cast<char*>("(JJ)I"),
       reinterpret_cast<void*>(&ParcelWriteLong)},
      {const_cast<char*>("nativeWriteFloat"), const_cast<char*>("(JF)I"),
       reinterpret_cast<void*>(&ParcelWriteFloat)},
      {const_cast<char*>("nativeWriteDouble"), const_cast<char*>("(JD)I"),
       reinterpret_cast<void*>(&ParcelWriteDouble)},
      {const_cast<char*>("nativeSignalExceptionForError"), const_cast<char*>("(I)V"),
       reinterpret_cast<void*>(&ParcelSignalException)},
      {const_cast<char*>("nativeWriteString8"),
       const_cast<char*>("(JLjava/lang/String;)V"),
       reinterpret_cast<void*>(&ParcelWriteString)},
      {const_cast<char*>("nativeWriteString16"),
       const_cast<char*>("(JLjava/lang/String;)V"),
       reinterpret_cast<void*>(&ParcelWriteString)},
      {const_cast<char*>("nativeWriteStrongBinder"),
       const_cast<char*>("(JLandroid/os/IBinder;)V"),
       reinterpret_cast<void*>(&ParcelWriteStrongBinder)},
      {const_cast<char*>("nativeWriteFileDescriptor"),
       const_cast<char*>("(JLjava/io/FileDescriptor;)V"),
       reinterpret_cast<void*>(&ParcelWriteFileDescriptor)},
      {const_cast<char*>("nativeCreateByteArray"), const_cast<char*>("(J)[B"),
       reinterpret_cast<void*>(&ParcelCreateByteArray)},
      {const_cast<char*>("nativeReadByteArray"), const_cast<char*>("(J[BI)Z"),
       reinterpret_cast<void*>(&ParcelReadByteArray)},
      {const_cast<char*>("nativeReadBlob"), const_cast<char*>("(J)[B"),
       reinterpret_cast<void*>(&ParcelCreateByteArray)},
      {const_cast<char*>("nativeReadInt"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&ParcelReadInt)},
      {const_cast<char*>("nativeReadLong"), const_cast<char*>("(J)J"),
       reinterpret_cast<void*>(&ParcelReadLong)},
      {const_cast<char*>("nativeReadFloat"), const_cast<char*>("(J)F"),
       reinterpret_cast<void*>(&ParcelReadFloat)},
      {const_cast<char*>("nativeReadDouble"), const_cast<char*>("(J)D"),
       reinterpret_cast<void*>(&ParcelReadDouble)},
      {const_cast<char*>("nativeReadString8"),
       const_cast<char*>("(J)Ljava/lang/String;"),
       reinterpret_cast<void*>(&ParcelReadString)},
      {const_cast<char*>("nativeReadString16"),
       const_cast<char*>("(J)Ljava/lang/String;"),
       reinterpret_cast<void*>(&ParcelReadString)},
      {const_cast<char*>("nativeReadStrongBinder"),
       const_cast<char*>("(J)Landroid/os/IBinder;"),
       reinterpret_cast<void*>(&ParcelReadStrongBinder)},
      {const_cast<char*>("nativeReadFileDescriptor"),
       const_cast<char*>("(J)Ljava/io/FileDescriptor;"),
       reinterpret_cast<void*>(&ParcelReadFileDescriptor)},
      {const_cast<char*>("nativeCreate"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&ParcelCreate)},
      {const_cast<char*>("nativeFreeBuffer"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&ParcelFreeBuffer)},
      {const_cast<char*>("nativeDestroy"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&ParcelDestroy)},
      {const_cast<char*>("nativeMarshall"), const_cast<char*>("(J)[B"),
       reinterpret_cast<void*>(&ParcelMarshall)},
      {const_cast<char*>("nativeUnmarshall"), const_cast<char*>("(J[BII)V"),
       reinterpret_cast<void*>(&ParcelUnmarshall)},
      {const_cast<char*>("nativeCompareData"), const_cast<char*>("(JJ)I"),
       reinterpret_cast<void*>(&ParcelCompareData)},
      {const_cast<char*>("nativeAppendFrom"), const_cast<char*>("(JJII)V"),
       reinterpret_cast<void*>(&ParcelAppendFrom)},
      {const_cast<char*>("nativeHasFileDescriptors"), const_cast<char*>("(J)Z"),
       reinterpret_cast<void*>(&ParcelHasFileDescriptors)},
      {const_cast<char*>("nativeHasFileDescriptorsInRange"),
       const_cast<char*>("(JII)Z"),
       reinterpret_cast<void*>(&ParcelHasFileDescriptorsRange)},
      {const_cast<char*>("nativeHasBinders"), const_cast<char*>("(J)Z"),
       reinterpret_cast<void*>(&ParcelHasBinders)},
      {const_cast<char*>("nativeHasBindersInRange"), const_cast<char*>("(JII)Z"),
       reinterpret_cast<void*>(&ParcelHasBindersRange)},
      {const_cast<char*>("nativeWriteInterfaceToken"),
       const_cast<char*>("(JLjava/lang/String;)V"),
       reinterpret_cast<void*>(&ParcelWriteString)},
      {const_cast<char*>("nativeEnforceInterface"),
       const_cast<char*>("(JLjava/lang/String;)V"),
       reinterpret_cast<void*>(&ParcelEnforceInterface)},
      {const_cast<char*>("nativeReplaceCallingWorkSourceUid"),
       const_cast<char*>("(JI)Z"),
       reinterpret_cast<void*>(&ParcelReplaceWorkSource)},
      {const_cast<char*>("nativeReadCallingWorkSourceUid"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&ParcelReadWorkSource)},
      {const_cast<char*>("nativeGetOpenAshmemSize"), const_cast<char*>("(J)J"),
       reinterpret_cast<void*>(&ParcelOpenAshmemSize)},
  };
  if (!Register(env, "android/os/Parcel", parcel_methods,
                static_cast<jint>(std::size(parcel_methods)))) {
    return false;
  }

  JNINativeMethod binder_methods[] = {
      {const_cast<char*>("getNativeBBinderHolder"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&BinderGetNativeHolder)},
      {const_cast<char*>("getNativeFinalizer"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&BinderGetNativeFinalizer)},
      {const_cast<char*>("getCallingUid"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&BinderGetCallingUid)},
      {const_cast<char*>("getCallingPid"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&BinderGetCallingPid)},
      {const_cast<char*>("isDirectlyHandlingTransactionNative"),
       const_cast<char*>("()Z"),
       reinterpret_cast<void*>(&BinderIsDirectlyHandlingTransactionNative)},
      {const_cast<char*>("clearCallingIdentity"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&BinderClearCallingIdentity)},
      {const_cast<char*>("restoreCallingIdentity"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&BinderRestoreCallingIdentity)},
      {const_cast<char*>("flushPendingCommands"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&BinderFlushPendingCommands)},
      {const_cast<char*>("getThreadStrictModePolicy"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&BinderGetThreadStrictModePolicy)},
      {const_cast<char*>("setThreadStrictModePolicy"), const_cast<char*>("(I)V"),
       reinterpret_cast<void*>(&BinderSetThreadStrictModePolicy)},
  };
  if (!Register(env, "android/os/Binder", binder_methods,
                static_cast<jint>(std::size(binder_methods)))) {
    return false;
  }

  JNINativeMethod binder_internal_methods[] = {
      {const_cast<char*>("getContextObject"),
       const_cast<char*>("()Landroid/os/IBinder;"),
       reinterpret_cast<void*>(&BinderInternalGetContextObject)},
      {const_cast<char*>("handleGc"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&BinderInternalHandleGc)},
  };
  if (!Register(env, "com/android/internal/os/BinderInternal",
                binder_internal_methods,
                static_cast<jint>(std::size(binder_internal_methods)))) {
    return false;
  }
#endif

  JNINativeMethod service_manager_proxy_methods[] = {
      {const_cast<char*>("getNativeServiceManager"),
       const_cast<char*>("()Landroid/os/IBinder;"),
       reinterpret_cast<void*>(&ServiceManagerProxyGetNativeServiceManager)},
  };
  if (!Register(env, "android/os/ServiceManagerProxy",
                service_manager_proxy_methods,
                static_cast<jint>(std::size(service_manager_proxy_methods)))) {
    return false;
  }

  JNINativeMethod input_channel_methods[] = {
      {const_cast<char*>("nativeDup"), const_cast<char*>("(J)J"),
       reinterpret_cast<void*>(&InputChannelDup)},
      {const_cast<char*>("nativeGetFinalizer"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&InputChannelGetFinalizer)},
      {const_cast<char*>("nativeGetName"),
       const_cast<char*>("(J)Ljava/lang/String;"),
       reinterpret_cast<void*>(&InputChannelGetName)},
      {const_cast<char*>("nativeGetToken"),
       const_cast<char*>("(J)Landroid/os/IBinder;"),
       reinterpret_cast<void*>(&InputChannelGetToken)},
      {const_cast<char*>("nativeOpenInputChannelPair"),
       const_cast<char*>("(Ljava/lang/String;)[J"),
       reinterpret_cast<void*>(&InputChannelOpenPair)},
      {const_cast<char*>("nativeReadFromParcel"),
       const_cast<char*>("(Landroid/os/Parcel;)J"),
       reinterpret_cast<void*>(&InputChannelReadParcel)},
      {const_cast<char*>("nativeWriteToParcel"),
       const_cast<char*>("(Landroid/os/Parcel;J)V"),
       reinterpret_cast<void*>(&InputChannelWriteParcel)},
      {const_cast<char*>("nativeDispose"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&InputChannelDispose)},
  };
  if (!Register(env, "android/view/InputChannel", input_channel_methods,
                static_cast<jint>(std::size(input_channel_methods)))) {
    return false;
  }

  JNINativeMethod input_window_publisher_methods[] = {
      {const_cast<char*>("nativePublish"),
       const_cast<char*>("(Landroid/view/InputChannel;IIIIZ)V"),
       reinterpret_cast<void*>(&InputWindowPublish)},
  };
  if (!Register(env, "dev/darwinart/runtime/wm/WindowInputPublisher",
                input_window_publisher_methods,
                static_cast<jint>(std::size(input_window_publisher_methods)))) {
    return false;
  }

  JNINativeMethod input_receiver_methods[] = {
      {const_cast<char*>("nativeConsumeBatchedInputEvents"),
       const_cast<char*>("(JJ)Z"),
       reinterpret_cast<void*>(&InputReceiverConsume)},
      {const_cast<char*>("nativeDispose"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&InputReceiverDispose)},
      {const_cast<char*>("nativeDump"),
       const_cast<char*>("(JLjava/lang/String;)Ljava/lang/String;"),
       reinterpret_cast<void*>(&InputReceiverDump)},
      {const_cast<char*>("nativeFinishInputEvent"),
       const_cast<char*>("(JIZ)V"),
       reinterpret_cast<void*>(&InputReceiverFinish)},
      {const_cast<char*>("nativeInit"),
       const_cast<char*>("(Ljava/lang/ref/WeakReference;"
                         "Landroid/view/InputChannel;Landroid/os/MessageQueue;)J"),
       reinterpret_cast<void*>(&InputReceiverInit)},
      {const_cast<char*>("nativeProbablyHasInput"), const_cast<char*>("(J)Z"),
       reinterpret_cast<void*>(&InputReceiverProbablyHasInput)},
      {const_cast<char*>("nativeReportTimeline"),
       const_cast<char*>("(JIJJ)V"),
       reinterpret_cast<void*>(&InputReceiverReportTimeline)},
  };
  if (!Register(env, "android/view/InputEventReceiver", input_receiver_methods,
                static_cast<jint>(std::size(input_receiver_methods)))) {
    return false;
  }

  JNINativeMethod key_character_map_methods[] = {
      {const_cast<char*>("nativeApplyOverlay"),
       const_cast<char*>("(JLjava/lang/String;Ljava/lang/String;)V"),
       reinterpret_cast<void*>(&KeyMapApplyOverlay)},
      {const_cast<char*>("nativeDispose"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&KeyMapDispose)},
      {const_cast<char*>("nativeEquals"), const_cast<char*>("(JJ)Z"),
       reinterpret_cast<void*>(&KeyMapEquals)},
      {const_cast<char*>("nativeGetCharacter"), const_cast<char*>("(JII)C"),
       reinterpret_cast<void*>(&KeyMapGetCharacter)},
      {const_cast<char*>("nativeGetDisplayLabel"), const_cast<char*>("(JI)C"),
       reinterpret_cast<void*>(&KeyMapGetDisplayLabel)},
      {const_cast<char*>("nativeGetEvents"),
       const_cast<char*>("(J[C)[Landroid/view/KeyEvent;"),
       reinterpret_cast<void*>(&KeyMapGetEvents)},
      {const_cast<char*>("nativeGetFallbackAction"),
       const_cast<char*>("(JIILandroid/view/KeyCharacterMap$FallbackAction;)Z"),
       reinterpret_cast<void*>(&KeyMapGetFallbackAction)},
      {const_cast<char*>("nativeGetKeyboardType"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&KeyMapGetKeyboardType)},
      {const_cast<char*>("nativeGetMappedKey"), const_cast<char*>("(JI)I"),
       reinterpret_cast<void*>(&KeyMapGetMappedKey)},
      {const_cast<char*>("nativeGetMatch"), const_cast<char*>("(JI[CI)C"),
       reinterpret_cast<void*>(&KeyMapGetMatch)},
      {const_cast<char*>("nativeGetNumber"), const_cast<char*>("(JI)C"),
       reinterpret_cast<void*>(&KeyMapGetNumber)},
      {const_cast<char*>("nativeObtainEmptyKeyCharacterMap"),
       const_cast<char*>("(I)Landroid/view/KeyCharacterMap;"),
       reinterpret_cast<void*>(&KeyMapObtainEmpty)},
      {const_cast<char*>("nativeReadFromParcel"),
       const_cast<char*>("(Landroid/os/Parcel;)J"),
       reinterpret_cast<void*>(&KeyMapReadFromParcel)},
      {const_cast<char*>("nativeWriteToParcel"),
       const_cast<char*>("(JLandroid/os/Parcel;)V"),
       reinterpret_cast<void*>(&KeyMapWriteToParcel)},
  };
  if (!Register(env, "android/view/KeyCharacterMap",
                key_character_map_methods,
                static_cast<jint>(std::size(key_character_map_methods)))) {
    return false;
  }
  JNINativeMethod key_event_methods[] = {
      {const_cast<char*>("nativeKeyCodeFromString"),
       const_cast<char*>("(Ljava/lang/String;)I"),
       reinterpret_cast<void*>(&KeyEventCodeFromString)},
      {const_cast<char*>("nativeKeyCodeToString"),
       const_cast<char*>("(I)Ljava/lang/String;"),
       reinterpret_cast<void*>(&KeyEventCodeToString)},
      {const_cast<char*>("nativeNextId"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&KeyEventNextId)},
  };
  return Register(env, "android/view/KeyEvent", key_event_methods,
                  static_cast<jint>(std::size(key_event_methods)));
}

}  // namespace darwin_art
