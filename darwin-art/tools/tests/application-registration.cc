// Contract test for the app-entry boundary, not an ART/APK acceptance test.
#include "runtime/framework/app/process_registration.h"
#include <cassert>
#include <cstring>
#include <iostream>

namespace darwin_art::framework::os {
bool RegisterSystemServiceClientTransport(JNIEnv*) { return true; }
}

namespace darwin_art::framework::wm {
bool RegisterDesktopWindowMetadataClient(JNIEnv*) { return true; }
}

namespace {
bool pending;
bool fail_class;
bool fail_lookup;
bool fail_call;
int finds;
int calls;
int deletes;
int token;
jclass Class() { return reinterpret_cast<jclass>(&token); }
jmethodID Method() { return reinterpret_cast<jmethodID>(&token); }
jboolean ExceptionCheck(JNIEnv*) { return pending ? JNI_TRUE : JNI_FALSE; }
jclass FindClass(JNIEnv*, const char* name) {
  ++finds;
  // No lookup or replacement of Runtime.nativeLoad is permitted here.
  assert(std::strcmp(name, "android/os/Looper") == 0);
  if (fail_class) { pending = true; return nullptr; }
  return Class();
}
jmethodID GetStaticMethodID(JNIEnv*, jclass type, const char* name, const char* signature) {
  assert(type == Class());
  assert(std::strcmp(name, "prepareMainLooper") == 0);
  assert(std::strcmp(signature, "()V") == 0);
  if (fail_lookup) { pending = true; return nullptr; }
  return Method();
}
void CallStaticVoidMethodV(JNIEnv*, jclass type, jmethodID method, va_list) {
  assert(type == Class() && method == Method());
  ++calls;
  if (fail_call) pending = true;
}
void DeleteLocalRef(JNIEnv*, jobject object) {
  assert(object == nullptr || object == Class());
  ++deletes;
}
void Reset() {
  pending = fail_class = fail_lookup = fail_call = false;
  finds = calls = deletes = 0;
}
}

int main() {
  JNINativeInterface table{};
  table.ExceptionCheck = ExceptionCheck;
  table.FindClass = FindClass;
  table.GetStaticMethodID = GetStaticMethodID;
  table.CallStaticVoidMethodV = CallStaticVoidMethodV;
  table.DeleteLocalRef = DeleteLocalRef;
  // RegisterNatives intentionally absent: this boundary must never invoke it.
  JNIEnv env{&table};
  using darwin_art::framework::app::FinishFrameworkRegistration;
  Reset();
  assert(FinishFrameworkRegistration(nullptr, true) == 4);
  pending = true;
  assert(FinishFrameworkRegistration(&env, true) == 4 && finds == 0);
  Reset();
  assert(FinishFrameworkRegistration(&env, false) == 0);
  assert(finds == 0 && calls == 0 && deletes == 0);
  Reset();
  assert(FinishFrameworkRegistration(&env, true) == 0);
  assert(finds == 1 && calls == 1 && deletes == 1);
  Reset();
  fail_class = true;
  assert(FinishFrameworkRegistration(&env, true) == 25 && pending && calls == 0);
  assert(deletes == 1);
  Reset();
  fail_lookup = true;
  assert(FinishFrameworkRegistration(&env, true) == 25 && pending && calls == 0);
  assert(deletes == 1);
  Reset();
  fail_call = true;
  assert(FinishFrameworkRegistration(&env, true) == 25 && pending && calls == 1);
  assert(deletes == 1);
  std::cout << "application registration: boot JNI owner preserved, Looper errors retained PASS\n";
}
