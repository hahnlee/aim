#include "input_channel_jni.h"
#include "channel_identity_catalog.h"
#include "input_channel_jni_resources.h"

#include <algorithm>
#include <cstdlib>
#include <iostream>
#include <iterator>
#include <mutex>
#include <new>
#include <string>
#include <utility>
#include <vector>

namespace darwin_art::input {
namespace {
JavaVM* g_channel_jni_vm = nullptr;
bool RegisterMethods(JNIEnv* env, const char* name, JNINativeMethod* methods, jint count) {
  jclass klass = env->FindClass(name);
  if (klass == nullptr) return false;
  const bool registered = env->RegisterNatives(klass, methods, count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return registered;
}
struct DarwinInputChannelState;

struct DarwinInputChannelState {
  explicit DarwinInputChannelState(JNIEnv* env, std::string channel_name,
                                  std::shared_ptr<ChannelEndpoint> resource)
      : resources(std::make_shared<InputChannelResources>(
            std::move(channel_name), std::move(resource))) {
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
  }

  DarwinInputChannelState(JNIEnv* env, std::string channel_name, jobject token,
                          std::shared_ptr<ChannelEndpoint> resource)
      : resources(std::make_shared<InputChannelResources>(
            std::move(channel_name), std::move(resource))) {
    if (token != nullptr) connection_token = env->NewGlobalRef(token);
  }

  DarwinInputChannelState(JNIEnv* env, std::shared_ptr<InputChannelResources> core,
                          jobject token) : resources(std::move(core)) {
    if (token != nullptr) connection_token = env->NewGlobalRef(token);
  }

  ~DarwinInputChannelState() {
    if (connection_token != nullptr && g_channel_jni_vm != nullptr) {
      JNIEnv* env = nullptr;
      bool detach = false;
      jint status = g_channel_jni_vm->GetEnv(reinterpret_cast<void**>(&env),
                                           JNI_VERSION_1_6);
      if (status == JNI_EDETACHED &&
          g_channel_jni_vm->AttachCurrentThread(&env, nullptr) == JNI_OK) {
        detach = true;
        status = JNI_OK;
      }
      if (status == JNI_OK && env != nullptr) {
        env->DeleteGlobalRef(connection_token);
      }
      if (detach) g_channel_jni_vm->DetachCurrentThread();
    }
  }

  std::shared_ptr<InputChannelResources> resources;
  jobject connection_token = nullptr;
};

struct DarwinInputChannel {
  std::shared_ptr<DarwinInputChannelState> state;
  bool server = false;
  bool disposed = false;
};

std::mutex g_input_channel_registry_mutex;
std::vector<std::weak_ptr<DarwinInputChannelState>> g_input_channel_registry;

bool RegisterInputChannelState(JNIEnv* env,
    const std::shared_ptr<DarwinInputChannelState>& state) {
  const auto identity = darwin_art::input::GetChannelIdentityCatalog().Bind(
      env, state->connection_token, state->resources);
  if (identity.status != darwin_art::input::ChannelIdentityStatus::kResolved) {
    if (identity.status == darwin_art::input::ChannelIdentityStatus::kConflict &&
        !env->ExceptionCheck()) {
      jclass type = env->FindClass("java/lang/IllegalArgumentException");
      if (type != nullptr) {
        env->ThrowNew(type, "InputChannel token/name identity mismatch");
        env->DeleteLocalRef(type);
      }
    }
    if (identity.status == darwin_art::input::ChannelIdentityStatus::kClosed &&
        !env->ExceptionCheck()) {
      jclass type = env->FindClass("java/lang/IllegalStateException");
      if (type != nullptr) {
        env->ThrowNew(type, "InputChannel identity catalog is shutting down");
        env->DeleteLocalRef(type);
      }
    }
    return false;
  }
  state->resources = identity.core;
  darwin_art::input::RegisterChannelResources(state->resources);
  std::lock_guard<std::mutex> lock(g_input_channel_registry_mutex);
  std::erase_if(g_input_channel_registry,
                [](const auto& candidate) { return candidate.expired(); });
  g_input_channel_registry.emplace_back(state);
  return true;
}

struct InputChannelStateLookup {
  darwin_art::input::ChannelIdentityStatus status;
  std::shared_ptr<DarwinInputChannelState> state;
};
InputChannelStateLookup FindInputChannelState(
    JNIEnv* env, jobject token) {
  if (env == nullptr || token == nullptr)
    return {darwin_art::input::ChannelIdentityStatus::kJniFailure, {}};
  const auto catalog_identity =
      darwin_art::input::GetChannelIdentityCatalog().Find(env, token);
  if (catalog_identity.status == darwin_art::input::ChannelIdentityStatus::kClosed) {
    if (!env->ExceptionCheck()) {
      jclass type = env->FindClass("java/lang/IllegalStateException");
      if (type != nullptr) {
        env->ThrowNew(type, "InputChannel identity catalog is shutting down");
        env->DeleteLocalRef(type);
      }
    }
    return {catalog_identity.status, {}};
  }
  if (catalog_identity.status != darwin_art::input::ChannelIdentityStatus::kResolved)
    return {catalog_identity.status, {}};
  std::vector<std::shared_ptr<DarwinInputChannelState>> identities;
  {
    std::lock_guard<std::mutex> lock(g_input_channel_registry_mutex);
    identities.reserve(g_input_channel_registry.size());
    std::erase_if(g_input_channel_registry,
                  [](const auto& entry) { return entry.expired(); });
    for (const auto& entry : g_input_channel_registry)
      if (auto identity = entry.lock()) identities.push_back(std::move(identity));
  }
  // JNI comparisons and final global-reference cleanup never run under the
  // identity registry mutex. Snapshot pins preserve every borrowed token.
  for (const auto& identity : identities) {
    if (identity->connection_token != nullptr &&
        env->IsSameObject(identity->connection_token, token) == JNI_TRUE)
      return {darwin_art::input::ChannelIdentityStatus::kResolved, identity};
  }
  auto wrapper = std::make_shared<DarwinInputChannelState>(env, catalog_identity.core, token);
  if (wrapper->connection_token == nullptr || env->ExceptionCheck())
    return {darwin_art::input::ChannelIdentityStatus::kJniFailure, {}};
  if (!RegisterInputChannelState(env, wrapper))
    return {darwin_art::input::ChannelIdentityStatus::kJniFailure, {}};
  return {darwin_art::input::ChannelIdentityStatus::kResolved, wrapper};
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
             : env->NewStringUTF(channel->state->resources->Name().c_str());
}

jobject InputChannelGetToken(JNIEnv* env, jobject, jlong pointer) {
  const auto* channel = InputChannel(pointer);
  return channel == nullptr || channel->state == nullptr ||
                 channel->state->connection_token == nullptr
             ? nullptr
             : env->NewLocalRef(channel->state->connection_token);
}

jlongArray InputChannelOpenPair(JNIEnv* env, jclass, jstring name) {
  if (env == nullptr || env->ExceptionCheck()) return nullptr;
  try {
  darwin_art::input::ChannelEndpointCreationError error;
  auto endpoint = darwin_art::input::ChannelEndpoint::CreateLocal(&error);
  if (error == darwin_art::input::ChannelEndpointCreationError::kOutOfMemory)
    darwin_art::input::ThrowInputChannelOutOfMemory(env);
  if (endpoint == nullptr) return nullptr;
  darwin_art::input::InputChannelUtfChars utf_owner(env, name);
  const char* utf = utf_owner.Get();
  if (env->ExceptionCheck()) return nullptr;
  std::shared_ptr<DarwinInputChannelState> state;
  try {
    state = std::make_shared<DarwinInputChannelState>(
        env, utf == nullptr ? "darwin-art-input" : utf, std::move(endpoint));
  } catch (const std::bad_alloc&) {
    darwin_art::input::ThrowInputChannelOutOfMemory(env);
    return nullptr;
  }
  if (state->connection_token == nullptr || env->ExceptionCheck()) {
    return nullptr;
  }
  if (!RegisterInputChannelState(env, state)) return nullptr;
  std::unique_ptr<DarwinInputChannel> client(
      new (std::nothrow) DarwinInputChannel{state, false, false});
  std::unique_ptr<DarwinInputChannel> server(
      new (std::nothrow) DarwinInputChannel{state, true, false});
  if (client == nullptr || server == nullptr) {
    darwin_art::input::ThrowInputChannelOutOfMemory(env);
    return nullptr;
  }
  jlong values[] = {
      static_cast<jlong>(reinterpret_cast<std::uintptr_t>(client.get())),
      static_cast<jlong>(reinterpret_cast<std::uintptr_t>(server.get())),
  };
  jlongArray result = env->NewLongArray(2);
  if (result != nullptr) env->SetLongArrayRegion(result, 0, 2, values);
  if (result == nullptr || env->ExceptionCheck()) return nullptr;
  (void)client.release();
  (void)server.release();
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android InputChannel open-pair name=" << state->resources->Name()
              << " read_fd=" << state->resources->Endpoint()->Transport()->ReadFd()
              << " write_fd=" << state->resources->Endpoint()->Transport()->WriteFd() << "\n";
  }
  return result;
  } catch (const std::bad_alloc&) {
    darwin_art::input::ThrowInputChannelOutOfMemory(env);
    return nullptr;
  }
}


darwin_art::input::InputChannelParcelBridge g_parcel_bridge;

void ThrowParcelBridgeFailure(JNIEnv* env) {
  if (env->ExceptionCheck()) return;
  jclass type = env->FindClass("java/lang/IllegalStateException");
  if (type != nullptr) {
    env->ThrowNew(type, "InputChannel Parcel transport failed");
    env->DeleteLocalRef(type);
  }
}

jlong InputChannelReadParcel(JNIEnv* env, jobject, jobject parcel_object) {
  if (env == nullptr || parcel_object == nullptr || g_parcel_bridge.read == nullptr ||
      env->ExceptionCheck())
    return 0;
  darwin_art::input::InputChannelParcelData data{};
  darwin_art::input::InputChannelParcelResources parcel_resources(env, data);
  try {
  if (!g_parcel_bridge.read(env, parcel_object, &data)) {
    ThrowParcelBridgeFailure(env);
    return 0;
  }
  if (env->ExceptionCheck()) return 0;
  if (!data.initialized ||
      data.name == nullptr || data.endpoint_fd < 0) {
    return 0;
  }
  darwin_art::input::InputChannelUtfChars utf_owner(env, data.name);
  const char* utf = utf_owner.Get();
  if (utf == nullptr || env->ExceptionCheck()) return 0;
  const std::string channel_name = utf == nullptr ? "" : utf;
  const auto lookup = FindInputChannelState(env, data.token);
  auto state = lookup.state;
  if (state == nullptr &&
      lookup.status != darwin_art::input::ChannelIdentityStatus::kNotFound) return 0;
  if (state != nullptr && state->resources->Name() != channel_name) {
    jclass type = env->FindClass("java/lang/IllegalArgumentException");
    if (type != nullptr) {
      env->ThrowNew(type, "InputChannel token/name identity mismatch");
      env->DeleteLocalRef(type);
    }
    return 0;  // Parcel scope consumes the owned duplicate, never adopts it.
  }
  bool adopted_endpoint = false;
  if (state == nullptr && data.token != nullptr && !channel_name.empty() &&
      !env->ExceptionCheck()) {
    darwin_art::input::ChannelEndpointCreationError error;
    auto endpoint = darwin_art::input::ChannelEndpoint::AdoptRemote(
        std::exchange(data.endpoint_fd, -1), &error);
    if (error == darwin_art::input::ChannelEndpointCreationError::kOutOfMemory)
      darwin_art::input::ThrowInputChannelOutOfMemory(env);
    adopted_endpoint = true;
    if (endpoint != nullptr) {
      state = std::make_shared<DarwinInputChannelState>(
          env, channel_name, data.token, std::move(endpoint));
    }
    if (state == nullptr || state->connection_token == nullptr || state->resources->Endpoint()->Transport()->ReadFd() < 0 ||
        state->resources->Endpoint()->Transport()->WriteFd() < 0 || env->ExceptionCheck()) {
      state.reset();
    } else {
      if (!RegisterInputChannelState(env, state)) state.reset();
    }
  } else if (state != nullptr) {
    // Same-process state already owns its endpoint pair. Consume the bridge's
    // owned duplicate exactly once; do not replace the live transport.
    darwin_art_bionic_socket_broker_close(data.endpoint_fd);
    data.endpoint_fd = -1;
  }
  if (state == nullptr || env->ExceptionCheck()) {
    if (!adopted_endpoint && data.endpoint_fd >= 0) {
      darwin_art_bionic_socket_broker_close(data.endpoint_fd);
      data.endpoint_fd = -1;
    }
    return 0;
  }
  auto* channel = new (std::nothrow) DarwinInputChannel{state, false, false};
  if (channel == nullptr) darwin_art::input::ThrowInputChannelOutOfMemory(env);
  if (channel == nullptr && adopted_endpoint) {
    // The state destructor closes the adopted endpoint on allocation failure.
    state.reset();
  }
  return static_cast<jlong>(reinterpret_cast<std::uintptr_t>(channel));
  } catch (const std::bad_alloc&) {
    darwin_art::input::ThrowInputChannelOutOfMemory(env);
    return 0;
  }
}

void InputChannelWriteParcel(JNIEnv* env, jobject, jobject parcel_object,
                             jlong pointer) {
  if (env == nullptr || parcel_object == nullptr || g_parcel_bridge.write == nullptr ||
      env->ExceptionCheck())
    return;
  const auto* channel = InputChannel(pointer);
  const bool valid = channel != nullptr && !channel->disposed &&
                     channel->state != nullptr &&
                     channel->state->connection_token != nullptr;
  const int endpoint_fd = valid
      ? channel->state->resources->Endpoint()->BorrowParcelFd(channel->server) : -1;
  if (!g_parcel_bridge.write(
      env, parcel_object, valid, valid ? channel->state->connection_token : nullptr,
      valid ? channel->state->resources->Name().c_str() : nullptr, endpoint_fd))
    ThrowParcelBridgeFailure(env);
}

}  // namespace

std::shared_ptr<InputChannelResources> AcquireInputChannelResources(jlong pointer) {
  const auto* channel = InputChannel(pointer);
  if (channel == nullptr || channel->disposed || channel->state == nullptr) return {};
  return channel->state->resources;
}
std::shared_ptr<InputChannelResources> AcquireInputChannelResources(JNIEnv* env, jobject object) {
  if (env == nullptr || object == nullptr || env->ExceptionCheck()) return {};
  jclass klass = env->GetObjectClass(object);
  jfieldID field = klass == nullptr ? nullptr : env->GetFieldID(klass, "mPtr", "J");
  const auto pointer = field == nullptr ? 0 : env->GetLongField(object, field);
  if (klass != nullptr) env->DeleteLocalRef(klass);
  if (env->ExceptionCheck()) return {};
  return AcquireInputChannelResources(pointer);
}

jobject PinInputChannelToken(
    JNIEnv* env, jobject object,
    const std::shared_ptr<InputChannelResources>& expected) {
  if (env == nullptr || object == nullptr || env->ExceptionCheck()) return nullptr;
  jclass klass = env->GetObjectClass(object);
  jfieldID field = klass == nullptr ? nullptr : env->GetFieldID(klass, "mPtr", "J");
  const jlong pointer = field == nullptr ? 0 : env->GetLongField(object, field);
  if (klass != nullptr) env->DeleteLocalRef(klass);
  // Validate the JNI lookup before touching the opaque native handle. This
  // preserves the framework's pending-exception contract on lookup failure.
  if (env->ExceptionCheck()) return nullptr;
  const auto* channel = InputChannel(pointer);
  if (channel == nullptr || channel->disposed || channel->state == nullptr ||
      channel->state->resources != expected ||
      channel->state->connection_token == nullptr)
    return nullptr;
  // This is deliberately the channel state's original token, not a newly
  // constructed Binder or a token recovered from another channel identity.
  return env->NewGlobalRef(channel->state->connection_token);
}

std::shared_ptr<InputChannelResources> AcquireServerInputChannelResources(
    JNIEnv* env, jobject object) {
  if (env == nullptr || object == nullptr || env->ExceptionCheck()) return {};
  jclass klass = env->GetObjectClass(object);
  jfieldID field = klass == nullptr ? nullptr : env->GetFieldID(klass, "mPtr", "J");
  const jlong pointer = field == nullptr ? 0 : env->GetLongField(object, field);
  if (klass != nullptr) env->DeleteLocalRef(klass);
  // A field value is unspecified on exception: validate before dereferencing.
  if (env->ExceptionCheck()) return {};
  const auto* channel = InputChannel(pointer);
  if (channel == nullptr || channel->disposed || !channel->server ||
      channel->state == nullptr) return {};
  return channel->state->resources;
}
bool RegisterInputChannelJni(JNIEnv* env, InputChannelParcelBridge bridge) {
  if (env == nullptr || env->GetJavaVM(&g_channel_jni_vm) != JNI_OK ||
      g_channel_jni_vm == nullptr || bridge.version != 1 ||
      bridge.read == nullptr || bridge.write == nullptr) return false;
  g_parcel_bridge = bridge;
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
  if (!RegisterMethods(env, "android/view/InputChannel", input_channel_methods,
                static_cast<jint>(std::size(input_channel_methods)))) {
    return false;
  }

  return true;
}
}  // namespace darwin_art::input
