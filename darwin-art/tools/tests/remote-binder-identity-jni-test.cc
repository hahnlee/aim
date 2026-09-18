#include "binder/remote_binder_identity_jni.h"

#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <initializer_list>
#include <limits>
#include <tuple>

namespace {

constexpr uintptr_t kBinder = 0x101;
constexpr uintptr_t kEndpointClass = 0x202;
constexpr uintptr_t kControlFd = 0x301;
constexpr uintptr_t kTargetId = 0x302;
constexpr uintptr_t kGeneration = 0x303;

template <typename T>
T Handle(uintptr_t value) {
  return reinterpret_cast<T>(value);
}

struct FakeJni {
  bool pending_exception = false;
  bool is_remote = true;
  bool throw_at_instance = false;
  const char* missing_field = nullptr;
  const char* throw_at_lookup = nullptr;
  const char* throw_at_read = nullptr;
  jint control_fd = 17;
  jint target_id = 23;
  jlong channel_generation = 0;
  int instance_calls = 0;
  int field_lookups = 0;
  int int_reads = 0;
  int long_reads = 0;
};

FakeJni* g_fake = nullptr;

jboolean IsInstanceOf(JNIEnv*, jobject, jclass) {
  ++g_fake->instance_calls;
  if (g_fake->throw_at_instance) {
    g_fake->pending_exception = true;
    return JNI_FALSE;
  }
  return g_fake->is_remote ? JNI_TRUE : JNI_FALSE;
}

jfieldID GetFieldID(JNIEnv*, jclass, const char* name, const char* signature) {
  ++g_fake->field_lookups;
  if (std::strcmp(name, "controlFd") == 0) assert(std::strcmp(signature, "I") == 0);
  if (std::strcmp(name, "targetId") == 0) assert(std::strcmp(signature, "I") == 0);
  if (std::strcmp(name, "channelGeneration") == 0)
    assert(std::strcmp(signature, "J") == 0);
  if ((g_fake->missing_field != nullptr &&
       std::strcmp(name, g_fake->missing_field) == 0) ||
      (g_fake->throw_at_lookup != nullptr &&
       std::strcmp(name, g_fake->throw_at_lookup) == 0)) {
    if (g_fake->throw_at_lookup != nullptr &&
        std::strcmp(name, g_fake->throw_at_lookup) == 0)
      g_fake->pending_exception = true;
    return nullptr;
  }
  if (std::strcmp(name, "controlFd") == 0) return Handle<jfieldID>(kControlFd);
  if (std::strcmp(name, "targetId") == 0) return Handle<jfieldID>(kTargetId);
  if (std::strcmp(name, "channelGeneration") == 0)
    return Handle<jfieldID>(kGeneration);
  assert(false && "unexpected field lookup");
  return nullptr;
}

jint GetIntField(JNIEnv*, jobject, jfieldID field) {
  ++g_fake->int_reads;
  if (g_fake->throw_at_read != nullptr &&
      ((field == Handle<jfieldID>(kControlFd) &&
        std::strcmp(g_fake->throw_at_read, "controlFd") == 0) ||
       (field == Handle<jfieldID>(kTargetId) &&
        std::strcmp(g_fake->throw_at_read, "targetId") == 0))) {
    g_fake->pending_exception = true;
  }
  return field == Handle<jfieldID>(kControlFd) ? g_fake->control_fd
                                               : g_fake->target_id;
}

jlong GetLongField(JNIEnv*, jobject, jfieldID field) {
  ++g_fake->long_reads;
  if (g_fake->throw_at_read != nullptr &&
      field == Handle<jfieldID>(kGeneration) &&
      std::strcmp(g_fake->throw_at_read, "channelGeneration") == 0)
    g_fake->pending_exception = true;
  return g_fake->channel_generation;
}

jboolean ExceptionCheck(JNIEnv*) {
  return g_fake->pending_exception ? JNI_TRUE : JNI_FALSE;
}

JNIEnv MakeEnv(FakeJni* fake, JNINativeInterface* functions) {
  g_fake = fake;
  *functions = JNINativeInterface{};
  functions->IsInstanceOf = &IsInstanceOf;
  functions->GetFieldID = &GetFieldID;
  functions->GetIntField = &GetIntField;
  functions->GetLongField = &GetLongField;
  functions->ExceptionCheck = &ExceptionCheck;
  return JNIEnv{functions};
}

void AssertInvalid(const darwin_art::binder::RemoteBinderIdentity& identity) {
  assert(identity.kind == darwin_art::binder::RemoteBinderIdentityKind::kInvalid);
  assert(identity.control_fd == -1);
  assert(identity.target_id == 0);
  assert(identity.channel_generation == 0);
}

}  // namespace

int main() {
  using darwin_art::binder::ReadRemoteBinderIdentity;
  using darwin_art::binder::RemoteBinderIdentityKind;

  // Null arguments and an already-pending exception are rejected without any
  // JNI access, and the exception remains pending for the caller.
  FakeJni fake;
  JNINativeInterface functions{};
  JNIEnv env = MakeEnv(&fake, &functions);
  AssertInvalid(ReadRemoteBinderIdentity(nullptr, Handle<jobject>(kBinder),
                                         Handle<jclass>(kEndpointClass)));
  AssertInvalid(ReadRemoteBinderIdentity(&env, nullptr,
                                         Handle<jclass>(kEndpointClass)));
  AssertInvalid(ReadRemoteBinderIdentity(&env, Handle<jobject>(kBinder), nullptr));
  assert(fake.instance_calls == 0 && fake.field_lookups == 0);
  fake.pending_exception = true;
  AssertInvalid(ReadRemoteBinderIdentity(&env, Handle<jobject>(kBinder),
                                         Handle<jclass>(kEndpointClass)));
  assert(fake.pending_exception && fake.instance_calls == 0);
  fake.pending_exception = false;

  // A valid object of another type is local, even if it has an impostor's
  // fields.  The helper must not inspect field shape in this case.
  fake.is_remote = false;
  const auto local = ReadRemoteBinderIdentity(
      &env, Handle<jobject>(kBinder), Handle<jclass>(kEndpointClass));
  assert(local.kind == RemoteBinderIdentityKind::kLocal);
  assert(fake.instance_calls == 1 && fake.field_lookups == 0 &&
         fake.int_reads == 0 && fake.long_reads == 0);
  fake.is_remote = true;

  // A pending exception from the type check is preserved and stops decoding.
  fake.throw_at_instance = true;
  AssertInvalid(ReadRemoteBinderIdentity(&env, Handle<jobject>(kBinder),
                                         Handle<jclass>(kEndpointClass)));
  assert(fake.pending_exception && fake.field_lookups == 0);
  fake.throw_at_instance = false;
  fake.pending_exception = false;

  // Missing or throwing field lookup is invalid and never reads fields.
  for (const char* field : {"controlFd", "targetId", "channelGeneration"}) {
    fake.missing_field = field;
    const int reads_before = fake.int_reads + fake.long_reads;
    AssertInvalid(ReadRemoteBinderIdentity(&env, Handle<jobject>(kBinder),
                                           Handle<jclass>(kEndpointClass)));
    assert(fake.int_reads + fake.long_reads == reads_before);
  }
  fake.missing_field = nullptr;
  fake.throw_at_lookup = "targetId";
  AssertInvalid(ReadRemoteBinderIdentity(&env, Handle<jobject>(kBinder),
                                         Handle<jclass>(kEndpointClass)));
  assert(fake.pending_exception && fake.int_reads == 0 && fake.long_reads == 0);
  fake.throw_at_lookup = nullptr;
  fake.pending_exception = false;

  // A negative Java long retains its high bit as an unsigned generation.
  fake.channel_generation = std::numeric_limits<jlong>::min();
  const auto remote = ReadRemoteBinderIdentity(
      &env, Handle<jobject>(kBinder), Handle<jclass>(kEndpointClass));
  assert(remote.kind == RemoteBinderIdentityKind::kRemote);
  assert(remote.control_fd == fake.control_fd &&
         remote.target_id == static_cast<uint32_t>(fake.target_id) &&
         remote.channel_generation == UINT64_C(0x8000000000000000));

  // Read exceptions are preserved and invalidate the complete result.
  for (const char* field : {"controlFd", "targetId", "channelGeneration"}) {
    fake.throw_at_read = field;
    fake.pending_exception = false;
    AssertInvalid(ReadRemoteBinderIdentity(&env, Handle<jobject>(kBinder),
                                           Handle<jclass>(kEndpointClass)));
    assert(fake.pending_exception);
  }
  fake.throw_at_read = nullptr;
  fake.pending_exception = false;

  // Metadata validation happens after all typed reads.
  for (const auto values : {std::tuple<jint, jint, jlong>{-1, 1, 1},
                            {1, 0, 1}, {1, 1, 0}}) {
    fake.control_fd = std::get<0>(values);
    fake.target_id = std::get<1>(values);
    fake.channel_generation = std::get<2>(values);
    AssertInvalid(ReadRemoteBinderIdentity(&env, Handle<jobject>(kBinder),
                                           Handle<jclass>(kEndpointClass)));
  }

  std::puts("remote-binder-identity-jni: PASS type-gated, exception-safe decode");
}
