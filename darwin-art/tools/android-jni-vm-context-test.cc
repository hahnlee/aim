#include "jni/android_varargs.h"
#include "jni/vm_context.h"

#include <array>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <memory>
#include <string>

#include <jni.h>

namespace {

JNIEnv *g_vm_env = nullptr;
JavaVM *g_vm = nullptr;
bool g_attached = true;
int g_get_env_calls = 0;
int g_normal_attach_calls = 0;
int g_daemon_attach_calls = 0;
int g_detach_calls = 0;
void *g_last_attach_arguments = nullptr;
jint g_normal_attach_status = JNI_OK;
jint g_daemon_attach_status = JNI_OK;
jint g_detach_status = JNI_OK;

int g_find_class_calls = 0;
const char *g_find_class_name_pointer = nullptr;
std::string g_find_class_name;
bool g_pending_exception = false;

int g_get_method_calls = 0;
const char *g_method_name_pointer = nullptr;
const char *g_method_signature_pointer = nullptr;
int g_call_int_calls = 0;
const jvalue *g_call_arguments = nullptr;

constexpr const char *kClassName = "example/Widget";
constexpr const char *kMissingClassName = "example/Missing";
constexpr const char *kMethodName = "combine";
constexpr const char *kMethodSignature = "(IJ)I";
constexpr uintptr_t kClassValue = 0x1111222233334444ull;
constexpr uintptr_t kObjectValue = 0x5555666677778888ull;
constexpr uintptr_t kMethodValue = 0x9999aaaabbbbccccull;
constexpr uintptr_t kAttachArgumentsValue = 0xddddeeeeffff0001ull;

jclass ClassValue() { return reinterpret_cast<jclass>(kClassValue); }
jobject ObjectValue() { return reinterpret_cast<jobject>(kObjectValue); }
jmethodID MethodValue() { return reinterpret_cast<jmethodID>(kMethodValue); }

jint GetEnv(JavaVM *vm, void **output, jint version) {
  assert(vm == g_vm);
  ++g_get_env_calls;
  if (version != JNI_VERSION_1_6) {
    *output = nullptr;
    return JNI_EVERSION;
  }
  if (!g_attached) {
    *output = nullptr;
    return JNI_EDETACHED;
  }
  *output = g_vm_env;
  return JNI_OK;
}

jint AttachCurrentThread(JavaVM *vm, JNIEnv **output, void *arguments) {
  assert(vm == g_vm);
  ++g_normal_attach_calls;
  g_last_attach_arguments = arguments;
  if (g_normal_attach_status == JNI_OK) {
    g_attached = true;
    *output = g_vm_env;
  } else {
    *output = nullptr;
  }
  return g_normal_attach_status;
}

jint AttachCurrentThreadAsDaemon(JavaVM *vm, JNIEnv **output, void *arguments) {
  assert(vm == g_vm);
  ++g_daemon_attach_calls;
  g_last_attach_arguments = arguments;
  if (g_daemon_attach_status == JNI_OK) {
    g_attached = true;
    *output = g_vm_env;
  } else {
    *output = nullptr;
  }
  return g_daemon_attach_status;
}

jint DetachCurrentThread(JavaVM *vm) {
  assert(vm == g_vm);
  ++g_detach_calls;
  if (g_detach_status == JNI_OK)
    g_attached = false;
  return g_detach_status;
}

jclass FindClass(JNIEnv *, const char *name) {
  assert(name != nullptr);
  ++g_find_class_calls;
  g_find_class_name_pointer = name;
  g_find_class_name = name;
  if (std::strcmp(name, kClassName) == 0)
    return ClassValue();
  g_pending_exception = true;
  return nullptr;
}

jmethodID GetMethodID(JNIEnv *, jclass clazz, const char *name,
                      const char *signature) {
  assert(clazz == ClassValue());
  ++g_get_method_calls;
  g_method_name_pointer = name;
  g_method_signature_pointer = signature;
  assert(std::strcmp(name, kMethodName) == 0);
  assert(std::strcmp(signature, kMethodSignature) == 0);
  return MethodValue();
}

jint CallIntMethodA(JNIEnv *, jobject object, jmethodID method,
                    const jvalue *arguments) {
  assert(object == ObjectValue());
  assert(method == MethodValue());
  assert(arguments != nullptr);
  ++g_call_int_calls;
  g_call_arguments = arguments;
  assert(arguments[0].i == static_cast<jint>(0x12345678));
  assert(arguments[1].j == static_cast<jlong>(0x1122334455667788ll));
  g_pending_exception = true; // Model an exception raised by Java, not on entry.
  // A negative result verifies the adapter's signed-to-uint64 representation.
  return static_cast<jint>(-7654321);
}

jint CallStaticIntMethodA(JNIEnv *, jclass, jmethodID, const jvalue *) {
  assert(false && "static method slot must not be selected");
  return 0;
}

class RecordingRegistration final : public darwin_art::jni::NativeRegistration {
public:
  explicit RecordingRegistration(int *destructed) : destructed_(destructed) {}
  ~RecordingRegistration() override { ++*destructed_; }

  jint Register(JNIEnv *env, jclass clazz,
                const DarwinArtJniNativeMethod *methods,
                jint count) noexcept override {
    assert(env == g_vm_env);
    assert(clazz == ClassValue());
    assert(methods == expected_methods_);
    assert(count == expected_count_);
    ++calls_;
    return JNI_OK;
  }

  const DarwinArtJniNativeMethod *expected_methods_ = nullptr;
  jint expected_count_ = 0;
  int calls_ = 0;

private:
  int *destructed_;
};

struct GuestArguments {
  alignas(16) std::array<std::uint8_t, 64> gp{};
  alignas(16) std::array<std::uint8_t, 128> fp{};
  alignas(16) std::array<std::uint8_t, 64> stack{};
  darwin_art::jni::AndroidArm64VaList args{stack.data(), gp.data() + gp.size(),
                                           fp.data() + fp.size(), -64, -128};

  GuestArguments() {
    const std::uint64_t integer = 0x0000000012345678ull;
    const std::uint64_t long_value = 0x1122334455667788ull;
    std::memcpy(gp.data(), &integer, sizeof(integer));
    std::memcpy(gp.data() + 8, &long_value, sizeof(long_value));
  }
};

void ResetJniObservations() {
  g_find_class_calls = 0;
  g_find_class_name_pointer = nullptr;
  g_find_class_name.clear();
  g_pending_exception = false;
  g_get_method_calls = 0;
  g_method_name_pointer = nullptr;
  g_method_signature_pointer = nullptr;
  g_call_int_calls = 0;
  g_call_arguments = nullptr;
}

} // namespace

int main() {
  JNINativeInterface native_functions{};
  native_functions.FindClass = FindClass;
  native_functions.GetMethodID = GetMethodID;
  native_functions.CallIntMethodA = CallIntMethodA;
  native_functions.CallStaticIntMethodA = CallStaticIntMethodA;
  // RegisterNatives, ExceptionClear and all unrelated slots remain null.
  JNIEnv env{&native_functions};
  g_vm_env = &env;

  JNIInvokeInterface vm_functions{};
  vm_functions.GetEnv = GetEnv;
  vm_functions.AttachCurrentThread = AttachCurrentThread;
  vm_functions.AttachCurrentThreadAsDaemon = AttachCurrentThreadAsDaemon;
  vm_functions.DetachCurrentThread = DetachCurrentThread;
  JavaVM vm{&vm_functions};
  g_vm = &vm;

  int registrations_destroyed = 0;
  auto registration =
      std::make_shared<RecordingRegistration>(&registrations_destroyed);
  std::weak_ptr<RecordingRegistration> retained = registration;
  assert(!darwin_art::jni::VmContext::Create(nullptr, registration));
  assert(!darwin_art::jni::VmContext::Create(&vm, nullptr));
  auto context = darwin_art::jni::VmContext::Create(&vm, registration);
  assert(context);
  registration.reset();
  assert(!retained.expired());
  auto recorder =
      std::static_pointer_cast<RecordingRegistration>(retained.lock());
  assert(recorder);

  auto backend = context->Backend();
  assert(backend.context != nullptr);
  assert(backend.current_env(backend.context) == &env);
  assert(g_get_env_calls == 1);

  g_attached = false;
  const int get_calls_before_detached = g_get_env_calls;
  const int attaches_before_detached =
      g_normal_attach_calls + g_daemon_attach_calls;
  assert(backend.current_env(backend.context) == nullptr);
  assert(g_get_env_calls == get_calls_before_detached + 1);
  assert(g_normal_attach_calls + g_daemon_attach_calls ==
         attaches_before_detached);
  g_attached = true;

  g_normal_attach_status = JNI_OK;
  void *attach_arguments = reinterpret_cast<void *>(kAttachArgumentsValue);
  assert(backend.attach_current_thread(backend.context, attach_arguments, 0) ==
         JNI_OK);
  assert(g_normal_attach_calls == 1);
  assert(g_daemon_attach_calls == 0);
  assert(g_last_attach_arguments == attach_arguments);

  g_daemon_attach_status = JNI_EVERSION;
  assert(backend.attach_current_thread(backend.context, attach_arguments, 1) ==
         JNI_EVERSION);
  assert(g_daemon_attach_calls == 1);
  assert(g_last_attach_arguments == attach_arguments);

  g_detach_status = JNI_ERR;
  assert(backend.detach_current_thread(backend.context) == JNI_ERR);
  assert(g_detach_calls == 1);
  g_detach_status = JNI_OK;
  assert(backend.detach_current_thread(backend.context) == JNI_OK);
  assert(g_detach_calls == 2);
  g_attached = true;

  ResetJniObservations();
  const char class_name[] = "example/Widget";
  assert(backend.find_class(backend.context, class_name) == ClassValue());
  assert(g_find_class_calls == 1);
  assert(g_find_class_name_pointer == class_name);
  assert(g_find_class_name == kClassName);
  assert(!g_pending_exception);
  const char missing_name[] = "example/Missing";
  assert(backend.find_class(backend.context, missing_name) == nullptr);
  assert(g_find_class_calls == 2);
  assert(g_find_class_name_pointer == missing_name);
  assert(g_find_class_name == kMissingClassName);
  assert(g_pending_exception);

  DarwinArtJniNativeMethod methods[] = {
      {"nativeCombine", "(IJ)I", reinterpret_cast<void *>(uintptr_t{0x42})}};
  g_pending_exception = false;
  recorder->expected_methods_ = methods;
  recorder->expected_count_ = 1;
  assert(backend.register_natives(backend.context, ClassValue(), methods, 1) ==
         JNI_OK);
  assert(recorder->calls_ == 1);

  GuestArguments guest_arguments;
  // A method ID that was never looked up has no descriptor and must not reach
  // any JNI call. This is a contract rejection, not a claim of full JNI ABI.
  g_call_int_calls = 0;
  assert(backend.call_method_v(backend.context, ObjectValue(),
                               reinterpret_cast<void *>(uintptr_t{0x77}),
                               &guest_arguments.args, 'I', 0) == 0);
  assert(g_call_int_calls == 0);

  ResetJniObservations();
  const char method_name[] = "combine";
  const char method_signature[] = "(IJ)I";
  assert(backend.get_method_id(backend.context, ClassValue(), method_name,
                               method_signature, 0) == MethodValue());
  assert(g_get_method_calls == 1);
  assert(g_method_name_pointer == method_name);
  assert(g_method_signature_pointer == method_signature);

  g_call_int_calls = 0;
  g_pending_exception = false;
  const std::uint64_t expected_negative =
      static_cast<std::uint64_t>(static_cast<std::int64_t>(-7654321));
  assert(backend.call_method_v(backend.context, ObjectValue(), MethodValue(),
                               &guest_arguments.args, 'I',
                               0) == expected_negative);
  assert(g_call_int_calls == 1);
  assert(g_call_arguments != nullptr);
  assert(g_pending_exception);

  recorder.reset();
  context.reset();
  assert(retained.expired());
  assert(registrations_destroyed == 1);
  std::puts(
      "android-jni-vm-context: PASS env-no-autoattach attach-daemon-detach "
      "direct-findclass registration-owner descriptor-cache translated-call");
  return 0;
}
