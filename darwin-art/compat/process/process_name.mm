#include "process_name.h"
#import <Foundation/Foundation.h>
#include <pthread.h>
#include <string>

namespace darwin_art::process {
namespace {
void Throw(JNIEnv* env, const char* type, const char* message) {
  jclass exception = env->FindClass(type);
  if (exception != nullptr) env->ThrowNew(exception, message);
}
}

void SetArgV0(JNIEnv* env, jclass, jstring name) {
  if (name == nullptr) {
    Throw(env, "java/lang/NullPointerException", "process name");
    return;
  }
  const jsize length = env->GetStringLength(name);
  if (length == 0) return;  // AndroidRuntime also leaves an empty name unchanged.
  const jchar* characters = env->GetStringChars(name, nullptr);
  if (characters == nullptr) return;
  @autoreleasepool {
    NSString* value = [[NSString alloc] initWithCharacters:characters length:length];
    env->ReleaseStringChars(name, characters);
    @try {
      // Darwin has no Linux argv/prctl process-title API. Keep the full name
      // in Foundation and use the OS-supported current-thread name for tools.
      // Process.setArgV0 owns Android's full sArgV0 value in the framework.
      const char* encoded = value.UTF8String;
      if (encoded == nullptr) {
        Throw(env, "java/lang/IllegalArgumentException", "invalid process name");
      } else {
        std::string thread_name(encoded);
        if (thread_name.size() > 63) {
          size_t end = 63;
          while (end > 0 && (static_cast<unsigned char>(thread_name[end]) & 0xc0) == 0x80) --end;
          thread_name.resize(end);
        }
        const int error = pthread_setname_np(thread_name.c_str());
        if (error != 0) {
          Throw(env, "java/lang/IllegalStateException", "pthread_setname_np failed");
        } else {
          [NSProcessInfo processInfo].processName = value;
        }
      }
    } @catch (NSException* exception) {
      Throw(env, "java/lang/IllegalStateException", exception.reason.UTF8String);
    }
    [value release];
  }
}
}
