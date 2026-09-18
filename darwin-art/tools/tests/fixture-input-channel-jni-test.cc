#include "../../probes/fixture_input_channel.h"

#include <jni.h>

#include <cassert>
#include <cstdarg>
#include <string>
#include <unordered_map>
#include <vector>

namespace {

struct FakeObject {
  std::string class_name;
  int fd = -1;
};

struct FakeMethod {
  std::string name;
  std::string signature;
};

struct FakeVm final {
  std::vector<FakeObject*> objects;
  std::vector<FakeMethod*> methods;
  std::unordered_map<std::string, jclass> classes;
  bool pending = false;
  jthrowable pending_exception = nullptr;
  int global_calls = 0;
  std::unordered_map<jobject, int> globals;
  int fail_global_call = 0;
  int close_calls = 0;
  int recycle_calls = 0;
  int receiver_dispose_calls = 0;
  int channel_dispose_calls = 0;
  bool socketpair_failed = false;
  jobject parcel_token = nullptr;
  jobject parcel_fd = nullptr;
  jobject constructed_root = nullptr;
  jobject constructed_channel = nullptr;
  jobject constructed_looper = nullptr;
  std::vector<std::string> calls;

  ~FakeVm() {
    for (FakeMethod* method : methods) delete method;
    for (FakeObject* object : objects) delete object;
  }

  jobject NewObjectHandle(const std::string& class_name) {
    auto* object = new FakeObject{class_name};
    objects.push_back(object);
    return reinterpret_cast<jobject>(object);
  }

  jclass Class(const char* class_name) {
    const auto found = classes.find(class_name);
    if (found != classes.end()) return found->second;
    jclass value = reinterpret_cast<jclass>(NewObjectHandle(class_name));
    classes.emplace(class_name, value);
    return value;
  }

  FakeObject* Object(jobject value) {
    return reinterpret_cast<FakeObject*>(value);
  }

  FakeMethod* Method(jmethodID value) {
    return reinterpret_cast<FakeMethod*>(value);
  }
};

FakeVm* g_vm = nullptr;

jclass FindClass(JNIEnv*, const char* name) { return g_vm->Class(name); }

jclass GetObjectClass(JNIEnv*, jobject object) {
  return g_vm->Class(g_vm->Object(object)->class_name.c_str());
}

jmethodID FindMethod(jclass, const char* name, const char* signature) {
  auto* method = new FakeMethod{name, signature};
  g_vm->methods.push_back(method);
  return reinterpret_cast<jmethodID>(method);
}

jmethodID GetMethodID(JNIEnv*, jclass clazz, const char* name,
                      const char* signature) {
  return FindMethod(clazz, name, signature);
}

jmethodID GetStaticMethodID(JNIEnv*, jclass clazz, const char* name,
                            const char* signature) {
  return FindMethod(clazz, name, signature);
}

jobject NewObjectV(JNIEnv*, jclass clazz, jmethodID method, va_list args) {
  const std::string class_name = g_vm->Object(reinterpret_cast<jobject>(clazz))->class_name;
  auto* method_info = g_vm->Method(method);
  if (method_info->name == "<init>" &&
      class_name == "android/view/ViewRootImpl$WindowInputEventReceiver") {
    g_vm->constructed_root = va_arg(args, jobject);
    g_vm->constructed_channel = va_arg(args, jobject);
    g_vm->constructed_looper = va_arg(args, jobject);
  }
  g_vm->calls.push_back(class_name + "::<init>");
  return g_vm->NewObjectHandle(class_name);
}

jobject NewGlobalRef(JNIEnv*, jobject object) {
  ++g_vm->global_calls;
  if (g_vm->fail_global_call == g_vm->global_calls) {
    g_vm->pending = true;
    g_vm->pending_exception = reinterpret_cast<jthrowable>(
        g_vm->NewObjectHandle("java/lang/OutOfMemoryError"));
    return nullptr;
  }
  ++g_vm->globals[object];
  return object;
}

void DeleteGlobalRef(JNIEnv*, jobject object) {
  const auto found = g_vm->globals.find(object);
  assert(found != g_vm->globals.end() && found->second > 0);
  if (--found->second == 0) g_vm->globals.erase(found);
}
void DeleteLocalRef(JNIEnv*, jobject) {}

jstring NewStringUTF(JNIEnv*, const char*) {
  return reinterpret_cast<jstring>(g_vm->NewObjectHandle("java/lang/String"));
}

void CallStaticVoidMethodV(JNIEnv*, jclass clazz, jmethodID method, va_list args) {
  const std::string class_name = g_vm->Object(reinterpret_cast<jobject>(clazz))->class_name;
  const std::string method_name = g_vm->Method(method)->name;
  g_vm->calls.push_back(class_name + "::" + method_name);
  if (method_name == "socketpair") {
    (void)va_arg(args, jint);
    (void)va_arg(args, jint);
    (void)va_arg(args, jint);
    auto* fd_a = g_vm->Object(va_arg(args, jobject));
    auto* fd_b = g_vm->Object(va_arg(args, jobject));
    if (g_vm->socketpair_failed) {
      g_vm->pending = true;
      g_vm->pending_exception = reinterpret_cast<jthrowable>(
          g_vm->NewObjectHandle("android/system/ErrnoException"));
      fd_a->fd = 41;
      return;
    }
    fd_a->fd = 41;
    fd_b->fd = 42;
    return;
  }
  if (method_name == "close") {
    auto* fd = g_vm->Object(va_arg(args, jobject));
    ++g_vm->close_calls;
    fd->fd = -1;
  }
}

jobject CallStaticObjectMethodV(JNIEnv*, jclass clazz, jmethodID method,
                                va_list) {
  const std::string class_name = g_vm->Object(reinterpret_cast<jobject>(clazz))->class_name;
  const std::string method_name = g_vm->Method(method)->name;
  g_vm->calls.push_back(class_name + "::" + method_name);
  assert(method_name == "obtain");
  return g_vm->NewObjectHandle("android/os/Parcel");
}

void CallVoidMethodV(JNIEnv*, jobject object, jmethodID method, va_list args) {
  const std::string method_name = g_vm->Method(method)->name;
  g_vm->calls.push_back(g_vm->Object(object)->class_name + "::" + method_name);
  if (method_name == "writeInt") {
    assert(va_arg(args, jint) == 1);
  } else if (method_name == "writeStrongBinder" ||
             method_name == "writeString" || method_name == "readFromParcel") {
    jobject argument = va_arg(args, jobject);
    if (method_name == "writeStrongBinder") g_vm->parcel_token = argument;
  } else if (method_name == "writeFileDescriptor") {
    g_vm->parcel_fd = va_arg(args, jobject);
  } else if (method_name == "setDataPosition") {
    assert(va_arg(args, jint) == 0);
  } else if (method_name == "recycle") {
    ++g_vm->recycle_calls;
  } else if (method_name == "dispose") {
    if (g_vm->Object(object)->class_name ==
        "android/view/ViewRootImpl$WindowInputEventReceiver") {
      ++g_vm->receiver_dispose_calls;
    } else if (g_vm->Object(object)->class_name == "android/view/InputChannel") {
      ++g_vm->channel_dispose_calls;
    }
  }
}

jthrowable ExceptionOccurred(JNIEnv*) {
  return g_vm->pending ? g_vm->pending_exception : nullptr;
}
jboolean ExceptionCheck(JNIEnv*) { return g_vm->pending ? JNI_TRUE : JNI_FALSE; }
void ExceptionClear(JNIEnv*) {
  g_vm->pending = false;
  g_vm->pending_exception = nullptr;
}
jint ThrowNew(JNIEnv*, jclass clazz, const char*) {
  g_vm->pending = true;
  g_vm->pending_exception = reinterpret_cast<jthrowable>(clazz);
  return JNI_OK;
}
jint Throw(JNIEnv*, jthrowable exception) {
  g_vm->pending = true;
  g_vm->pending_exception = exception;
  return JNI_OK;
}

JNIEnv MakeEnv(FakeVm* vm) {
  g_vm = vm;
  static JNINativeInterface table{};
  table.FindClass = &FindClass;
  table.GetObjectClass = &GetObjectClass;
  table.GetMethodID = &GetMethodID;
  table.GetStaticMethodID = &GetStaticMethodID;
  table.NewObjectV = &NewObjectV;
  table.NewGlobalRef = &NewGlobalRef;
  table.DeleteGlobalRef = &DeleteGlobalRef;
  table.DeleteLocalRef = &DeleteLocalRef;
  table.NewStringUTF = &NewStringUTF;
  table.CallStaticVoidMethodV = &CallStaticVoidMethodV;
  table.CallStaticObjectMethodV = &CallStaticObjectMethodV;
  table.CallVoidMethodV = &CallVoidMethodV;
  table.ExceptionOccurred = &ExceptionOccurred;
  table.ExceptionCheck = &ExceptionCheck;
  table.ExceptionClear = &ExceptionClear;
  table.ThrowNew = &ThrowNew;
  table.Throw = &Throw;
  return JNIEnv{&table};
}

void TestSuccessAndPendingException() {
  FakeVm vm;
  JNIEnv env = MakeEnv(&vm);
  jobject root = vm.NewObjectHandle("android/view/ViewRootImpl");
  jobject looper = vm.NewObjectHandle("android/os/Looper");
  darwin_art_graphics_fixture::FixtureInputChannel channel;
  assert(channel.Initialize(&env, root, looper, "fixture-imported"));
  assert(channel.producer_fd() != nullptr && channel.receiver_fd() != nullptr);
  assert(vm.recycle_calls == 1);
  const std::vector<std::string> expected_calls = {
      "java/io/FileDescriptor::<init>",
      "java/io/FileDescriptor::<init>",
      "android/system/Os::socketpair",
      "android/os/Binder::<init>",
      "android/os/Parcel::obtain",
      "android/os/Parcel::writeInt",
      "android/os/Parcel::writeStrongBinder",
      "android/os/Parcel::writeString",
      "android/os/Parcel::writeFileDescriptor",
      "android/os/Parcel::setDataPosition",
      "android/view/InputChannel::<init>",
      "android/view/InputChannel::readFromParcel",
      "android/os/Parcel::recycle",
      "android/view/ViewRootImpl$WindowInputEventReceiver::<init>",
  };
  assert(vm.calls == expected_calls);
  assert(vm.parcel_token == channel.token());
  assert(vm.parcel_fd == channel.receiver_fd());
  assert(vm.constructed_root == channel.view_root());
  assert(vm.constructed_channel == channel.channel());
  assert(vm.constructed_looper == looper);
  const auto original = reinterpret_cast<jthrowable>(
      vm.NewObjectHandle("java/lang/RuntimeException"));
  vm.pending = true;
  vm.pending_exception = original;
  channel.Dispose(&env);
  assert(vm.pending && vm.pending_exception == original);
  assert(vm.close_calls == 2);
  assert(vm.receiver_dispose_calls == 1 && vm.channel_dispose_calls == 1);
  assert(vm.globals.empty());
  channel.Dispose(&env);
  assert(vm.close_calls == 2);
  assert(vm.receiver_dispose_calls == 1 && vm.channel_dispose_calls == 1);
}

void TestGlobalPromotionFailure(int failed_call) {
  FakeVm vm;
  vm.fail_global_call = failed_call;
  JNIEnv env = MakeEnv(&vm);
  jobject root = vm.NewObjectHandle("android/view/ViewRootImpl");
  jobject looper = vm.NewObjectHandle("android/os/Looper");
  darwin_art_graphics_fixture::FixtureInputChannel channel;
  assert(!channel.Initialize(&env, root, looper, "oom"));
  assert(vm.close_calls == 2);
  assert(vm.recycle_calls == (failed_call >= 4 ? 1 : 0));
  assert(vm.receiver_dispose_calls == (failed_call == 6 ? 1 : 0));
  assert(vm.channel_dispose_calls == (failed_call >= 4 ? 1 : 0));
  assert(vm.pending && vm.globals.empty());
  assert(vm.Object(reinterpret_cast<jobject>(vm.pending_exception))->class_name ==
         "java/lang/OutOfMemoryError");
  channel.Dispose(&env);
  assert(vm.close_calls == 2 && vm.globals.empty());
  assert(!channel.Initialize(&env, root, looper, "one-shot"));
}

void TestSocketpairFailureClosesAssignedDescriptors() {
  FakeVm vm;
  vm.socketpair_failed = true;
  JNIEnv env = MakeEnv(&vm);
  jobject root = vm.NewObjectHandle("android/view/ViewRootImpl");
  jobject looper = vm.NewObjectHandle("android/os/Looper");
  darwin_art_graphics_fixture::FixtureInputChannel channel;
  assert(!channel.Initialize(&env, root, looper, "socketpair-failure"));
  assert(vm.close_calls == 2);
  assert(vm.pending);
}

}  // namespace

int main() {
  TestSuccessAndPendingException();
  for (int failed_call = 1; failed_call <= 6; ++failed_call)
    TestGlobalPromotionFailure(failed_call);
  TestSocketpairFailureClosesAssignedDescriptors();
  return 0;
}
