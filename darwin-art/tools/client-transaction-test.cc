// Boundary test with JNI doubles, not ActivityThread or Binder integration.
#include "../runtime/framework/wm/client_transaction.h"
#include <cassert>
#include <cstdarg>
#include <cstring>
#include <cstdio>
#include <vector>

namespace {
_jobject endpoint, transaction, first, second;
_jobjectArray array;
_jthrowable remote_failure, invalid_failure;
_jclass endpoint_type, item_type, transaction_type, exception_type;
jthrowable pending = nullptr;
bool fail_schedule = false;
bool fail_lookup = false;
bool fail_constructor_lookup = false;
int pushes = 0, pops = 0, schedules = 0;
std::vector<jobject> items, appended;
jmethodID ctor = reinterpret_cast<jmethodID>(1);
jmethodID add = reinterpret_cast<jmethodID>(2);
jmethodID schedule = reinterpret_cast<jmethodID>(3);
jboolean ExceptionCheck(JNIEnv*) { return pending != nullptr; }
jint PushLocalFrame(JNIEnv*, jint) { ++pushes; return JNI_OK; }
jobject PopLocalFrame(JNIEnv*, jobject object) { ++pops; return object; }
jclass FindClass(JNIEnv*, const char* name) {
  assert(!pending);
  if (fail_lookup) { pending = &remote_failure; return nullptr; }
  if (!std::strcmp(name, "android/app/IApplicationThread")) return &endpoint_type;
  if (!std::strcmp(name, "android/app/servertransaction/ClientTransactionItem")) return &item_type;
  if (!std::strcmp(name, "android/app/servertransaction/ClientTransaction")) return &transaction_type;
  assert(!std::strcmp(name, "java/lang/IllegalArgumentException"));
  return &exception_type;
}
jboolean IsInstanceOf(JNIEnv*, jobject value, jclass type) {
  return (type == &endpoint_type && value == &endpoint) ||
      (type == &item_type && (value == &first || value == &second));
}
jmethodID GetMethodID(JNIEnv*, jclass, const char* name, const char* signature) {
  assert(!pending);
  if (fail_constructor_lookup) { pending = &remote_failure; return nullptr; }
  if (!std::strcmp(name, "<init>")) {
    assert(!std::strcmp(signature, "(Landroid/app/IApplicationThread;)V")); return ctor;
  }
  if (!std::strcmp(name, "addTransactionItem")) {
    assert(!std::strcmp(signature, "(Landroid/app/servertransaction/ClientTransactionItem;)V"));
    return add;
  }
  assert(!std::strcmp(name, "schedule"));
  assert(!std::strcmp(signature, "()Landroid/os/RemoteException;")); return schedule;
}
jobject NewObjectV(JNIEnv*, jclass, jmethodID method, va_list args) {
  assert(method == ctor && va_arg(args, jobject) == &endpoint); return &transaction;
}
void CallVoidMethodV(JNIEnv*, jobject object, jmethodID method, va_list args) {
  assert(object == &transaction && method == add);
  appended.push_back(va_arg(args, jobject));
}
jobject CallObjectMethodV(JNIEnv*, jobject object, jmethodID method, va_list) {
  assert(object == &transaction && method == schedule); ++schedules;
  return fail_schedule ? &remote_failure : nullptr;
}
jsize GetArrayLength(JNIEnv*, jarray) { return items.size(); }
jobject GetObjectArrayElement(JNIEnv*, jobjectArray, jsize index) { return items.at(index); }
void DeleteLocalRef(JNIEnv*, jobject) {}
jint Throw(JNIEnv*, jthrowable failure) { pending = failure; return JNI_OK; }
jint ThrowNew(JNIEnv*, jclass, const char*) { pending = &invalid_failure; return JNI_OK; }
void Reset() {
  pending = nullptr; fail_schedule = false; pushes = pops = schedules = 0;
  fail_lookup = fail_constructor_lookup = false;
  items = {&first, &second}; appended.clear();
}
}
int main() {
  JNINativeInterface functions{};
  functions.ExceptionCheck = ExceptionCheck;
  functions.PushLocalFrame = PushLocalFrame; functions.PopLocalFrame = PopLocalFrame;
  functions.FindClass = FindClass; functions.IsInstanceOf = IsInstanceOf;
  functions.GetMethodID = GetMethodID; functions.NewObjectV = NewObjectV;
  functions.CallVoidMethodV = CallVoidMethodV; functions.CallObjectMethodV = CallObjectMethodV;
  functions.GetArrayLength = GetArrayLength; functions.GetObjectArrayElement = GetObjectArrayElement;
  functions.DeleteLocalRef = DeleteLocalRef; functions.Throw = Throw; functions.ThrowNew = ThrowNew;
  JNIEnv env{&functions};
  using darwin_art::framework::wm::ScheduleClientTransaction;
  Reset();
  assert(ScheduleClientTransaction(&env, &endpoint, &array));
  assert(appended == items && schedules == 1 && pushes == 1 && pops == 1);
  Reset(); items[1] = nullptr;
  assert(!ScheduleClientTransaction(&env, &endpoint, &array));
  assert(pending && schedules == 0 && pushes == pops);
  Reset(); items.clear();
  assert(!ScheduleClientTransaction(&env, &endpoint, &array));
  assert(pending && schedules == 0 && pushes == pops);
  Reset(); fail_schedule = true;
  assert(!ScheduleClientTransaction(&env, &endpoint, &array));
  assert(pending == &remote_failure && schedules == 1 && pushes == pops);
  Reset(); pending = &remote_failure;
  assert(!ScheduleClientTransaction(&env, &endpoint, &array));
  assert(pending == &remote_failure && pushes == 0 && schedules == 0);
  Reset(); fail_lookup = true;
  assert(!ScheduleClientTransaction(&env, &endpoint, &array));
  assert(pending == &remote_failure && pushes == pops && schedules == 0);
  Reset(); fail_constructor_lookup = true;
  assert(!ScheduleClientTransaction(&env, &endpoint, &array));
  assert(pending == &remote_failure && pushes == pops && schedules == 0);
  Reset();
  assert(!ScheduleClientTransaction(&env, nullptr, &array));
  assert(pending && pushes == 0 && schedules == 0);
  std::puts("ClientTransaction boundary: PASS (ordering, validation, remote failure, local frames)");
}
