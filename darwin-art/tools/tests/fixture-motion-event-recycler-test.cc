#include "../../probes/fixture_motion_event_recycler.h"

#include <jni.h>

#include <cassert>
#include <cstdarg>
#include <string>
#include <vector>

namespace {

struct Object {
  std::string class_name;
};

struct FakeVm final {
  std::vector<Object*> objects;
  bool pending = false;
  jthrowable exception = nullptr;
  bool new_local_fails = false;
  bool new_local_sets_pending = false;
  bool recycle_throws = false;
  int new_local_calls = 0;
  int delete_local_calls = 0;
  int recycle_calls = 0;

  ~FakeVm() {
    for (Object* object : objects) delete object;
  }
  jobject NewObject(const char* class_name) {
    auto* object = new Object{class_name};
    objects.push_back(object);
    return reinterpret_cast<jobject>(object);
  }
  void Raise(const char* class_name) {
    exception = reinterpret_cast<jthrowable>(NewObject(class_name));
    pending = true;
  }
};

FakeVm* g_vm = nullptr;
jmethodID const kRecycle = reinterpret_cast<jmethodID>(0x71);

jobject NewLocalRef(JNIEnv*, jobject object) {
  ++g_vm->new_local_calls;
  if (g_vm->new_local_fails) {
    if (g_vm->new_local_sets_pending) g_vm->Raise("java/lang/OutOfMemoryError");
    return nullptr;
  }
  return object;
}
void DeleteLocalRef(JNIEnv*, jobject) { ++g_vm->delete_local_calls; }
jboolean ExceptionCheck(JNIEnv*) { return g_vm->pending ? JNI_TRUE : JNI_FALSE; }
jthrowable ExceptionOccurred(JNIEnv*) {
  return g_vm->pending ? g_vm->exception : nullptr;
}
void ExceptionClear(JNIEnv*) {
  g_vm->pending = false;
  g_vm->exception = nullptr;
}
jint Throw(JNIEnv*, jthrowable exception) {
  g_vm->pending = true;
  g_vm->exception = exception;
  return JNI_OK;
}
void CallVoidMethodV(JNIEnv*, jobject, jmethodID method, va_list) {
  assert(method == kRecycle);
  ++g_vm->recycle_calls;
  if (g_vm->recycle_throws) g_vm->Raise("java/lang/RuntimeException");
}

JNIEnv MakeEnv(FakeVm* vm) {
  g_vm = vm;
  static JNINativeInterface table{};
  table.NewLocalRef = &NewLocalRef;
  table.DeleteLocalRef = &DeleteLocalRef;
  table.ExceptionCheck = &ExceptionCheck;
  table.ExceptionOccurred = &ExceptionOccurred;
  table.ExceptionClear = &ExceptionClear;
  table.Throw = &Throw;
  table.CallVoidMethodV = &CallVoidMethodV;
  return JNIEnv{&table};
}

void TestExplicitAndDestructorFallback() {
  FakeVm vm;
  JNIEnv env = MakeEnv(&vm);
  jobject event = vm.NewObject("android/view/MotionEvent");
  {
    darwin_art_graphics_fixture::FixtureMotionEventRecycler recycler(
        &env, event, kRecycle);
    assert(recycler.valid());
    assert(recycler.Recycle());
    assert(recycler.Recycle());
    assert(vm.recycle_calls == 1);
  }
  assert(vm.recycle_calls == 1 && vm.delete_local_calls == 1);

  {
    darwin_art_graphics_fixture::FixtureMotionEventRecycler recycler(
        &env, event, kRecycle);
    assert(recycler.valid());
  }
  assert(vm.recycle_calls == 2 && vm.delete_local_calls == 2);
}

void TestEarlyAndPromotionFailure() {
  FakeVm vm;
  JNIEnv env = MakeEnv(&vm);
  jobject event = vm.NewObject("android/view/MotionEvent");
  {
    darwin_art_graphics_fixture::FixtureMotionEventRecycler invalid(
        &env, event, nullptr);
    assert(!invalid.valid() && vm.recycle_calls == 0);
  }
  vm.new_local_fails = true;
  vm.new_local_sets_pending = true;
  const jthrowable original = reinterpret_cast<jthrowable>(
      vm.NewObject("java/lang/OutOfMemoryError"));
  vm.pending = true;
  vm.exception = original;
  {
    darwin_art_graphics_fixture::FixtureMotionEventRecycler failed(
        &env, event, kRecycle);
    assert(!failed.valid());
    assert(vm.recycle_calls == 1 && vm.pending && vm.exception == original);
  }
  assert(vm.recycle_calls == 1);

  vm.pending = false;
  vm.exception = nullptr;
  vm.new_local_sets_pending = false;
  {
    darwin_art_graphics_fixture::FixtureMotionEventRecycler failed(
        &env, event, kRecycle);
    assert(!failed.valid() && vm.recycle_calls == 2 && !vm.pending);
  }
}

void TestCleanupExceptions() {
  FakeVm vm;
  JNIEnv env = MakeEnv(&vm);
  jobject event = vm.NewObject("android/view/MotionEvent");
  {
    darwin_art_graphics_fixture::FixtureMotionEventRecycler recycler(
        &env, event, kRecycle);
    vm.recycle_throws = true;
    assert(!recycler.Recycle() && vm.pending && vm.recycle_calls == 1);
    assert(!recycler.Recycle() && vm.recycle_calls == 1);
  }
  assert(vm.recycle_calls == 1);

  vm.pending = false;
  vm.exception = nullptr;
  vm.recycle_throws = false;
  {
    darwin_art_graphics_fixture::FixtureMotionEventRecycler recycler(
        &env, event, kRecycle);
    const jthrowable original = reinterpret_cast<jthrowable>(
        vm.NewObject("java/lang/IllegalStateException"));
    vm.pending = true;
    vm.exception = original;
    assert(!recycler.Recycle() && vm.pending && vm.exception == original);
    assert(!recycler.Recycle() && vm.recycle_calls == 2);
    assert(vm.recycle_calls == 2);
  }
  assert(vm.recycle_calls == 2);
}

void TestPendingConstructorAndNoLookup() {
  FakeVm vm;
  JNIEnv env = MakeEnv(&vm);
  jobject event = vm.NewObject("android/view/MotionEvent");
  const jthrowable original = reinterpret_cast<jthrowable>(
      vm.NewObject("java/lang/RuntimeException"));
  vm.pending = true;
  vm.exception = original;
  {
    darwin_art_graphics_fixture::FixtureMotionEventRecycler recycler(
        &env, event, kRecycle);
    assert(!recycler.valid() && vm.recycle_calls == 1 && vm.pending &&
           vm.exception == original && vm.new_local_calls == 0);
  }
  assert(vm.recycle_calls == 1);
}

}  // namespace

int main() {
  TestExplicitAndDestructorFallback();
  TestEarlyAndPromotionFailure();
  TestCleanupExceptions();
  TestPendingConstructorAndNoLookup();
  return 0;
}
