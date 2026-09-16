#include "java_exception_report.h"

#include <iostream>

namespace darwin_art::framework::app {
namespace {

void PrintJavaString(JNIEnv* env, jstring value, const char* prefix) {
  if (value == nullptr || env->ExceptionCheck()) return;
  const char* text = env->GetStringUTFChars(value, nullptr);
  if (text == nullptr) return;
  std::cerr << prefix << text << '\n';
  env->ReleaseStringUTFChars(value, text);
}

void PrintThrowable(JNIEnv* env, jthrowable failure, jmethodID to_string,
                    jmethodID get_stack, jmethodID element_to_string,
                    const char* description_prefix) {
  jstring description = static_cast<jstring>(
      env->CallObjectMethod(failure, to_string));
  if (!env->ExceptionCheck()) {
    PrintJavaString(env, description, description_prefix);
  }
  env->DeleteLocalRef(description);
  jobjectArray stack =
      env->ExceptionCheck()
          ? nullptr
          : static_cast<jobjectArray>(
                env->CallObjectMethod(failure, get_stack));
  if (!env->ExceptionCheck() && stack != nullptr) {
    const jsize count = env->GetArrayLength(stack);
    for (jsize index = 0; index < count && !env->ExceptionCheck(); ++index) {
      jobject frame = env->GetObjectArrayElement(stack, index);
      jstring text =
          frame == nullptr
              ? nullptr
              : static_cast<jstring>(
                    env->CallObjectMethod(frame, element_to_string));
      if (!env->ExceptionCheck()) PrintJavaString(env, text, "  at ");
      env->DeleteLocalRef(frame);
      env->DeleteLocalRef(text);
    }
  }
  env->DeleteLocalRef(stack);
}

}

void ReportPendingJavaException(JNIEnv* env) {
  if (env == nullptr) return;
  jthrowable failure = env->ExceptionOccurred();
  if (failure == nullptr) {
    std::cerr << "no pending Java exception\n";
    return;
  }
  env->ExceptionClear();
  if (env->PushLocalFrame(16) < 0) {
    env->ExceptionClear();
    env->Throw(failure);
    return;
  }
  jclass throwable = env->FindClass("java/lang/Throwable");
  jmethodID to_string =
      throwable == nullptr ? nullptr : env->GetMethodID(
          throwable, "toString", "()Ljava/lang/String;");
  jmethodID get_stack =
      throwable == nullptr ? nullptr : env->GetMethodID(
          throwable, "getStackTrace", "()[Ljava/lang/StackTraceElement;");
  jmethodID get_cause =
      throwable == nullptr ? nullptr : env->GetMethodID(
          throwable, "getCause", "()Ljava/lang/Throwable;");
  jclass element = env->FindClass("java/lang/StackTraceElement");
  jmethodID element_to_string =
      element == nullptr ? nullptr : env->GetMethodID(
          element, "toString", "()Ljava/lang/String;");
  if (to_string != nullptr && get_stack != nullptr &&
      element_to_string != nullptr && !env->ExceptionCheck()) {
    PrintThrowable(env, failure, to_string, get_stack, element_to_string, "");
    jthrowable current = failure;
    for (int depth = 0; depth < 8 && get_cause != nullptr &&
                        !env->ExceptionCheck();
         ++depth) {
      jthrowable cause = static_cast<jthrowable>(
          env->CallObjectMethod(current, get_cause));
      if (cause == nullptr || env->IsSameObject(cause, current)) {
        env->DeleteLocalRef(cause);
        break;
      }
      PrintThrowable(env, cause, to_string, get_stack, element_to_string,
                     "Caused by: ");
      if (current != failure) env->DeleteLocalRef(current);
      current = cause;
    }
    if (current != failure) env->DeleteLocalRef(current);
  }
  if (env->ExceptionCheck()) env->ExceptionClear();
  env->PopLocalFrame(nullptr);
  env->Throw(failure);
}

}
