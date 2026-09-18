#include "../../compat/darwin_angle_egl.h"
#include "../../compat/window/blast_buffer_queue_jni.h"

#include <android/surface_control.h>

#include <cassert>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <map>
#include <string>
#include <vector>

struct ASurfaceTransaction {
  uint64_t id = 0;
};

namespace {

struct FakeWindow {
  int references = 1;
  DarwinArtAndroidNativeWindowTransactionCallback callback = nullptr;
  void* callback_context = nullptr;
  void (*release_context)(void*) = nullptr;
};

struct RegisteredMethod {
  void* function = nullptr;
  std::string signature;
};

JNIInvokeInterface g_vm_functions{};
JavaVM g_vm{&g_vm_functions};
JNIEnv* g_env = nullptr;
JNIEnv* g_quiesce_env = nullptr;

std::string g_current_class;
std::map<std::string, RegisteredMethod> g_registered;
FakeWindow* g_last_window = nullptr;
DarwinArtAndroidNativeWindowTransactionCallback g_stale_callback = nullptr;
void* g_stale_context = nullptr;
void (*g_stale_release)(void*) = nullptr;

uint64_t g_next_transaction_id = 1;
std::vector<ASurfaceTransaction*> g_created_transactions;
std::map<ASurfaceTransaction*, int> g_delete_counts;
int g_apply_count = 0;
int g_window_release_count = 0;

int g_new_global_ref_count = 0;
int g_delete_global_ref_count = 0;
int g_detach_count = 0;
std::vector<bool> g_delete_global_quiesce;
std::vector<bool> g_detach_quiesce;

using DestroyFunction = void (*)(JNIEnv*, jclass, jlong);
DestroyFunction g_destroy = nullptr;
jlong g_destroy_handle = 0;
bool g_consumer_called = false;

bool g_attach_fails = false;
bool g_reentrant_transaction_delete = false;
bool g_reentrant_transaction_destroy = false;
bool g_reentrant_sweep_called = false;
bool g_reentrant_sweep_result = true;

jobject TestConsumer() {
  return reinterpret_cast<jobject>(static_cast<uintptr_t>(0x66));
}

std::string MethodKey(const char* name) {
  return g_current_class + "::" + name;
}

void ResetObservations() {
  g_current_class.clear();
  g_registered.clear();
  g_last_window = nullptr;
  g_stale_callback = nullptr;
  g_stale_context = nullptr;
  g_stale_release = nullptr;
  g_created_transactions.clear();
  g_delete_counts.clear();
  g_apply_count = 0;
  g_window_release_count = 0;
  g_new_global_ref_count = 0;
  g_delete_global_ref_count = 0;
  g_detach_count = 0;
  g_delete_global_quiesce.clear();
  g_detach_quiesce.clear();
  g_destroy = nullptr;
  g_destroy_handle = 0;
  g_consumer_called = false;
  g_attach_fails = false;
  g_reentrant_transaction_delete = false;
  g_reentrant_transaction_destroy = false;
  g_reentrant_sweep_called = false;
  g_reentrant_sweep_result = true;
}

jclass FakeFindClass(JNIEnv*, const char* name) {
  g_current_class = name == nullptr ? "" : name;
  return reinterpret_cast<jclass>(static_cast<uintptr_t>(0x11));
}

jint FakeRegisterNatives(JNIEnv*, jclass, const JNINativeMethod* methods,
                         jint count) {
  assert(methods != nullptr);
  for (jint i = 0; i < count; ++i) {
    g_registered[MethodKey(methods[i].name)] =
        {methods[i].fnPtr, methods[i].signature};
  }
  return JNI_OK;
}

void FakeDeleteLocalRef(JNIEnv*, jobject) {}

jint FakeGetJavaVM(JNIEnv*, JavaVM** vm) {
  assert(vm != nullptr);
  *vm = &g_vm;
  return JNI_OK;
}

jclass FakeGetObjectClass(JNIEnv*, jobject object) {
  assert(object == TestConsumer());
  return reinterpret_cast<jclass>(static_cast<uintptr_t>(0x22));
}

jmethodID FakeGetMethodID(JNIEnv*, jclass, const char* name, const char*) {
  assert(name != nullptr);
  if (std::string(name) == "accept") {
    return reinterpret_cast<jmethodID>(static_cast<uintptr_t>(0x33));
  }
  return reinterpret_cast<jmethodID>(static_cast<uintptr_t>(0x34));
}

jobject FakeNewObjectV(JNIEnv*, jclass, jmethodID method, va_list) {
  assert(method != nullptr);
  return reinterpret_cast<jobject>(static_cast<uintptr_t>(0x44));
}

jboolean FakeExceptionCheck(JNIEnv*) { return JNI_FALSE; }

void FakeCallVoidMethodV(JNIEnv* env, jobject object, jmethodID method,
                         va_list) {
  assert(object == TestConsumer());
  assert(method != nullptr);
  g_consumer_called = true;
  // Consumer.accept synchronously destroys its queue. The callback's shared
  // State and admission lease must keep the owner usable until return.
  assert(g_destroy != nullptr);
  g_destroy(env, nullptr, g_destroy_handle);
}

jobject FakeNewGlobalRef(JNIEnv*, jobject object) {
  assert(object == TestConsumer());
  ++g_new_global_ref_count;
  return object;
}

void FakeDeleteGlobalRef(JNIEnv* env, jobject object) {
  assert(object == TestConsumer());
  ++g_delete_global_ref_count;
  // This is deliberately reentrant. During an admitted callback, and during
  // owner cleanup, readiness must remain false until the final lease retires.
  g_delete_global_quiesce.push_back(
      darwin_art::window::QuiesceBlastBufferQueues(env));
}

jint FakeGetEnv(JavaVM*, void** result, jint) {
  assert(result != nullptr);
  *result = nullptr;
  return JNI_EDETACHED;
}

jint FakeAttachCurrentThread(JavaVM*, JNIEnv** result, void*) {
  assert(result != nullptr);
  if (g_attach_fails) return JNI_ERR;
  *result = g_env;
  return JNI_OK;
}

jint FakeDetachCurrentThread(JavaVM*) {
  ++g_detach_count;
  g_detach_quiesce.push_back(
      darwin_art::window::QuiesceBlastBufferQueues(g_quiesce_env));
  return JNI_OK;
}

extern "C" ASurfaceTransaction* ASurfaceTransaction_create() {
  auto* transaction = new ASurfaceTransaction{g_next_transaction_id++};
  g_created_transactions.push_back(transaction);
  return transaction;
}

extern "C" void ASurfaceTransaction_delete(ASurfaceTransaction* transaction) {
  if (transaction == nullptr) return;
  ++g_delete_counts[transaction];
  if (g_reentrant_transaction_delete && !g_reentrant_sweep_called) {
    g_reentrant_sweep_called = true;
    if (g_reentrant_transaction_destroy) {
      assert(g_destroy != nullptr);
      g_destroy(g_env, nullptr, g_destroy_handle);
    }
    g_reentrant_sweep_result =
        darwin_art::window::QuiesceBlastBufferQueues(g_quiesce_env);
  }
  delete transaction;
}

extern "C" void ASurfaceTransaction_apply(ASurfaceTransaction* transaction) {
  assert(transaction != nullptr);
  ++g_apply_count;
}

extern "C" void ASurfaceTransaction_setOnCommit(
    ASurfaceTransaction*, void*, void (*)(void*, ASurfaceTransactionStats*)) {}

extern "C" void darwin_art_android_surface_transaction_merge(
    ASurfaceTransaction* destination, ASurfaceTransaction* source) {
  assert(destination != nullptr);
  assert(source != nullptr);
}

extern "C" bool darwin_art_android_surface_transaction_merge_deferred(
    ASurfaceTransaction* destination, ASurfaceTransaction* source,
    ASurfaceTransaction* disposal) {
  assert(destination != nullptr);
  assert(source != nullptr);
  assert(disposal != nullptr);
  return true;
}

extern "C" void darwin_art_android_surface_transaction_set_on_discard(
    ASurfaceTransaction*, void*, void (*)(void*)) {}

extern "C" void* darwin_art_android_ANativeWindow_create(int32_t width,
                                                           int32_t height,
                                                           int32_t format) {
  assert(width > 0);
  assert(height > 0);
  assert(format > 0);
  g_last_window = new FakeWindow();
  return g_last_window;
}

extern "C" void darwin_art_android_ANativeWindow_acquire(void* window) {
  assert(window != nullptr);
  ++static_cast<FakeWindow*>(window)->references;
}

extern "C" void darwin_art_android_ANativeWindow_release(void* window) {
  if (window == nullptr) return;
  auto* fake = static_cast<FakeWindow*>(window);
  ++g_window_release_count;
  assert(fake->references > 0);
  if (--fake->references == 0) {
    if (g_last_window == fake) g_last_window = nullptr;
    delete fake;
  }
}

extern "C" int32_t darwin_art_android_ANativeWindow_setBuffersGeometry(
    void*, int32_t width, int32_t height, int32_t format) {
  assert(width > 0);
  assert(height > 0);
  assert(format > 0);
  return 0;
}

extern "C" void darwin_art_android_ANativeWindow_set_surface_control(
    void*, void*) {}

extern "C" bool darwin_art_android_ANativeWindow_set_transaction_callback(
    void* window, DarwinArtAndroidNativeWindowTransactionCallback callback,
    void* context, void (*release_context)(void*)) {
  assert(window != nullptr);
  auto* fake = static_cast<FakeWindow*>(window);
  if (callback != nullptr) {
    fake->callback = callback;
    fake->callback_context = context;
    fake->release_context = release_context;
    g_stale_callback = callback;
    g_stale_context = context;
    g_stale_release = release_context;
  } else {
    // Model a producer snapshot that has already retained the observer. The
    // test releases this snapshot only after the owner sweep is complete.
    fake->callback = nullptr;
    fake->callback_context = nullptr;
    fake->release_context = nullptr;
  }
  return true;
}

template <typename Function>
Function Native(const char* name) {
  const auto it = g_registered.find(
      std::string("android/graphics/BLASTBufferQueue::") + name);
  assert(it != g_registered.end());
  return reinterpret_cast<Function>(it->second.function);
}

void ReleaseStaleObserver() {
  assert(g_stale_release != nullptr);
  g_stale_release(g_stale_context);
  g_stale_callback = nullptr;
  g_stale_context = nullptr;
  g_stale_release = nullptr;
}

void CreateQueue(JNIEnv* env, jlong* handle) {
  using CreateFunction = jlong (*)(JNIEnv*, jclass, jstring, jboolean);
  using UpdateFunction = void (*)(JNIEnv*, jclass, jlong, jlong, jlong, jlong,
                                  jint);
  const auto create = Native<CreateFunction>("nativeCreate");
  const auto update = Native<UpdateFunction>("nativeUpdate");
  *handle = create(env, nullptr, nullptr, JNI_FALSE);
  assert(*handle != 0);
  update(env, nullptr, *handle, 0, 640, 480, 1);
  assert(g_last_window != nullptr);
  assert(g_stale_callback != nullptr);
  assert(g_stale_context != nullptr);
}

void RunConsumerDestroyScenario(JNIEnv* env) {
  ResetObservations();
  assert(darwin_art::window::RegisterBlastBufferQueueNatives(env));
  using DestroyFunctionType = void (*)(JNIEnv*, jclass, jlong);
  using SyncFunction = jboolean (*)(JNIEnv*, jclass, jlong, jobject, jboolean);
  const auto destroy = Native<DestroyFunctionType>("nativeDestroy");
  const auto sync = Native<SyncFunction>("nativeSyncNextTransaction");
  jlong handle = 0;
  CreateQueue(env, &handle);
  g_destroy = destroy;
  g_destroy_handle = handle;
  assert(sync(env, nullptr, handle, TestConsumer(), JNI_TRUE) == JNI_TRUE);
  assert(g_new_global_ref_count == 1);

  ASurfaceTransaction* incoming = ASurfaceTransaction_create();
  assert(g_stale_callback(g_stale_context, incoming, 17));
  assert(g_consumer_called);
  assert(g_window_release_count == 1);
  assert(g_delete_global_ref_count == 1);
  assert(g_detach_count == 1);
  assert(g_delete_global_quiesce == std::vector<bool>{false});
  assert(g_detach_quiesce == std::vector<bool>{false});
  // The Java wrapper owns this transaction after Consumer.accept. No native
  // apply/delete may happen before its eventual finalizer.
  assert(g_apply_count == 0);
  assert(g_delete_counts[incoming] == 0);
  assert(g_created_transactions.size() == 2);
  ASurfaceTransaction* disposal = g_created_transactions[1];
  assert(g_delete_counts[disposal] == 1);

  // The callback admission lease has now retired, so owner cleanup succeeds.
  assert(darwin_art::window::QuiesceBlastBufferQueues(env));
  ASurfaceTransaction* stale = ASurfaceTransaction_create();
  assert(g_stale_callback(g_stale_context, stale, 18));
  assert(g_apply_count == 0);
  assert(g_delete_counts[stale] == 1);
  ReleaseStaleObserver();
  ASurfaceTransaction_delete(incoming);
  assert(g_delete_counts[incoming] == 1);
  assert(darwin_art::window::RegisterBlastBufferQueueNatives(env));
}

void RunMergeOwnershipScenario(JNIEnv* env) {
  ResetObservations();
  assert(darwin_art::window::RegisterBlastBufferQueueNatives(env));
  using DestroyFunctionType = void (*)(JNIEnv*, jclass, jlong);
  using MergeFunction = void (*)(JNIEnv*, jclass, jlong, jlong, jlong);
  using ApplyPendingFunction = void (*)(JNIEnv*, jclass, jlong, jlong);
  const auto destroy = Native<DestroyFunctionType>("nativeDestroy");
  const auto merge = Native<MergeFunction>("nativeMergeWithNextTransaction");
  const auto apply_pending =
      Native<ApplyPendingFunction>("nativeApplyPendingTransactions");
  jlong handle = 0;
  CreateQueue(env, &handle);

  ASurfaceTransaction* future_source = ASurfaceTransaction_create();
  merge(env, nullptr, handle, reinterpret_cast<jlong>(future_source), 99);
  assert(g_apply_count == 0);
  assert(g_delete_counts[future_source] == 0);
  assert(g_created_transactions.size() == 2);
  ASurfaceTransaction* owned_future = g_created_transactions[1];
  apply_pending(env, nullptr, handle, 99);
  assert(g_apply_count == 1);
  assert(g_delete_counts[owned_future] == 1);
  assert(g_delete_counts[future_source] == 0);

  ASurfaceTransaction* due_source = ASurfaceTransaction_create();
  merge(env, nullptr, handle, reinterpret_cast<jlong>(due_source), 0);
  assert(g_apply_count == 2);
  assert(g_delete_counts[due_source] == 0);
  destroy(env, nullptr, handle);
  assert(g_window_release_count == 1);
  assert(darwin_art::window::QuiesceBlastBufferQueues(env));
  ReleaseStaleObserver();
  ASurfaceTransaction_delete(future_source);
  ASurfaceTransaction_delete(due_source);
  assert(g_delete_counts[future_source] == 1);
  assert(g_delete_counts[due_source] == 1);
  assert(darwin_art::window::RegisterBlastBufferQueueNatives(env));
}

void RunAttachFailureScenario(JNIEnv* env) {
  ResetObservations();
  assert(darwin_art::window::RegisterBlastBufferQueueNatives(env));
  using DestroyFunctionType = void (*)(JNIEnv*, jclass, jlong);
  using SyncFunction = jboolean (*)(JNIEnv*, jclass, jlong, jobject, jboolean);
  const auto destroy = Native<DestroyFunctionType>("nativeDestroy");
  const auto sync = Native<SyncFunction>("nativeSyncNextTransaction");
  jlong handle = 0;
  CreateQueue(env, &handle);
  assert(sync(env, nullptr, handle, TestConsumer(), JNI_TRUE) == JNI_TRUE);
  g_attach_fails = true;
  g_reentrant_transaction_delete = true;
  g_reentrant_transaction_destroy = true;
  g_destroy = destroy;
  g_destroy_handle = handle;
  ASurfaceTransaction* incoming = ASurfaceTransaction_create();
  assert(g_stale_callback(g_stale_context, incoming, 23));
  // Attach failure consumes this callback locally. The delete hook destroys
  // the queue while the callback lease is active, proving no JNI-state lock is
  // held across provider cleanup and that the State remains retained.
  assert(g_apply_count == 1);
  assert(g_delete_counts[incoming] == 1);
  assert(g_reentrant_sweep_called);
  assert(!g_reentrant_sweep_result);
  assert(g_delete_global_ref_count == 0);
  assert(g_detach_count == 0);

  g_reentrant_transaction_delete = false;
  assert(darwin_art::window::QuiesceBlastBufferQueues(env));
  assert(g_delete_global_ref_count == 1);
  assert(g_delete_global_quiesce == std::vector<bool>{false});
  assert(darwin_art::window::QuiesceBlastBufferQueues(env));

  // The deferred global is drained by the owner sweep. NativeDestroy already
  // ran reentrantly during incoming disposal, so no second destroy is legal.
  assert(g_window_release_count == 1);
  assert(g_last_window == nullptr);
  ReleaseStaleObserver();
  assert(darwin_art::window::RegisterBlastBufferQueueNatives(env));
}

}  // namespace

int main() {
  JNINativeInterface native_functions{};
  native_functions.FindClass = &FakeFindClass;
  native_functions.RegisterNatives = &FakeRegisterNatives;
  native_functions.DeleteLocalRef = &FakeDeleteLocalRef;
  native_functions.GetJavaVM = &FakeGetJavaVM;
  native_functions.GetObjectClass = &FakeGetObjectClass;
  native_functions.GetMethodID = &FakeGetMethodID;
  native_functions.NewObjectV = &FakeNewObjectV;
  native_functions.ExceptionCheck = &FakeExceptionCheck;
  native_functions.CallVoidMethodV = &FakeCallVoidMethodV;
  native_functions.NewGlobalRef = &FakeNewGlobalRef;
  native_functions.DeleteGlobalRef = &FakeDeleteGlobalRef;
  g_vm_functions.GetEnv = &FakeGetEnv;
  g_vm_functions.AttachCurrentThread = &FakeAttachCurrentThread;
  g_vm_functions.DetachCurrentThread = &FakeDetachCurrentThread;
  JNIEnv env{&native_functions};
  g_env = &env;
  g_quiesce_env = &env;
  RunMergeOwnershipScenario(&env);
  RunConsumerDestroyScenario(&env);
  RunAttachFailureScenario(&env);
  std::puts("blast-jni: PASS callback-owner lifetime, deferred attach cleanup, reentrant quiescence");
  return 0;
}
