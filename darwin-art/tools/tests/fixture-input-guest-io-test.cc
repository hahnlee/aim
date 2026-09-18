#include "../../probes/fixture_input_guest_io.h"

#include <jni.h>

#include <cassert>
#include <cstdarg>
#include <cstring>
#include <string>
#include <unordered_map>
#include <vector>

namespace {

struct Object {
  std::string class_name;
  jint errno_value = 0;
  std::vector<jbyte> bytes;
};
struct Method {
  std::string name;
  std::string signature;
};

struct FakeVm final {
  std::vector<Object*> objects;
  std::vector<Method*> methods;
  std::unordered_map<std::string, jclass> classes;
  bool pending = false;
  jthrowable exception = nullptr;
  jint io_result = 0;
  jint io_errno = 0;
  std::string io_exception_class;
  bool allocation_fails = false;
  bool set_region_fails = false;
  bool get_region_fails = false;
  int read_calls = 0;
  int write_calls = 0;

  ~FakeVm() {
    for (Method* method : methods) delete method;
    for (Object* object : objects) delete object;
  }
  jobject NewObject(const char* class_name) {
    auto* object = new Object{class_name, 0, {}};
    objects.push_back(object);
    return reinterpret_cast<jobject>(object);
  }
  Object* Get(jobject object) { return reinterpret_cast<Object*>(object); }
  jclass Class(const char* class_name) {
    const auto found = classes.find(class_name);
    if (found != classes.end()) return found->second;
    jclass value = reinterpret_cast<jclass>(NewObject(class_name));
    classes.emplace(class_name, value);
    return value;
  }
  Method* GetMethod(jmethodID method) {
    return reinterpret_cast<Method*>(method);
  }
  void Raise(const char* class_name, jint error = 0) {
    exception = reinterpret_cast<jthrowable>(NewObject(class_name));
    Get(reinterpret_cast<jobject>(exception))->errno_value = error;
    pending = true;
  }
};

FakeVm* g_vm = nullptr;

jclass FindClass(JNIEnv*, const char* name) { return g_vm->Class(name); }
jmethodID MethodLookup(jclass, const char* name, const char* signature) {
  auto* method = new Method{name, signature};
  g_vm->methods.push_back(method);
  return reinterpret_cast<jmethodID>(method);
}
jmethodID GetStaticMethodID(JNIEnv*, jclass clazz, const char* name,
                            const char* signature) {
  return MethodLookup(clazz, name, signature);
}
jfieldID GetFieldID(JNIEnv*, jclass, const char* name, const char*) {
  assert(std::strcmp(name, "errno") == 0);
  return reinterpret_cast<jfieldID>(0x31);
}
jbyteArray NewByteArray(JNIEnv*, jsize length) {
  if (g_vm->allocation_fails) {
    g_vm->Raise("java/lang/OutOfMemoryError");
    return nullptr;
  }
  jobject object = g_vm->NewObject("[B");
  g_vm->Get(object)->bytes.resize(static_cast<size_t>(length));
  return reinterpret_cast<jbyteArray>(object);
}
void SetByteArrayRegion(JNIEnv*, jbyteArray array, jsize start, jsize length,
                        const jbyte* data) {
  if (g_vm->set_region_fails) {
    g_vm->Raise("java/lang/RuntimeException");
    return;
  }
  auto* object = g_vm->Get(reinterpret_cast<jobject>(array));
  std::memcpy(object->bytes.data() + start, data, static_cast<size_t>(length));
}
void GetByteArrayRegion(JNIEnv*, jbyteArray array, jsize start, jsize length,
                        jbyte* data) {
  if (g_vm->get_region_fails) {
    g_vm->Raise("java/lang/RuntimeException");
    return;
  }
  auto* object = g_vm->Get(reinterpret_cast<jobject>(array));
  std::memcpy(data, object->bytes.data() + start, static_cast<size_t>(length));
}

jint CallStaticIntMethodV(JNIEnv*, jclass, jmethodID method, va_list args) {
  const std::string name = g_vm->GetMethod(method)->name;
  (void)va_arg(args, jobject);  // FileDescriptor
  auto* array = g_vm->Get(va_arg(args, jobject));
  const jint offset = va_arg(args, jint);
  const jint count = va_arg(args, jint);
  assert(offset == 0 && count == static_cast<jint>(array->bytes.size()));
  if (name == "write") {
    ++g_vm->write_calls;
  } else {
    assert(name == "read");
    ++g_vm->read_calls;
    for (jint i = 0; i < g_vm->io_result && i < count; ++i)
      array->bytes[static_cast<size_t>(i)] = static_cast<jbyte>('a' + i);
  }
  if (!g_vm->io_exception_class.empty()) {
    g_vm->Raise(g_vm->io_exception_class.c_str(), g_vm->io_errno);
  }
  return g_vm->io_result;
}

jboolean IsInstanceOf(JNIEnv*, jobject object, jclass clazz) {
  return g_vm->Get(object)->class_name ==
                 g_vm->Get(reinterpret_cast<jobject>(clazz))->class_name
             ? JNI_TRUE
             : JNI_FALSE;
}
jint GetIntField(JNIEnv*, jobject object, jfieldID) {
  return g_vm->Get(object)->errno_value;
}
jthrowable ExceptionOccurred(JNIEnv*) {
  return g_vm->pending ? g_vm->exception : nullptr;
}
jboolean ExceptionCheck(JNIEnv*) { return g_vm->pending ? JNI_TRUE : JNI_FALSE; }
void ExceptionClear(JNIEnv*) {
  g_vm->pending = false;
  g_vm->exception = nullptr;
}
jint ThrowNew(JNIEnv*, jclass clazz, const char*) {
  g_vm->pending = true;
  g_vm->exception = reinterpret_cast<jthrowable>(clazz);
  return JNI_OK;
}
jint Throw(JNIEnv*, jthrowable exception) {
  g_vm->pending = true;
  g_vm->exception = exception;
  return JNI_OK;
}
void DeleteLocalRef(JNIEnv*, jobject) {}
jobject NewLocalRef(JNIEnv*, jobject object) { return object; }

JNIEnv MakeEnv(FakeVm* vm) {
  g_vm = vm;
  static JNINativeInterface table{};
  table.FindClass = &FindClass;
  table.GetStaticMethodID = &GetStaticMethodID;
  table.GetFieldID = &GetFieldID;
  table.NewByteArray = &NewByteArray;
  table.SetByteArrayRegion = &SetByteArrayRegion;
  table.GetByteArrayRegion = &GetByteArrayRegion;
  table.CallStaticIntMethodV = &CallStaticIntMethodV;
  table.IsInstanceOf = &IsInstanceOf;
  table.GetIntField = &GetIntField;
  table.ExceptionOccurred = &ExceptionOccurred;
  table.ExceptionCheck = &ExceptionCheck;
  table.ExceptionClear = &ExceptionClear;
  table.ThrowNew = &ThrowNew;
  table.Throw = &Throw;
  table.DeleteLocalRef = &DeleteLocalRef;
  table.NewLocalRef = &NewLocalRef;
  return JNIEnv{&table};
}

darwin_art_graphics_fixture::FixtureInputGuestIo MakeAdapter(
    FakeVm* vm, JNIEnv* env, jobject* fd) {
  *fd = vm->NewObject("java/io/FileDescriptor");
  return darwin_art_graphics_fixture::FixtureInputGuestIo(env, *fd);
}

void TestProgressAndCounts() {
  FakeVm vm;
  JNIEnv env = MakeEnv(&vm);
  jobject fd = nullptr;
  auto adapter = MakeAdapter(&vm, &env, &fd);
  assert(adapter.IsValid());
  auto io = adapter.port();
  const char payload[] = "hello";
  vm.io_result = 3;
  auto wrote = io.write(io.context, payload, 5);
  assert(wrote.status == darwin_art_graphics_fixture::FixtureIoStatus::kProgress &&
         wrote.bytes == 3 && vm.write_calls == 1);
  char result[5]{};
  vm.io_result = 4;
  auto read = io.read(io.context, result, sizeof(result));
  assert(read.status == darwin_art_graphics_fixture::FixtureIoStatus::kProgress &&
         read.bytes == 4 && vm.read_calls == 1 && result[0] == 'a' && result[3] == 'd');
  vm.io_result = 6;
  assert(io.write(io.context, payload, 5).status ==
         darwin_art_graphics_fixture::FixtureIoStatus::kTerminal);
  assert(io.read(io.context, result, sizeof(result)).status ==
         darwin_art_graphics_fixture::FixtureIoStatus::kTerminal);
  assert(io.write(io.context, nullptr, 0).status ==
         darwin_art_graphics_fixture::FixtureIoStatus::kProgress);
  assert(io.read(io.context, nullptr, 0).status ==
         darwin_art_graphics_fixture::FixtureIoStatus::kProgress);
}

void TestWouldBlockAndInterruptedAreCleared() {
  for (jint error : {11, 4}) {
    FakeVm vm;
    JNIEnv env = MakeEnv(&vm);
    jobject fd = nullptr;
    auto adapter = MakeAdapter(&vm, &env, &fd);
    auto io = adapter.port();
    vm.io_result = -1;
    vm.io_errno = error;
    vm.io_exception_class = "android/system/ErrnoException";
    char byte = 0;
    auto result = io.read(io.context, &byte, 1);
    assert(result.status == darwin_art_graphics_fixture::FixtureIoStatus::kWouldBlock &&
           result.bytes == 0 && !vm.pending);
  }
}

void TestTerminalAndPendingPreservation() {
  FakeVm vm;
  JNIEnv env = MakeEnv(&vm);
  jobject fd = nullptr;
  auto adapter = MakeAdapter(&vm, &env, &fd);
  auto io = adapter.port();
  vm.io_result = -1;
  vm.io_errno = 32;
  vm.io_exception_class = "android/system/ErrnoException";
  const auto nonretryable = io.write(io.context, "x", 1);
  assert(nonretryable.status == darwin_art_graphics_fixture::FixtureIoStatus::kTerminal &&
         vm.pending);
  const jthrowable original = vm.exception;
  char no_retry_byte = 0;
  const auto no_retry = io.read(io.context, &no_retry_byte, 1);
  assert(no_retry.status == darwin_art_graphics_fixture::FixtureIoStatus::kTerminal &&
         vm.exception == original && vm.read_calls == 0);

  vm.pending = false;
  vm.exception = nullptr;
  vm.io_exception_class.clear();
  vm.io_result = 0;
  char byte = 0;
  assert(io.read(io.context, &byte, 1).status ==
         darwin_art_graphics_fixture::FixtureIoStatus::kTerminal);
  assert(io.write(io.context, nullptr, 1).status ==
         darwin_art_graphics_fixture::FixtureIoStatus::kTerminal && vm.pending);

  vm.pending = false;
  vm.exception = nullptr;
  vm.io_exception_class = "java/lang/RuntimeException";
  vm.io_result = -1;
  const auto generic = io.write(io.context, "x", 1);
  assert(generic.status == darwin_art_graphics_fixture::FixtureIoStatus::kTerminal &&
         vm.pending);
}

void TestAllocationAndCopyFailures() {
  for (bool set_failure : {false, true}) {
    FakeVm vm;
    JNIEnv env = MakeEnv(&vm);
    jobject fd = nullptr;
    auto adapter = MakeAdapter(&vm, &env, &fd);
    auto io = adapter.port();
    vm.io_result = 1;
    if (set_failure) vm.set_region_fails = true;
    else vm.allocation_fails = true;
    const auto result = io.write(io.context, "x", 1);
    assert(result.status == darwin_art_graphics_fixture::FixtureIoStatus::kTerminal &&
           vm.pending && vm.write_calls == 0);
  }
  FakeVm vm;
  JNIEnv env = MakeEnv(&vm);
  jobject fd = nullptr;
  auto adapter = MakeAdapter(&vm, &env, &fd);
  auto io = adapter.port();
  vm.io_result = 1;
  vm.get_region_fails = true;
  char byte = 0;
  assert(io.read(io.context, &byte, 1).status ==
         darwin_art_graphics_fixture::FixtureIoStatus::kTerminal);
  assert(vm.pending);
}

}  // namespace

int main() {
  TestProgressAndCounts();
  TestWouldBlockAndInterruptedAreCleared();
  TestTerminalAndPendingPreservation();
  TestAllocationAndCopyFailures();
  return 0;
}
