#include "main_loop.h"

#include <cstdlib>
#include <string>

namespace darwin_art::framework::app {
namespace {
void Error(JNIEnv* env, const char* message) {
  if (env->ExceptionCheck()) return;
  jclass type = env->FindClass("java/lang/IllegalStateException");
  if (type != nullptr) env->ThrowNew(type, message);
  env->DeleteLocalRef(type);
}
}
ApplicationMainLoopExit RunPreparedApplicationMainLoop(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck() || env->PushLocalFrame(16) < 0) {
    return ApplicationMainLoopExit::kInvalidEnvironment;
  }
  auto run = [&]() -> ApplicationMainLoopExit {
    jclass activity_thread = env->FindClass("android/app/ActivityThread");
    if (activity_thread == nullptr) return ApplicationMainLoopExit::kActivityThreadLookup;
    jmethodID current = env->GetStaticMethodID(activity_thread, "currentActivityThread",
        "()Landroid/app/ActivityThread;");
    if (current == nullptr) return ApplicationMainLoopExit::kActivityThreadLookup;
    jobject existing = env->CallStaticObjectMethod(activity_thread, current);
    if (env->ExceptionCheck()) return ApplicationMainLoopExit::kActivityThreadLookup;
    if (existing != nullptr) {
      Error(env, "Application process is already attached");
      return ApplicationMainLoopExit::kAlreadyAttached;
    }
    jclass string_class = env->FindClass("java/lang/String");
    if (string_class == nullptr) return ApplicationMainLoopExit::kActivityThreadLookup;
    const char* raw_start_sequence =
        std::getenv("DARWIN_ART_PROCESS_START_SEQUENCE");
    const jsize argument_count =
        raw_start_sequence == nullptr || *raw_start_sequence == '\0' ? 0 : 1;
    jobjectArray args = env->NewObjectArray(argument_count, string_class, nullptr);
    if (args == nullptr) return ApplicationMainLoopExit::kActivityThreadLookup;
    if (argument_count == 1) {
      const std::string argument = std::string("seq=") + raw_start_sequence;
      jstring value = env->NewStringUTF(argument.c_str());
      if (value == nullptr) return ApplicationMainLoopExit::kActivityThreadLookup;
      env->SetObjectArrayElement(args, 0, value);
      env->DeleteLocalRef(value);
      if (env->ExceptionCheck()) return ApplicationMainLoopExit::kActivityThreadLookup;
    }
    jmethodID main = env->GetStaticMethodID(
        activity_thread, "main", "([Ljava/lang/String;)V");
    if (main == nullptr) return ApplicationMainLoopExit::kActivityThreadLookup;
    env->CallStaticVoidMethod(activity_thread, main, args);
    Error(env, "ActivityThread.main() unexpectedly returned");
    return ApplicationMainLoopExit::kLooperDispatch;
  };
  const auto result = run();
  env->PopLocalFrame(nullptr);
  return result;
}
}
