#import <Foundation/Foundation.h>
#include <cassert>
#include <cstring>
#include <pthread.h>
#include <string>
#include "process/process_name.h"

static bool threw = false;
static jsize Length(JNIEnv*, jstring value) {
  return reinterpret_cast<std::u16string*>(value)->size();
}
static const jchar* Chars(JNIEnv*, jstring value, jboolean*) {
  return reinterpret_cast<const jchar*>(reinterpret_cast<std::u16string*>(value)->data());
}
static void Release(JNIEnv*, jstring, const jchar*) {}
static jclass Find(JNIEnv*, const char*) { return reinterpret_cast<jclass>(1); }
static jint Throw(JNIEnv*, jclass, const char*) { threw = true; return 0; }

int main() {
  JNINativeInterface_ table{};
  table.GetStringLength = Length;
  table.GetStringChars = Chars;
  table.ReleaseStringChars = Release;
  table.FindClass = Find;
  table.ThrowNew = Throw;
  JNIEnv env{&table};
  @autoreleasepool {
    std::u16string value = u"org.example.app:renderer";
    darwin_art::process::SetArgV0(&env, nullptr, reinterpret_cast<jstring>(&value));
    assert(!threw);
    assert([[NSProcessInfo processInfo].processName isEqualToString:@"org.example.app:renderer"]);
    char name[64]{};
    assert(pthread_getname_np(pthread_self(), name, sizeof(name)) == 0);
    assert(std::strcmp(name, "org.example.app:renderer") == 0);
    value.assign(40, u'한');
    darwin_art::process::SetArgV0(&env, nullptr, reinterpret_cast<jstring>(&value));
    assert(!threw);
    assert(pthread_getname_np(pthread_self(), name, sizeof(name)) == 0);
    assert(std::strlen(name) == 63);
    assert([NSString stringWithUTF8String:name] != nil);
    assert([NSProcessInfo processInfo].processName.length == 40);
    value.clear();
    darwin_art::process::SetArgV0(&env, nullptr, reinterpret_cast<jstring>(&value));
    assert([NSProcessInfo processInfo].processName.length == 40);
    darwin_art::process::SetArgV0(&env, nullptr, nullptr);
    assert(threw);
  }
  puts("process-name: Foundation/full, pthread/UTF8-bound, empty and null PASS");
}
