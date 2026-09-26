#include "darwin_framework_natives.h"
#include "darwin_binder_wire.h"
#include "diagnostics/view_resources.h"
#include "binder/calling_identity.h"
#include "binder/context_manager.h"
#include "binder/peer_credentials.h"
#include "binder/process_registry.h"
#include "binder/wire_dispatch_policy.h"
#include "binder/wire_connection_registry.h"
#include "binder/remote_binder_identity_jni.h"
#include "darwin_android_platform.h"
#include "../runtime/framework/input/channel_owner.h"
#include "../runtime/framework/input/key_character_map_jni.h"
#include "../runtime/framework/wm/window_input_publisher_jni.h"
#include "darwin_android_time.h"
#include "../tools/bionic-socket-broker-adapter/include/darwin_art_bionic_socket_broker.h"

#include <cstdint>
#include <algorithm>
#include <atomic>
#include <bit>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <future>
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

namespace android {
// libutils misc.cpp: runs the property change callbacks.
void report_sysprop_change();
}  // namespace android

namespace {

// IBinder.SYSPROPS_TRANSACTION ('_SPR').
constexpr uint32_t kSyspropsTransaction =
    ('_' << 24) | ('S' << 16) | ('P' << 8) | 'R';

JavaVM* g_framework_vm = nullptr;

struct DarwinParcel {
  std::vector<uint8_t> data;
  size_t position = 0;
  bool allow_fds = true;
  std::vector<jobject> binders;
  std::vector<int> file_descriptors;
};

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

// libbinder waitForService: block until the name is published. Publication
// is the Java service directory, so poll its checkService lookup.
jobject ServiceManagerWaitForServiceNative(JNIEnv* env, jclass service_manager,
                                           jstring name) {
  jmethodID check = env->GetStaticMethodID(service_manager, "checkService",
                                           "(Ljava/lang/String;)Landroid/os/IBinder;");
  if (check == nullptr || name == nullptr) return nullptr;
  constexpr int kPollMillis = 100;
  for (int waited = 0;; waited += kPollMillis) {
    jobject service = env->CallStaticObjectMethod(service_manager, check, name);
    if (service != nullptr || env->ExceptionCheck()) return service;
    if (waited > 0 && waited % 5000 == 0) {
      const char* text = env->GetStringUTFChars(name, nullptr);
      std::cerr << "ServiceManager: waited " << waited << " ms for service "
                << (text == nullptr ? "?" : text) << "\n";
      if (text != nullptr) env->ReleaseStringUTFChars(name, text);
    }
    usleep(kPollMillis * 1000);
  }
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
  uint32_t next_sequence = 1;
  uint32_t next_local_target = 2;
  bool ready = false;
  bool dispatcher_active = false;
  std::thread::id dispatcher_thread;
  jobject class_loader = nullptr;
  std::unordered_map<uint32_t, jobject> local_binders;
  std::unordered_map<uint32_t, std::unique_ptr<WireMessage>> pending_replies;
};

using WireRegistry = darwin_art::binder::WireConnectionRegistry<WireConnection>;
using WireHandle = WireRegistry::Handle;
using WireTransaction = WireRegistry::Transaction;
WireRegistry g_wire_registry;

// Only genuine channel owners use this establishment port. Parcel imports,
// transactions and Binder-object registration must find an existing owner.
WireHandle EstablishWireConnection(WireTransaction& transaction, int fd) {
  if (auto existing = transaction.FindEstablished(fd))
    return existing->lifetime->Live() ? existing : nullptr;
  return transaction.CreateOwned(fd, std::make_shared<WireConnection>());
}

bool WireOwnerCurrent(WireTransaction& transaction, const WireHandle& owner) {
  return owner != nullptr && owner->lifetime->Live() &&
      transaction.FindExact(owner->fd, owner->Generation()) == owner;
}

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
    if (darwin_art_android_shared_memory_get_guest_info(
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

uint32_t RegisterLocalBinder(JNIEnv* env, const WireHandle& owner, jobject binder) {
  if (owner == nullptr || !owner->lifetime->Live()) return 0;
  WireConnection& connection = *owner->payload;
  for (const auto& [target, candidate] : connection.local_binders) {
    if (env->IsSameObject(candidate, binder)) return target;
  }
  const uint32_t target = connection.next_local_target++;
  jobject pin = env->NewGlobalRef(binder);
  if (pin == nullptr) return 0;
  try {
    connection.local_binders.emplace(target, pin);
  } catch (...) {
    env->DeleteGlobalRef(pin);
    throw;
  }
  return target;
}

enum class RemoteTargetKind { kLocal, kReturnHome, kForward, kStale };
jclass RemoteBinderClass(JNIEnv* env);
RemoteTargetKind RemoteTarget(JNIEnv* env, WireTransaction& transaction,
    const WireHandle& owner, jobject binder, uint32_t* target) {
  // Field shape is not Binder identity. Ordinary local Binder implementations
  // may legitimately have fields with these names.
  jclass binder_class = RemoteBinderClass(env);
  if (binder_class == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(binder_class);
    return RemoteTargetKind::kStale;
  }
  if (!WireOwnerCurrent(transaction, owner)) {
    env->DeleteLocalRef(binder_class);
    return RemoteTargetKind::kStale;
  }
  const auto identity = darwin_art::binder::ReadRemoteBinderIdentity(
      env, binder, binder_class);
  env->DeleteLocalRef(binder_class);
  // JNI lookup may initialize a class and reenter channel teardown. Decoding
  // fields is not an authority grant; only this original wire owner can decide.
  if (!WireOwnerCurrent(transaction, owner) || env->ExceptionCheck() ||
      identity.kind == darwin_art::binder::RemoteBinderIdentityKind::kInvalid) {
    return RemoteTargetKind::kStale;
  }
  if (identity.kind == darwin_art::binder::RemoteBinderIdentityKind::kLocal)
    return RemoteTargetKind::kLocal;
  const auto source = transaction.FindExact(
      identity.control_fd, identity.channel_generation);
  const bool live = source != nullptr && source->lifetime->Live() && !env->ExceptionCheck();
  const bool matches = live && source == owner;
  if (matches) {
    *target = identity.target_id;
  }
  return !live ? RemoteTargetKind::kStale :
      matches ? RemoteTargetKind::kReturnHome : RemoteTargetKind::kForward;
}

bool ExportParcel(JNIEnv* env, const WireHandle& owner, DarwinParcel* parcel,
                  WireHeader* header, std::vector<WireBinder>* binders,
                  std::vector<uint8_t>* data,
                  std::vector<int>* descriptors) {
  if (parcel == nullptr || header == nullptr || binders == nullptr ||
      data == nullptr || descriptors == nullptr ||
      parcel->data.size() > std::numeric_limits<uint32_t>::max()) {
    return false;
  }
  auto transaction = g_wire_registry.Lock();
  if (!WireOwnerCurrent(transaction, owner)) return false;
  *data = parcel->data;
  binders->reserve(parcel->binders.size());
  for (jobject binder : parcel->binders) {
    uint32_t target = 0;
    const auto kind = RemoteTarget(env, transaction, owner, binder, &target);
    if (kind == RemoteTargetKind::kStale || env->ExceptionCheck()) return false;
    if (kind == RemoteTargetKind::kReturnHome) {
      binders->push_back({target, kWireBinderReturnsHome});
    } else {
      target = RegisterLocalBinder(env, owner, binder);
      if (target == 0 || env->ExceptionCheck()) return false;
      binders->push_back({target, 0});
    }
  }
  *descriptors = parcel->file_descriptors;
  header->data_size = static_cast<uint32_t>(data->size());
  header->binder_count = static_cast<uint32_t>(binders->size());
  header->fd_count = static_cast<uint32_t>(descriptors->size());
  return !env->ExceptionCheck();
}

jclass RemoteBinderClass(JNIEnv* env) {
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
  return remote_class;
}

jobject NewRemoteBinder(JNIEnv* env, const WireHandle& owner, uint32_t target) {
  if (owner == nullptr || !owner->lifetime->Live()) return nullptr;
  auto transaction = g_wire_registry.Lock();
  if (!WireOwnerCurrent(transaction, owner)) return nullptr;
  jclass remote_class = RemoteBinderClass(env);
  jmethodID constructor =
      !darwin_art::RegisterRemoteBinderNatives(env, remote_class)
          ? nullptr
          : env->GetMethodID(remote_class, "<init>", "(IIJ)V");
  jobject result = constructor == nullptr || !WireOwnerCurrent(transaction, owner)
                       ? nullptr
                       : env->NewObject(remote_class, constructor, owner->fd,
                                        static_cast<jint>(target),
                                        std::bit_cast<jlong>(owner->Generation()));
  env->DeleteLocalRef(remote_class);
  if (!WireOwnerCurrent(transaction, owner)) {
    env->DeleteLocalRef(result);
    return nullptr;
  }
  return result;
}

bool ImportParcel(JNIEnv* env, const WireHandle& owner, WireMessage* message,
                  DarwinParcel* parcel) {
  if (message == nullptr || parcel == nullptr) return false;
  auto transaction = g_wire_registry.Lock();
  if (!WireOwnerCurrent(transaction, owner)) return false;
  const int fd = owner->fd;
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
      auto& connection = *owner->payload;
      auto local = connection.local_binders.find(wire_binder.target);
      if (local != connection.local_binders.end()) {
        binder = env->NewLocalRef(local->second);
      }
    } else {
      binder = NewRemoteBinder(env, owner, wire_binder.target);
    }
    if (binder == nullptr || env->ExceptionCheck() ||
        !WireOwnerCurrent(transaction, owner)) {
      env->DeleteLocalRef(binder);
      return false;
    }
    jobject pin = env->NewGlobalRef(binder);
    env->DeleteLocalRef(binder);
    if (pin == nullptr || env->ExceptionCheck()) return false;
    try {
      parcel->binders.push_back(pin);
    } catch (...) {
      env->DeleteGlobalRef(pin);
      throw;
    }
  }
  parcel->file_descriptors = std::move(message->file_descriptors);
  message->file_descriptors.clear();
  return !env->ExceptionCheck() && WireOwnerCurrent(transaction, owner);
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

bool DispatchWireTransaction(JNIEnv* env, const WireHandle& owner, WireMessage* request) {
  auto transaction = g_wire_registry.Lock();
  if (!WireOwnerCurrent(transaction, owner)) return false;
  const int fd = owner->fd;
  if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
    std::cerr << "ART Binder wire: dispatch fd=" << fd
              << " target=" << request->header.target
              << " code=" << request->header.code
              << " flags=" << request->header.flags << "\n";
  }
  auto& connection = *owner->payload;
  auto local = connection.local_binders.find(request->header.target);
  if (local == connection.local_binders.end()) {
    return false;
  }
  // Class loading and Parcel import may reenter teardown. Keep the original
  // target as a local JNI root rather than retaining an invalidatable iterator.
  jobject local_target = env->NewLocalRef(local->second);
  if (local_target == nullptr) return false;
  if (!connection.peer_checked) {
    darwin_art::binder::PeerCredentials peer{};
    if (darwin_art::binder::ReadPeerCredentials(fd, &peer)) {
      connection.peer = peer;
      connection.peer_android_uid = darwin_art_runtime_registered_process_uid(peer.pid);
    }
    connection.peer_checked = true;
  }
  // Unknown/inherited channels must not impersonate this server. Android UID
  // requires package registry resolution; Darwin's host UID is not that UID.
  const auto& peer = connection.peer;
  const int32_t caller_pid = (request->header.flags & kBinderFlagOneWay) != 0 || !peer
                                ? 0 : static_cast<int32_t>(peer->pid);
  darwin_art::binder::IncomingIdentity caller(caller_pid, connection.peer_android_uid);
  jobject data = ObtainJavaParcel(env);
  jobject reply = ObtainJavaParcel(env);
  DarwinParcel* data_native = JavaParcel(env, data);
  DarwinParcel* reply_native = JavaParcel(env, reply);
  const bool request_imported =
      data != nullptr && reply != nullptr && data_native != nullptr &&
      reply_native != nullptr && ImportParcel(env, owner, request, data_native);
  if (!request_imported) {
    RecycleJavaParcel(env, reply);
    RecycleJavaParcel(env, data);
    env->DeleteLocalRef(local_target);
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
        transact == nullptr || !WireOwnerCurrent(transaction, owner)
            ? JNI_FALSE
            : env->CallBooleanMethod(
                  local_target, transact,
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
  // JavaBBinder::onTransact: a SYSPROPS_TRANSACTION poke (SystemPropPoker)
  // also runs BBinder's handling, which reports the property change to the
  // callbacks SystemProperties.addChangeCallback registered.
  if (request->header.code == kSyspropsTransaction && !env->ExceptionCheck()) {
    android::report_sysprop_change();
    transaction_handled = true;
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
    transaction_handled = ExportParcel(env, owner, reply_native, &response,
                                       &binders, &bytes, &descriptors);
    response.status = darwin_art::binder::DecideWireDispatch(
                          transaction_handled, false)
                          .response_status;
  }
  const bool response_sent =
      WireOwnerCurrent(transaction, owner) &&
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
  env->DeleteLocalRef(local_target);
  return decision.keep_channel;
}

void RetireWireConnection(JNIEnv* env, WireTransaction& transaction,
                          const WireHandle& owner) {
  if (owner == nullptr || !transaction.RetireExact(owner)) return;
  // Resource owners must retire under this lock before closing the original.
  // Wake readers of the same socket, never a reused descriptor's successor.
  (void)shutdown(owner->fd, SHUT_RDWR);
  // RetireExact seals admission before removing the exact catalog entry.
  // JNI/resource cleanup does not itself grant or imply input quiescence.
  transaction.NotifyAll();
  if (env == nullptr) {
    std::cerr << "ART Binder wire: sealed channel without JNI cleanup fd="
              << owner->fd << " generation=" << owner->Generation() << "\n";
    return;
  }
  auto& connection = *owner->payload;
  for (const auto& [target, binder] : connection.local_binders) {
    static_cast<void>(target);
    env->DeleteGlobalRef(binder);
  }
  connection.local_binders.clear();
  if (connection.class_loader != nullptr) {
    env->DeleteGlobalRef(connection.class_loader);
    connection.class_loader = nullptr;
  }
  connection.pending_replies.clear();
  transaction.NotifyAll();
}

void CloseRemoteBinderChannelGeneration(JNIEnv* env, int control_fd,
                                        uint64_t generation) {
  auto transaction = g_wire_registry.Lock();
  RetireWireConnection(env, transaction, transaction.FindExact(control_fd, generation));
}

class ScopedWireReader final {
 public:
  explicit ScopedWireReader(int fd = -1) : fd_(fd) {}
  ~ScopedWireReader() { if (fd_ >= 0) close(fd_); }
  ScopedWireReader(const ScopedWireReader&) = delete;
  ScopedWireReader& operator=(const ScopedWireReader&) = delete;
  bool Duplicate(int original) noexcept {
    if (fd_ >= 0) return false;
    fd_ = fcntl(original, F_DUPFD_CLOEXEC, 0);
    return fd_ >= 0;
  }
  int Fd() const noexcept { return fd_; }
 private:
  int fd_;
};

struct WireDispatcherStartup final {
  std::promise<bool> attached;
  std::promise<bool> run;
  std::future<bool> run_permission = run.get_future();
};

void RunRemoteBinderDispatcher(JavaVM* vm, int fd, uint64_t generation,
    int reader_fd, std::shared_ptr<WireDispatcherStartup> startup) {
  ScopedWireReader reader(reader_fd);
  JNIEnv* env = nullptr;
  if (vm == nullptr || vm->AttachCurrentThread(&env, nullptr) != JNI_OK ||
      env == nullptr) {
    // Creator owns JNI cleanup. Report before any registry lock acquisition.
    startup->attached.set_value(false);
    return;
  }
  startup->attached.set_value(true);
  // Detach failure/canceled startup can join without any registry dependency,
  // even if the creator is inside a recursively dispatched Java callback.
  if (!startup->run_permission.get()) {
    vm->DetachCurrentThread();
    return;
  }
  startup.reset();
  jobject class_loader = nullptr;
  {
    auto transaction = g_wire_registry.Lock();
    auto owner = transaction.FindExact(fd, generation);
    if (owner == nullptr || !owner->lifetime->Live()) {
      vm->DetachCurrentThread();
      return;
    }
    owner->payload->dispatcher_thread = std::this_thread::get_id();
    class_loader = env->NewLocalRef(owner->payload->class_loader);
    transaction.NotifyAll();
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
    if (!ReceiveWireMessage(reader_fd, incoming.get())) {
      if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
        std::cerr << "ART Binder wire: dispatcher closing fd=" << fd
                  << " generation=" << generation << " reason=receive\n";
      }
      break;
    }
    auto transaction = g_wire_registry.Lock();
    auto owner = transaction.FindExact(fd, generation);
    if (owner == nullptr || !owner->lifetime->Live()) {
      break;
    }
    if (incoming->header.type == kWireReady) {
      owner->payload->ready = true;
      transaction.NotifyAll();
      continue;
    }
    if (incoming->header.type == kWireReply) {
      owner->payload->pending_replies.insert_or_assign(
          incoming->header.sequence, std::move(incoming));
      transaction.NotifyAll();
      continue;
    }
    if (incoming->header.type != kWireTransaction ||
        !DispatchWireTransaction(env, owner, incoming.get())) {
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

bool ReadInputChannelParcel(
    JNIEnv* env, jobject parcel_object,
    darwin_art::input::InputChannelParcelData* out) {
  if (env == nullptr || parcel_object == nullptr || out == nullptr) return false;
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
  android::Parcel* parcel = android::parcelForJavaObject(env, parcel_object);
  if (parcel == nullptr) return false;
  out->initialized = parcel->readInt32() == 1;
  if (!out->initialized) return true;
  out->token = android::javaObjectForIBinder(env, parcel->readStrongBinder());
  const android::String8 name_utf8(parcel->readString16());
  out->name = env->NewStringUTF(name_utf8.c_str());
  const int borrowed_endpoint_fd = parcel->readFileDescriptor();
  out->endpoint_fd = borrowed_endpoint_fd < 0
      ? -1
      : darwin_art_bionic_socket_broker_dup(borrowed_endpoint_fd);
#else
  DarwinParcel* parcel = JavaParcel(env, parcel_object);
  if (parcel == nullptr) return false;
  const jlong p = reinterpret_cast<jlong>(parcel);
  out->initialized = ParcelReadInt(p) == 1;
  if (!out->initialized) return true;
  out->token = ParcelReadStrongBinder(env, nullptr, p);
  out->name = ParcelReadString(env, nullptr, p);
  out->endpoint_fd = ParcelReadGuestFileDescriptor(p);
#endif
  return true;
}

bool WriteInputChannelParcel(
    JNIEnv* env, jobject parcel_object, bool initialized, jobject token,
    const char* name, int endpoint_fd) {
  if (env == nullptr || parcel_object == nullptr) return false;
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
  android::Parcel* parcel = android::parcelForJavaObject(env, parcel_object);
  if (parcel == nullptr) return false;
  android::status_t status = parcel->writeInt32(initialized ? 1 : 0);
  if (initialized && status == android::OK)
    status = parcel->writeStrongBinder(android::ibinderForJavaObject(env, token));
  if (initialized && status == android::OK)
    status = parcel->writeString16(android::String16(name == nullptr ? "" : name));
  if (initialized && status == android::OK)
    status = parcel->writeDupFileDescriptor(endpoint_fd);
  if (status != android::OK && initialized && !env->ExceptionCheck()) {
    jclass exception = env->FindClass("java/lang/IllegalStateException");
    if (exception != nullptr) {
      env->ThrowNew(exception, "Unable to write framework InputChannel");
      env->DeleteLocalRef(exception);
    }
    return false;
  }
  return status == android::OK;
#else
  DarwinParcel* parcel = JavaParcel(env, parcel_object);
  if (parcel == nullptr) return false;
  const jlong p = reinterpret_cast<jlong>(parcel);
  ParcelWriteInt(p, initialized ? 1 : 0);
  if (!initialized) return true;
  ParcelWriteStrongBinder(env, nullptr, p, token);
  jstring name_string = name == nullptr ? nullptr : env->NewStringUTF(name);
  ParcelWriteString(env, nullptr, p, name_string);
  env->DeleteLocalRef(name_string);
  return ParcelWriteGuestFileDescriptor(p, endpoint_fd);
#endif
}


}  // namespace

namespace darwin_art {

bool StartRemoteBinderDispatcherForOwner(JNIEnv* env, const WireHandle& owner) {
  const jint control_fd = owner == nullptr ? -1 : owner->fd;
  if (env == nullptr || control_fd < 0) return false;
  JavaVM* vm = nullptr;
  if (env->GetJavaVM(&vm) != JNI_OK || vm == nullptr) {
    if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr)
      std::cerr << "ART Binder wire: dispatcher GetJavaVM failure pid="
                << getpid() << " fd=" << control_fd << "\n";
    auto transaction = g_wire_registry.Lock();
    RetireWireConnection(env, transaction, owner);
    return false;
  }
  std::shared_ptr<WireDispatcherStartup> startup;
  std::future<bool> attached;
  try {
    startup = std::make_shared<WireDispatcherStartup>();
    attached = startup->attached.get_future();
  } catch (...) {
    auto transaction = g_wire_registry.Lock();
    RetireWireConnection(env, transaction, owner);
    return false;
  }
  int reader_fd = -1;
  {
    auto transaction = g_wire_registry.Lock();
    if (!WireOwnerCurrent(transaction, owner)) return false;
    WireConnection& connection = *owner->payload;
    if (connection.dispatcher_active) return true;
    reader_fd = fcntl(control_fd, F_DUPFD_CLOEXEC, 0);
    if (reader_fd < 0) {
      RetireWireConnection(env, transaction, owner);
      return false;
    }
    connection.dispatcher_active = true;
  }
  const uint64_t generation = owner->Generation();
  std::thread worker;
  try {
    worker = std::thread(RunRemoteBinderDispatcher, vm, control_fd, generation,
                         reader_fd, startup);
  } catch (...) {
    close(reader_fd);
    if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr)
      std::cerr << "ART Binder wire: dispatcher thread failure pid="
                << getpid() << " fd=" << control_fd
                << " generation=" << generation << "\n";
    auto transaction = g_wire_registry.Lock();
    RetireWireConnection(env, transaction, owner);
    return false;
  }
  // Attach result is published before all registry/Java work on the worker.
  if (!attached.get()) {
    worker.join();
    auto transaction = g_wire_registry.Lock();
    RetireWireConnection(env, transaction, owner);
    return false;
  }
  bool current = false;
  {
    auto transaction = g_wire_registry.Lock();
    current = WireOwnerCurrent(transaction, owner);
  }
  if (!current) {
    startup->run.set_value(false);
    worker.join();
    return false;
  }
  try {
    worker.detach();
  } catch (...) {
    startup->run.set_value(false);
    worker.join();
    auto transaction = g_wire_registry.Lock();
    RetireWireConnection(env, transaction, owner);
    return false;
  }
  startup->run.set_value(true);
  return true;
}

bool StartRemoteBinderDispatcher(JNIEnv* env, jint control_fd) {
  if (env == nullptr || control_fd < 0) return false;
  WireHandle owner;
  {
    auto transaction = g_wire_registry.Lock();
    owner = EstablishWireConnection(transaction, control_fd);
  }
  return StartRemoteBinderDispatcherForOwner(env, owner);
}

bool StartServingRemoteBinder(JNIEnv* env, jint control_fd,
                              jobject local_binder) {
  if (env == nullptr || control_fd < 0 || local_binder == nullptr) return false;
  WireHandle owner;
  {
    auto transaction = g_wire_registry.Lock();
    owner = EstablishWireConnection(transaction, control_fd);
    if (owner == nullptr) return false;
    WireConnection& connection = *owner->payload;
    if (!connection.local_binders.contains(1)) {
      jobject published = env->NewGlobalRef(local_binder);
      if (published == nullptr) {
        if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr)
          std::cerr << "ART Binder wire: NewGlobalRef failure pid=" << getpid()
                    << " fd=" << control_fd
                    << " generation=" << owner->Generation() << "\n";
        RetireWireConnection(env, transaction, owner);
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
      if (loader != nullptr && !env->ExceptionCheck() &&
          WireOwnerCurrent(transaction, owner)) {
        connection.class_loader = env->NewGlobalRef(loader);
      }
      env->DeleteLocalRef(loader);
      env->DeleteLocalRef(class_class);
      env->DeleteLocalRef(binder_class);
      if (env->ExceptionCheck()) env->ExceptionClear();
    }
    WireHeader ready;
    ready.type = kWireReady;
    if (!WireOwnerCurrent(transaction, owner) ||
        !SendWireMessage(control_fd, ready, {}, {}, {})) {
      if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr)
        std::cerr << "ART Binder wire: ready send failure pid=" << getpid()
                  << " fd=" << control_fd
                  << " generation=" << owner->Generation() << "\n";
      RetireWireConnection(env, transaction, owner);
      return false;
    }
    connection.ready = true;
  }
  const bool dispatcher = StartRemoteBinderDispatcherForOwner(env, owner);
  if (!dispatcher && std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr)
    std::cerr << "ART Binder wire: dispatcher start failure pid=" << getpid()
              << " fd=" << control_fd << "\n";
  return dispatcher;
}

bool SendServiceBindIntent(JNIEnv* env, jint control_fd, jobject intent) {
  if (env == nullptr || control_fd < 0 || intent == nullptr) return false;
  auto transaction = g_wire_registry.Lock();
  auto owner = EstablishWireConnection(transaction, control_fd);
  if (owner == nullptr) return false;
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
      ExportParcel(env, owner, JavaParcel(env, parcel), &message, &binders,
                   &bytes, &descriptors) &&
      WireOwnerCurrent(transaction, owner) &&
      SendWireMessage(control_fd, message, binders, bytes, descriptors);
  RecycleJavaParcel(env, parcel);
  return success && !env->ExceptionCheck();
}

jobject ReceiveServiceBindIntent(JNIEnv* env, jint control_fd) {
  if (env == nullptr || control_fd < 0) return nullptr;
  auto transaction = g_wire_registry.Lock();
  auto owner = EstablishWireConnection(transaction, control_fd);
  if (owner == nullptr) return nullptr;
  WireMessage message;
  jobject parcel = nullptr;
  jobject intent = nullptr;
  if (!ReceiveWireMessage(control_fd, &message) ||
      message.header.type != kWireServiceBindIntent ||
      (parcel = ObtainJavaParcel(env)) == nullptr ||
      !ImportParcel(env, owner, &message, JavaParcel(env, parcel))) {
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

jboolean TransactRemoteBinder(JNIEnv* env, jint control_fd, uint64_t generation,
                              jint target_id,
                              jint code, jobject data, jobject reply,
                              jint flags) {
  if (env == nullptr || control_fd < 0 || target_id <= 0 || data == nullptr) {
    return JNI_FALSE;
  }
  auto transaction = g_wire_registry.Lock();
  auto owner = transaction.FindExact(control_fd, generation);
  if (!WireOwnerCurrent(transaction, owner)) return JNI_FALSE;
  // A recursive JNI callback cannot wait while its caller still holds another
  // recursive level. Reject that unsupported wait visibly rather than deadlock.
  const auto wait = [&](auto predicate) {
    try {
      transaction.Wait(predicate);
      return true;
    } catch (const std::logic_error&) {
      jclass exception = env->FindClass("java/lang/IllegalStateException");
      if (exception != nullptr) {
        env->ThrowNew(exception, "Recursive Binder transport wait is unsupported");
        env->DeleteLocalRef(exception);
      }
      return false;
    }
  };
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
  WireConnection& connection = *owner->payload;
  if (!connection.ready) {
    if (connection.dispatcher_active &&
        connection.dispatcher_thread != std::this_thread::get_id()) {
      if (!wait([&] {
        return !WireOwnerCurrent(transaction, owner) || connection.ready;
      })) return JNI_FALSE;
      if (!WireOwnerCurrent(transaction, owner) || !connection.ready) {
        debug_failure("wait-ready-dispatcher");
        return JNI_FALSE;
      }
    } else {
      WireMessage ready;
      if (!ReceiveWireMessage(control_fd, &ready) ||
          !WireOwnerCurrent(transaction, owner) ||
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
  if (!ExportParcel(env, owner, JavaParcel(env, data), &request, &binders,
                    &bytes, &descriptors) ||
      !WireOwnerCurrent(transaction, owner) ||
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
    if (!wait([&] {
      return !WireOwnerCurrent(transaction, owner) ||
             connection.pending_replies.contains(sequence);
    })) return JNI_FALSE;
    if (!WireOwnerCurrent(transaction, owner)) {
      debug_failure("wait-reply-channel-closed");
      return JNI_FALSE;
    }
    auto pending = connection.pending_replies.find(sequence);
    if (pending == connection.pending_replies.end()) {
      debug_failure("wait-reply-missing");
      return JNI_FALSE;
    }
    std::unique_ptr<WireMessage> incoming = std::move(pending->second);
    connection.pending_replies.erase(pending);
    if (incoming->header.type != kWireReply ||
        incoming->header.status != 0) {
      debug_failure("wait-reply-status");
      return JNI_FALSE;
    }
    return (flags & kBinderFlagOneWay) != 0 || reply == nullptr ||
                   ImportParcel(env, owner, incoming.get(),
                                JavaParcel(env, reply))
               ? JNI_TRUE
               : JNI_FALSE;
  }
  // Wait for the transport ACK even though Java observes one-way semantics.
  // Nested callback transactions are dispatched by the loop before the ACK.
  for (;;) {
    WireMessage incoming;
    if (!WireOwnerCurrent(transaction, owner) ||
        !ReceiveWireMessage(control_fd, &incoming)) {
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
      if (!DispatchWireTransaction(env, owner, &incoming)) return JNI_FALSE;
      continue;
    }
    if (!WireOwnerCurrent(transaction, owner) ||
        incoming.header.type != kWireReply ||
        incoming.header.sequence != request.sequence ||
        incoming.header.status != 0) {
      debug_failure("receive-reply-status");
      return JNI_FALSE;
    }
    return (flags & kBinderFlagOneWay) != 0 || reply == nullptr ||
                   ImportParcel(env, owner, &incoming,
                                JavaParcel(env, reply))
               ? JNI_TRUE
               : JNI_FALSE;
  }
}

int ServeRemoteBinder(JNIEnv* env, jint control_fd, jobject local_binder) {
  if (env == nullptr || control_fd < 0 || local_binder == nullptr) return -1;
  WireHandle owner;
  ScopedWireReader reader;
  {
    auto transaction = g_wire_registry.Lock();
    owner = EstablishWireConnection(transaction, control_fd);
    if (owner == nullptr) return -1;
    WireConnection& connection = *owner->payload;
    if (!reader.Duplicate(control_fd)) {
      RetireWireConnection(env, transaction, owner);
      return -1;
    }
    connection.dispatcher_active = true;
    connection.dispatcher_thread = std::this_thread::get_id();
    if (!connection.local_binders.contains(1)) {
      jobject published = env->NewGlobalRef(local_binder);
      if (published == nullptr) {
        RetireWireConnection(env, transaction, owner);
        return -1;
      }
      connection.local_binders.emplace(1, published);
    }
    WireHeader ready;
    ready.type = kWireReady;
    if (!SendWireMessage(control_fd, ready, {}, {}, {})) {
      RetireWireConnection(env, transaction, owner);
      return -1;
    }
    connection.ready = true;
  }
  for (;;) {
    auto incoming = std::make_unique<WireMessage>();
    if (!ReceiveWireMessage(reader.Fd(), incoming.get())) break;
    if (std::getenv("DARWIN_ART_DEBUG_BINDER") != nullptr) {
      std::cerr << "ART Binder wire: received fd=" << control_fd
                << " type=" << incoming->header.type
                << " code=" << incoming->header.code << "\n";
    }
    auto transaction = g_wire_registry.Lock();
    if (!WireOwnerCurrent(transaction, owner)) return -1;
    if (incoming->header.type == kWireReply) {
      owner->payload->pending_replies.insert_or_assign(
          incoming->header.sequence, std::move(incoming));
      transaction.NotifyAll();
      continue;
    }
    if (incoming->header.type != kWireTransaction ||
        !DispatchWireTransaction(env, owner, incoming.get())) {
      RetireWireConnection(env, transaction, owner);
      return -1;
    }
  }
  CloseRemoteBinderChannelGeneration(env, control_fd, owner->Generation());
  return 0;
}

void CloseRemoteBinderChannel(JNIEnv* env, jint control_fd) {
  auto transaction = g_wire_registry.Lock();
  RetireWireConnection(env, transaction, transaction.FindEstablished(control_fd));
}

std::shared_ptr<binder::WireChannelLifetime>
CaptureEstablishedRemoteBinderChannelLifetime(jint control_fd) {
  auto transaction = g_wire_registry.Lock();
  auto owner = transaction.FindEstablished(control_fd);
  return WireOwnerCurrent(transaction, owner) ? owner->lifetime : nullptr;
}

std::shared_ptr<binder::WireChannelLifetime>
FindRemoteBinderChannelLifetime(jint control_fd, uint64_t generation) {
  auto transaction = g_wire_registry.Lock();
  auto owner = transaction.FindExact(control_fd, generation);
  return WireOwnerCurrent(transaction, owner) ? owner->lifetime : nullptr;
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
  WireHandle owner;
  {
    auto transaction = g_wire_registry.Lock();
    owner = EstablishWireConnection(transaction, fd);
  }
  jobject root = NewRemoteBinder(env, owner, 1);
  if (root == nullptr || env->ExceptionCheck() ||
      !StartRemoteBinderDispatcherForOwner(env, owner)) {
    env->DeleteLocalRef(root);
    CloseRemoteBinderChannel(env, fd);
    close(fd);
    return nullptr;
  }
  // Process-lifetime manager capability. The dispatcher releases Binder refs
  // at EOF. Descriptor ownership is still process-lifetime; generation identity
  // independently prevents a stale Java endpoint from aliasing an FD successor.
  return root;
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

  JNINativeMethod service_manager_methods[] = {
      {const_cast<char*>("waitForServiceNative"),
       const_cast<char*>("(Ljava/lang/String;)Landroid/os/IBinder;"),
       reinterpret_cast<void*>(&ServiceManagerWaitForServiceNative)},
  };
  if (!Register(env, "android/os/ServiceManager", service_manager_methods,
                static_cast<jint>(std::size(service_manager_methods)))) {
    return false;
  }

  darwin_art::input::InputChannelParcelBridge parcel_bridge{
      .version = 1,
      .read = &ReadInputChannelParcel,
      .write = &WriteInputChannelParcel,
  };
  if (!darwin_art::input::RegisterInputNatives(env, parcel_bridge)) {
    return false;
  }
  if (!darwin_art::framework::wm::RegisterWindowInputPublisherNatives(env)) {
    return false;
  }

  return darwin_art::input::RegisterKeyCharacterMapNatives(env);

}
}  // namespace darwin_art
