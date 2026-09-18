#include "root_key_decision_jni.h"
#include "root_key_server_jni.h"
#include "desktop_root_client_jni.h"
#include "../input/root_key_authority.h"
#include "../input/channel_identity_catalog.h"
#include "../input/channel_resources.h"
#include <bit>
#include <cstdlib>
#include <cstdio>
#include <limits>
#include <map>
#include <mutex>
#include <new>
#include <utility>

namespace darwin_art::framework::wm {
namespace {
constexpr jint kRetained = 0, kRetry = 1, kFailed = 2;
using input::DecisionRecordHandle;

// Only this registry, admitted JNI operations and the shutdown tail own these
// refs. Background notifications and input tickets retain JNI-free owners.
// Admission precedes every local reference pin; refs retire before its count.
struct GlobalReference final {
  explicit GlobalReference(JavaVM* owner) : vm(owner) {}
  ~GlobalReference() {
    if (!object) return;
    JNIEnv* env = nullptr;
    if (!vm || vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) != JNI_OK)
      std::abort();  // violated ownership: never attach a thread or leak a ref
    env->DeleteGlobalRef(object);
  }
  JavaVM* const vm;
  jobject object = nullptr;
};
using Reference = std::shared_ptr<GlobalReference>;
struct Envelope final { DecisionRecordHandle record; Reference token; };
struct Entry final {
  std::mutex mutex;
  std::shared_ptr<window::DesktopRootEvents> root;
  input::RootKeyAuthorityHandle authority;
  std::shared_ptr<binder::EndpointLifetime> wire;
  uint64_t generation = 0;
  Reference capability;
  DecisionRecordHandle record;
  std::shared_ptr<Envelope> envelope;
  bool token_pending = false, closed = false;
};
struct Registry final {
  std::mutex mutex;
  bool closed = false;
  uint64_t next = 1;
  size_t in_flight = 0;
  JavaVM* vm = nullptr;
  Reference type, remote_type;
  std::map<uint64_t, std::shared_ptr<Entry>> entries;
};
Registry& State() { static Registry value; return value; }
class Admission final {
 public:
  explicit Admission(bool cleanup = false) {
    auto& state = State(); std::lock_guard lock(state.mutex);
    if (cleanup || !state.closed) { ++state.in_flight; active_ = true; }
  }
  ~Admission() {
    if (!active_) return;
    auto& state = State(); std::lock_guard lock(state.mutex); --state.in_flight;
  }
  explicit operator bool() const { return active_; }
 private:
  bool active_ = false;
};
uint64_t Bits(jlong value) { return std::bit_cast<uint64_t>(value); }
std::shared_ptr<Entry> Find(jlong binding) {
  auto& state = State(); std::lock_guard lock(state.mutex);
  auto found = state.entries.find(Bits(binding));
  return found == state.entries.end() ? nullptr : found->second;
}
bool Live(const Entry& entry) {
  return !entry.closed && !entry.authority->Closed() && entry.wire->Matches(entry.generation);
}
void SealLocked(Entry& entry) {
  entry.closed = true;
  entry.authority->Close();  // NOT the shared Binder server's transport lifetime
}
jint FailCurrent(const std::shared_ptr<Entry>& entry, const DecisionRecordHandle& record) {
  std::lock_guard lock(entry->mutex);
  if (entry->record != record) return kRetained;
  SealLocked(*entry); return kFailed;
}
bool Same(const input::DecisionRecord& a, const input::DecisionRecord& b) {
  return a.incarnation == b.incarnation && a.fact_serial == b.fact_serial &&
      a.sequence == b.sequence && a.epoch == b.epoch && a.selection_present == b.selection_present;
}
Reference Pin(JNIEnv* env, JavaVM* vm, jobject object) {
  auto result = std::make_shared<GlobalReference>(vm);
  result->object = env->NewGlobalRef(object);
  if (!result->object || env->ExceptionCheck()) return nullptr;
  return result;
}
struct LocalReference final {
  JNIEnv* const env;
  jobject object = nullptr;
  ~LocalReference() { if (object) env->DeleteLocalRef(object); }
};
jclass LoadSupportClass(JNIEnv* env, const char* name) {
  jclass type = env->FindClass("java/lang/ClassLoader");
  if (!type || env->ExceptionCheck()) { if (type) env->DeleteLocalRef(type); return nullptr; }
  jmethodID get = env->GetStaticMethodID(type, "getSystemClassLoader", "()Ljava/lang/ClassLoader;");
  jmethodID load = !get || env->ExceptionCheck() ? nullptr :
      env->GetMethodID(type, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;");
  jobject loader = !load || env->ExceptionCheck() ? nullptr : env->CallStaticObjectMethod(type, get);
  jstring text = !loader || env->ExceptionCheck() ? nullptr : env->NewStringUTF(name);
  jclass result = !text || env->ExceptionCheck() ? nullptr :
      static_cast<jclass>(env->CallObjectMethod(loader, load, text));
  if (text) env->DeleteLocalRef(text);
  if (loader) env->DeleteLocalRef(loader);
  env->DeleteLocalRef(type); return result;
}
jlong Attach(JNIEnv* env, jclass, jlong prior, jlong target, jlong incarnation, jobject capability) {
  const auto reject = [](const char* reason) -> jlong {
    if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr)
      std::fprintf(stderr, "ART RootKey decision attach rejected reason=%s\n", reason);
    return 0;
  };
  Admission admission;
  Reference remote, capability_pin;
  std::shared_ptr<Entry> entry;
  if (!admission || !env || env->ExceptionCheck() || !capability) return reject("admission");
  JavaVM* vm;
  { auto& state = State(); std::lock_guard lock(state.mutex);
    vm = state.vm; remote = state.remote_type; }
  if (!vm || !remote) return reject("registration");
  auto root = RetainDesktopRootClientTarget(target);
  if (!root || root->incarnation() != Bits(incarnation)) return reject("root-identity");
  auto authority = input::AcquireRootKeyAuthority(root);
  if (!authority) return reject("authority");
  auto server = CaptureRootKeyServer(env, capability, static_cast<jclass>(remote->object));
  if (env->ExceptionCheck() || !server.lifetime) return reject("server-capture");
  const auto generation = server.lifetime->Generation();
  if (!server.lifetime->Matches(generation)) return reject("server-generation");
  if (prior) {
    entry = Find(prior);
    if (!entry || entry->root != root || entry->authority != authority ||
        entry->wire != server.lifetime || entry->generation != generation) return 0;
    const bool same = env->IsSameObject(capability, entry->capability->object) == JNI_TRUE;
    const bool pending = env->ExceptionCheck();
    std::lock_guard lock(entry->mutex);
    return same && !pending && Live(*entry) ? prior : 0;
  }
  try {
    capability_pin = Pin(env, vm, capability);
    if (!capability_pin || !authority->AttachServer(server.lifetime, generation)) return reject("server-admission");
    entry = std::make_shared<Entry>();
    entry->root = root; entry->authority = authority; entry->wire = server.lifetime;
    entry->generation = generation; entry->capability = capability_pin;
    auto& state = State(); std::lock_guard lock(state.mutex);
    if (state.closed || authority->Closed() || !server.lifetime->Matches(generation) ||
        state.next > static_cast<uint64_t>(std::numeric_limits<int64_t>::max())) return 0;
    for (const auto& item : state.entries) if (item.second->root == root) return 0;
    const uint64_t id = state.next++;
    state.entries.emplace(id, entry); return static_cast<jlong>(id);
  } catch (const std::bad_alloc&) { return 0; }
}
jint Publish(JNIEnv* env, jclass, jlong binding, jlong incarnation, jlong fact,
             jlong sequence, jlong epoch, jobject token) {
  Admission admission;
  std::shared_ptr<Entry> entry;
  DecisionRecordHandle record;
  std::shared_ptr<Envelope> envelope, retired;
  if (!admission || !env) return kFailed;
  entry = Find(binding);
  if (!entry || entry->root->incarnation() != Bits(incarnation)) return kFailed;
  auto candidate = input::CreateRootKeyDecisionRecord(
      Bits(incarnation), Bits(fact), Bits(sequence), Bits(epoch), token != nullptr);
  bool fresh = false;
  {
    std::lock_guard lock(entry->mutex);
    if (!Live(*entry)) return kFailed;
    if (entry->record && Bits(sequence) < entry->record->sequence) return kRetained;
    if (!candidate) { SealLocked(*entry); return kFailed; }
    if (entry->record && Bits(sequence) == entry->record->sequence) {
      record = entry->record;
      if (!Same(*record, *candidate)) { SealLocked(*entry); return kFailed; }
      if (entry->token_pending) return kRetry;
      envelope = entry->envelope;
    } else {
      record = candidate; retired = std::move(entry->envelope);
      entry->record = record; entry->token_pending = true;
      // Clear old authority before ANY callback-capable JNI or identity lookup.
      if (!entry->authority->PublishDecision(record)) { SealLocked(*entry); return kFailed; }
      fresh = true;
    }
  }
  retired.reset();  // ref retirement may reenter; no owner/input locks held
  if (env->ExceptionCheck()) return FailCurrent(entry, record);
  try {
    if (fresh) {
      envelope = std::make_shared<Envelope>(); envelope->record = record;
      if (token) envelope->token = Pin(env, entry->capability->vm, token);
      if (token && !envelope->token) return FailCurrent(entry, record);
      std::lock_guard lock(entry->mutex);
      if (entry->record != record) return kRetained;
      if (!Live(*entry)) return kFailed;
      entry->envelope = envelope; entry->token_pending = false;
    } else {
      if (!envelope) return FailCurrent(entry, record);
      const bool same = env->IsSameObject(token,
          envelope->token ? envelope->token->object : nullptr) == JNI_TRUE;
      const bool pending = env->ExceptionCheck();
      { std::lock_guard lock(entry->mutex);
        if (entry->record != record) return kRetained;
        if (!Live(*entry)) return kFailed; }
      if (!same || pending) return FailCurrent(entry, record);
    }
    if (!record->selection_present) return kRetained;
    // Equal retries retry unresolved identity. ACK is not receiver readiness.
    auto identity = input::GetChannelIdentityCatalog().Find(env, envelope->token->object);
    auto routing = identity.core ? identity.core->Routing() : input::InputRoutingHandle{};
    const bool pending = env->ExceptionCheck();
    std::lock_guard lock(entry->mutex);
    if (entry->record != record) return kRetained;
    if (!Live(*entry)) return kFailed;
    if (pending || (identity.status != input::ChannelIdentityStatus::kResolved &&
                    identity.status != input::ChannelIdentityStatus::kNotFound)) {
      SealLocked(*entry); return kFailed;
    }
    // The real WMS scheduler retains and backs off on RETRY. Without a native
    // identity-progress driver yet, ACK would abandon an unresolved token.
    if (identity.status == input::ChannelIdentityStatus::kNotFound) return kRetry;
    if (!routing || !entry->authority->ResolveDecision(record, routing)) {
      SealLocked(*entry); return kFailed;
    }
    return kRetained;
  } catch (const std::bad_alloc&) { return FailCurrent(entry, record); }
}
void Close(JNIEnv*, jclass, jlong binding) {
  Admission admission;
  std::shared_ptr<Entry> entry;
  if (!admission) return;
  {
    auto& state = State(); std::lock_guard lock(state.mutex);
    auto found = state.entries.find(Bits(binding));
    if (found != state.entries.end()) {
      entry = found->second;
      state.entries.erase(found);
    }
  }
  if (!entry) return;
  std::lock_guard lock(entry->mutex);
  SealLocked(*entry);  // no JNI; preserve pending Java exception
  // Entry/global refs retire after the entry mutex but before Admission. An
  // admitted publish may still pin it; global close waits for that admission.
}
}  // namespace

bool RegisterRootKeyDecisionClient(JNIEnv* env) {
  Admission admission;
  Reference type_pin, remote_pin;
  if (!admission || !env || env->ExceptionCheck()) return false;
  JavaVM* vm = nullptr;
  if (env->GetJavaVM(&vm) != JNI_OK || !vm) return false;
  { auto& state = State(); std::lock_guard lock(state.mutex);
    if (state.type) return !state.closed && state.vm == vm; }
  try {
    LocalReference type_local{env, LoadSupportClass(env,
        "dev.darwinart.runtime.wm.DesktopRootKeyDecisionNative")};
    jclass type = static_cast<jclass>(type_local.object);
    if (!type || env->ExceptionCheck()) return false;
    const JNINativeMethod methods[] = {
      {const_cast<char*>("nativeAttach"), const_cast<char*>("(JJJLandroid/os/IBinder;)J"), reinterpret_cast<void*>(Attach)},
      {const_cast<char*>("nativePublish"), const_cast<char*>("(JJJJJLandroid/os/IBinder;)I"), reinterpret_cast<void*>(Publish)},
      {const_cast<char*>("nativeClose"), const_cast<char*>("(J)V"), reinterpret_cast<void*>(Close)},
    };
    const bool registered = env->RegisterNatives(type, methods, 3) == JNI_OK;
    if (registered && !env->ExceptionCheck()) type_pin = Pin(env, vm, type);
    if (!type_pin) return false;
    LocalReference remote_local{env, LoadSupportClass(env,
        "dev.darwinart.runtime.os.RemoteBinder")};
    jclass remote = static_cast<jclass>(remote_local.object);
    if (remote && !env->ExceptionCheck()) remote_pin = Pin(env, vm, remote);
    if (!remote_pin) return false;
    auto& state = State(); std::lock_guard lock(state.mutex);
    if (state.closed) return false;
    if (state.type) return state.vm == vm;
    state.vm = vm; state.type = type_pin; state.remote_type = remote_pin; return true;
  } catch (const std::bad_alloc&) { return false; }
}
bool CloseRootKeyDecisionAdmission(JNIEnv*) {
  Admission admission(true);
  std::map<uint64_t, std::shared_ptr<Entry>> retired;
  { auto& state = State(); std::lock_guard lock(state.mutex);
    state.closed = true; retired.swap(state.entries); }
  for (const auto& item : retired) {
    std::lock_guard lock(item.second->mutex); SealLocked(*item.second);
  }
  auto& state = State(); std::lock_guard lock(state.mutex); return state.in_flight == 1;
}
bool PollRootKeyDecisionQuiesced(JNIEnv*) {
  auto& state = State(); std::lock_guard lock(state.mutex); return state.closed && state.in_flight == 0;
}
bool ClearRootKeyDecisionReferences(JNIEnv* env) {
  Admission admission(true);
  Reference retired_type, retired_remote;
  if (!env) return false;
  { auto& state = State(); std::lock_guard lock(state.mutex);
    if (!state.closed || state.in_flight != 1 || !state.entries.empty()) return false;
    retired_type.swap(state.type); retired_remote.swap(state.remote_type); state.vm = nullptr; }
  return true;  // explicit VM-thread tail; refs retire outside registry mutex
}
}  // namespace darwin_art::framework::wm
