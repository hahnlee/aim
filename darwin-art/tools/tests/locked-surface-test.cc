#include "window/locked_surface.h"
#include <cassert>
#include <cstdio>
#include <cstring>

namespace {
bool pending, missing, read_failure;
int depth, entries, exits;
jlong native_value;
jclass Class(JNIEnv*, jobject) { return reinterpret_cast<jclass>(1); }
jfieldID Field(JNIEnv*, jclass, const char* name, const char*) {
  if (missing) { pending = true; return nullptr; }
  return reinterpret_cast<jfieldID>(std::strcmp(name, "mLock") == 0 ? 2 : 3);
}
jobject Object(JNIEnv*, jobject, jfieldID) { return reinterpret_cast<jobject>(4); }
jlong Value(JNIEnv*, jobject, jfieldID) {
  assert(depth == 1);
  if (read_failure) pending = true;
  return native_value;
}
jint Enter(JNIEnv*, jobject) { ++entries; ++depth; return JNI_OK; }
jint Exit(JNIEnv*, jobject) { ++exits; --depth; return JNI_OK; }
jboolean Exception(JNIEnv*) { return pending; }
void Delete(JNIEnv*, jobject) {}
}
int main() {
  JNINativeInterface functions{};
  functions.GetObjectClass = Class; functions.GetFieldID = Field;
  functions.GetObjectField = Object; functions.GetLongField = Value;
  functions.MonitorEnter = Enter; functions.MonitorExit = Exit;
  functions.ExceptionCheck = Exception; functions.DeleteLocalRef = Delete;
  JNIEnv env{&functions};
  auto object = reinterpret_cast<jobject>(5);
  { darwin_art::window::LockedSurface lock(nullptr, object); assert(!lock.identity()); }
  { darwin_art::window::LockedSurface lock(&env, nullptr); assert(!lock.identity()); }
  native_value = 123;
  { darwin_art::window::LockedSurface lock(&env, object); assert(lock.identity() == 123 && depth == 1); }
  assert(depth == 0 && entries == 1 && exits == 1);
  native_value = 0;
  { darwin_art::window::LockedSurface lock(&env, object); assert(!lock.identity() && depth == 1); }
  assert(depth == 0 && entries == exits);
  missing = true;
  { darwin_art::window::LockedSurface lock(&env, object); assert(!lock.identity() && pending); }
  assert(pending && depth == 0);
  missing = false; pending = false; read_failure = true; native_value = 123;
  { darwin_art::window::LockedSurface lock(&env, object); assert(!lock.identity() && pending && depth == 1); }
  assert(pending && depth == 0 && entries == exits);
  const int before = entries;
  { darwin_art::window::LockedSurface lock(&env, object); assert(!lock.identity()); }
  assert(pending && entries == before);
  std::puts("Surface identity: lock held through retain scope, released/null rejected, exceptions preserved PASS");
}
