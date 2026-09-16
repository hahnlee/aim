#include "client_transaction.h"

namespace darwin_art::framework::wm {
namespace {
void Invalid(JNIEnv* env, const char* message) {
  if (env->ExceptionCheck()) return;
  jclass type = env->FindClass("java/lang/IllegalArgumentException");
  if (type != nullptr) env->ThrowNew(type, message);
  env->DeleteLocalRef(type);
}
}

bool ScheduleClientTransaction(JNIEnv* env, jobject application_thread,
                               jobjectArray items) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  if (application_thread == nullptr || items == nullptr) {
    Invalid(env, "Application thread and transaction items are required");
    return false;
  }
  if (env->PushLocalFrame(16) < 0) return false;
  auto finish = [&](bool result) { env->PopLocalFrame(nullptr); return result; };
  jclass endpoint = env->FindClass("android/app/IApplicationThread");
  if (endpoint == nullptr) return finish(false);
  jclass item_type = env->FindClass("android/app/servertransaction/ClientTransactionItem");
  if (endpoint == nullptr || item_type == nullptr || env->ExceptionCheck()) return finish(false);
  if (!env->IsInstanceOf(application_thread, endpoint)) {
    Invalid(env, "Expected IApplicationThread endpoint");
    return finish(false);
  }
  const jsize count = env->GetArrayLength(items);
  if (count == 0) {
    Invalid(env, "Empty client transaction");
    return finish(false);
  }
  jclass type = env->FindClass("android/app/servertransaction/ClientTransaction");
  if (type == nullptr) return finish(false);
  jmethodID constructor = env->GetMethodID(type, "<init>", "(Landroid/app/IApplicationThread;)V");
  if (constructor == nullptr) return finish(false);
  jmethodID add = env->GetMethodID(type, "addTransactionItem",
      "(Landroid/app/servertransaction/ClientTransactionItem;)V");
  if (add == nullptr) return finish(false);
  jmethodID schedule = env->GetMethodID(type, "schedule", "()Landroid/os/RemoteException;");
  if (constructor == nullptr || add == nullptr || schedule == nullptr || env->ExceptionCheck()) {
    return finish(false);
  }
  jobject transaction = env->NewObject(type, constructor, application_thread);
  if (transaction == nullptr || env->ExceptionCheck()) return finish(false);
  for (jsize index = 0; index < count; ++index) {
    jobject item = env->GetObjectArrayElement(items, index);
    if (env->ExceptionCheck()) return finish(false);
    if (item == nullptr || !env->IsInstanceOf(item, item_type)) {
      Invalid(env, "Expected non-null ClientTransactionItem");
      return finish(false);
    }
    env->CallVoidMethod(transaction, add, item);
    env->DeleteLocalRef(item);
    if (env->ExceptionCheck()) return finish(false);
  }
  // Original AOSP schedule calls IApplicationThread.scheduleTransaction;
  // ApplicationThread/TransactionExecutor retain preExecute and execution.
  auto failure = static_cast<jthrowable>(env->CallObjectMethod(transaction, schedule));
  if (!env->ExceptionCheck() && failure != nullptr) env->Throw(failure);
  return finish(!env->ExceptionCheck());
}
}
