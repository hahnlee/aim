#include "activity_launch_transaction.h"

#include "client_transaction.h"
#include "../display/configuration.h"

namespace darwin_art::framework::wm {
namespace {
jobject Construct(JNIEnv* env, const char* name, const char* signature,
                  const jvalue* arguments = nullptr) {
  jclass type = env->FindClass(name);
  jmethodID constructor = type == nullptr
                              ? nullptr
                              : env->GetMethodID(type, "<init>", signature);
  return constructor == nullptr || env->ExceptionCheck() ? nullptr
                                : env->NewObjectA(type, constructor, arguments);
}

bool ScheduleResolvedWithinFrame(JNIEnv* env, jobject application_binder,
                                 jobject previous_activity_token,
                                 jobject activity_token, jobject intent,
                                 jobject info, bool register_token) {
  jclass stub = env->FindClass("android/app/IApplicationThread$Stub");
  jmethodID as_interface = stub == nullptr
                               ? nullptr
                               : env->GetStaticMethodID(
                                     stub, "asInterface",
                                     "(Landroid/os/IBinder;)"
                                     "Landroid/app/IApplicationThread;");
  jobject application_thread = as_interface == nullptr
                                   ? nullptr
                                   : env->CallStaticObjectMethod(
                                         stub, as_interface, application_binder);

  jclass resources = env->FindClass("android/content/res/Resources");
  jmethodID get_system = resources == nullptr
                             ? nullptr
                             : env->GetStaticMethodID(
                                   resources, "getSystem",
                                   "()Landroid/content/res/Resources;");
  jobject system_resources = get_system == nullptr
                                 ? nullptr
                                 : env->CallStaticObjectMethod(resources, get_system);
  jmethodID get_configuration = resources == nullptr
                                    ? nullptr
                                    : env->GetMethodID(
                                          resources, "getConfiguration",
                                          "()Landroid/content/res/Configuration;");
  jobject base_configuration = get_configuration == nullptr
                                   ? nullptr
                                   : env->CallObjectMethod(system_resources,
                                                           get_configuration);
  jobject current = display::ConfigurationForBuiltInDisplay(env, base_configuration);
  jobject override = Construct(env, "android/content/res/Configuration", "()V");
  if (activity_token == nullptr) {
    activity_token = Construct(env, "android/os/Binder", "()V");
  }
  jobject assist_token = Construct(env, "android/os/Binder", "()V");
  jobject shareable_token = Construct(env, "android/os/Binder", "()V");
  jobject caller_info_token = Construct(env, "android/os/Binder", "()V");
  jobject window_info = Construct(env, "android/window/ActivityWindowInfo", "()V");
  if (info == nullptr || application_thread == nullptr || intent == nullptr ||
      current == nullptr || override == nullptr || activity_token == nullptr ||
      assist_token == nullptr || shareable_token == nullptr ||
      caller_info_token == nullptr || window_info == nullptr ||
      env->ExceptionCheck()) {
    return false;
  }

  jclass controller =
      env->FindClass("dev/darwinart/runtime/wm/ActivityClientControllerEndpoint");
  jmethodID register_token_method =
      controller == nullptr
          ? nullptr
          : env->GetStaticMethodID(controller, "registerActivityToken",
                                   "(Landroid/os/IBinder;Landroid/os/IBinder;)V");
  if (register_token_method == nullptr || env->ExceptionCheck()) return false;
  if (register_token) {
    env->CallStaticVoidMethod(controller, register_token_method,
                              application_binder, activity_token);
    if (env->ExceptionCheck()) return false;
  }

  static constexpr const char* kLaunchSignature =
      "(Landroid/os/IBinder;Landroid/content/Intent;I"
      "Landroid/content/pm/ActivityInfo;Landroid/content/res/Configuration;"
      "Landroid/content/res/Configuration;ILjava/lang/String;"
      "Lcom/android/internal/app/IVoiceInteractor;ILandroid/os/Bundle;"
      "Landroid/os/PersistableBundle;Ljava/util/List;Ljava/util/List;"
      "Landroid/app/ActivityOptions$SceneTransitionInfo;Z"
      "Landroid/app/ProfilerInfo;Landroid/os/IBinder;"
      "Landroid/app/IActivityClientController;Landroid/os/IBinder;Z"
      "Landroid/os/IBinder;Landroid/os/IBinder;"
      "Landroid/window/ActivityWindowInfo;)V";
  jvalue launch_args[24]{};
  launch_args[0].l = activity_token;
  launch_args[1].l = intent;
  launch_args[3].l = info;
  launch_args[4].l = current;
  launch_args[5].l = override;
  launch_args[9].i = -1;  // ActivityManager.PROCESS_STATE_UNKNOWN.
  launch_args[17].l = assist_token;
  launch_args[19].l = shareable_token;
  launch_args[22].l = caller_info_token;
  launch_args[23].l = window_info;
  jobject launch = Construct(env, "android/app/servertransaction/LaunchActivityItem",
                             kLaunchSignature, launch_args);
  jvalue resume_args[3]{};
  resume_args[0].l = activity_token;
  jobject resume = Construct(env, "android/app/servertransaction/ResumeActivityItem",
                             "(Landroid/os/IBinder;ZZ)V", resume_args);
  jvalue pause_args[5]{};
  pause_args[0].l = previous_activity_token;
  pause_args[2].z = JNI_TRUE;  // userLeaving
  jobject pause = previous_activity_token == nullptr
                      ? nullptr
                      : Construct(env,
                                  "android/app/servertransaction/PauseActivityItem",
                                  "(Landroid/os/IBinder;ZZZZ)V", pause_args);
  jclass item_type = env->FindClass(
      "android/app/servertransaction/ClientTransactionItem");
  const jsize item_count = previous_activity_token == nullptr ? 2 : 3;
  jobjectArray items = item_type == nullptr
                           ? nullptr
                           : env->NewObjectArray(item_count, item_type, nullptr);
  if (launch != nullptr && resume != nullptr && items != nullptr &&
      (previous_activity_token == nullptr || pause != nullptr) &&
      !env->ExceptionCheck()) {
    jsize offset = 0;
    if (pause != nullptr) env->SetObjectArrayElement(items, offset++, pause);
    env->SetObjectArrayElement(items, offset++, launch);
    env->SetObjectArrayElement(items, offset, resume);
  }
  return items != nullptr && !env->ExceptionCheck() &&
         ScheduleClientTransaction(env, application_thread, items);
}

jboolean ScheduleResolved(JNIEnv* env, jclass, jobject application,
                          jobject previous_activity_token,
                          jobject activity_token, jobject intent, jobject info) {
  return ScheduleResolvedActivityLaunch(env, application, previous_activity_token,
                                        activity_token, intent, info)
             ? JNI_TRUE
             : JNI_FALSE;
}

jboolean ScheduleFinish(JNIEnv* env, jclass, jobject application_binder,
                        jobject activity_token,
                        jobject previous_activity_token) {
  if (env == nullptr || application_binder == nullptr || activity_token == nullptr ||
      env->ExceptionCheck() || env->PushLocalFrame(32) < 0) {
    return JNI_FALSE;
  }
  jclass stub = env->FindClass("android/app/IApplicationThread$Stub");
  jmethodID as_interface =
      stub == nullptr
          ? nullptr
          : env->GetStaticMethodID(stub, "asInterface",
                                   "(Landroid/os/IBinder;)"
                                   "Landroid/app/IApplicationThread;");
  jobject application_thread =
      as_interface == nullptr
          ? nullptr
          : env->CallStaticObjectMethod(stub, as_interface, application_binder);
  jvalue pause_args[5]{};
  pause_args[0].l = activity_token;
  pause_args[1].z = JNI_TRUE;  // finished
  jobject pause = Construct(env, "android/app/servertransaction/PauseActivityItem",
                            "(Landroid/os/IBinder;ZZZZ)V", pause_args);
  jvalue destroy_args[2]{};
  destroy_args[0].l = activity_token;
  destroy_args[1].z = JNI_TRUE;
  jobject destroy = Construct(
      env, "android/app/servertransaction/DestroyActivityItem",
      "(Landroid/os/IBinder;Z)V", destroy_args);
  jvalue resume_args[3]{};
  resume_args[0].l = previous_activity_token;
  jobject resume = previous_activity_token == nullptr
                       ? nullptr
                       : Construct(env,
                                   "android/app/servertransaction/ResumeActivityItem",
                                   "(Landroid/os/IBinder;ZZ)V", resume_args);
  jclass item_type =
      env->FindClass("android/app/servertransaction/ClientTransactionItem");
  const jsize item_count = previous_activity_token == nullptr ? 2 : 3;
  jobjectArray items = item_type == nullptr
                           ? nullptr
                           : env->NewObjectArray(item_count, item_type, nullptr);
  if (application_thread != nullptr && pause != nullptr && destroy != nullptr &&
      items != nullptr && (previous_activity_token == nullptr || resume != nullptr) &&
      !env->ExceptionCheck()) {
    env->SetObjectArrayElement(items, 0, pause);
    env->SetObjectArrayElement(items, 1, destroy);
    if (resume != nullptr) env->SetObjectArrayElement(items, 2, resume);
  }
  const bool scheduled = items != nullptr && !env->ExceptionCheck() &&
                         ScheduleClientTransaction(env, application_thread, items);
  env->PopLocalFrame(nullptr);
  return scheduled ? JNI_TRUE : JNI_FALSE;
}
}  // namespace

bool ScheduleActivityLaunch(JNIEnv* env, jobject application_binder,
                            jstring package_name, jstring installed_record) {
  if (env == nullptr || application_binder == nullptr || package_name == nullptr ||
      installed_record == nullptr || env->ExceptionCheck() ||
      env->PushLocalFrame(64) < 0) {
    return false;
  }
  auto finish = [&](bool result) {
    env->PopLocalFrame(nullptr);
    return result;
  };

  jclass mapper = env->FindClass("dev/darwinart/runtime/pm/InstalledActivityInfo");
  jmethodID map = mapper == nullptr
                      ? nullptr
                      : env->GetStaticMethodID(
                            mapper, "launchActivity",
                            "(Ljava/lang/String;Ljava/lang/String;)"
                            "Landroid/content/pm/ActivityInfo;");
  jobject info = map == nullptr ? nullptr : env->CallStaticObjectMethod(
                                                mapper, map, package_name,
                                                installed_record);
  jclass info_type = info == nullptr ? nullptr : env->GetObjectClass(info);
  jfieldID name_field = info_type == nullptr
                            ? nullptr
                            : env->GetFieldID(info_type, "name", "Ljava/lang/String;");
  auto activity_name = name_field == nullptr
                           ? nullptr
                           : static_cast<jstring>(env->GetObjectField(info, name_field));

  jvalue intent_args[1]{};
  intent_args[0].l = env->NewStringUTF("android.intent.action.MAIN");
  jobject intent = Construct(env, "android/content/Intent", "(Ljava/lang/String;)V",
                             intent_args);
  jclass intent_type = intent == nullptr ? nullptr : env->GetObjectClass(intent);
  jmethodID set_class = intent_type == nullptr
                            ? nullptr
                            : env->GetMethodID(
                                  intent_type, "setClassName",
                                  "(Ljava/lang/String;Ljava/lang/String;)"
                                  "Landroid/content/Intent;");
  jmethodID add_category = intent_type == nullptr
                               ? nullptr
                               : env->GetMethodID(
                                     intent_type, "addCategory",
                                     "(Ljava/lang/String;)Landroid/content/Intent;");
  jmethodID add_flags = intent_type == nullptr
                            ? nullptr
                            : env->GetMethodID(
                                  intent_type, "addFlags",
                                  "(I)Landroid/content/Intent;");
  jstring launcher = env->NewStringUTF("android.intent.category.LAUNCHER");
  if (set_class != nullptr)
    env->CallObjectMethod(intent, set_class, package_name, activity_name);
  if (add_category != nullptr) env->CallObjectMethod(intent, add_category, launcher);
  if (add_flags != nullptr) env->CallObjectMethod(intent, add_flags, 0x10000000);

  if (info == nullptr || activity_name == nullptr || intent == nullptr ||
      env->ExceptionCheck()) {
    return finish(false);
  }
  return finish(ScheduleResolvedWithinFrame(env, application_binder, nullptr,
                                            nullptr, intent, info, true));
}

bool ScheduleResolvedActivityLaunch(JNIEnv* env, jobject application_binder,
                                    jobject previous_activity_token,
                                    jobject activity_token, jobject intent,
                                    jobject activity_info) {
  if (env == nullptr || application_binder == nullptr || intent == nullptr ||
      activity_info == nullptr || env->ExceptionCheck() ||
      env->PushLocalFrame(64) < 0) {
    return false;
  }
  const bool result =
      ScheduleResolvedWithinFrame(env, application_binder, previous_activity_token,
                                  activity_token, intent, activity_info, false);
  env->PopLocalFrame(nullptr);
  return result;
}

bool RegisterActivityLaunchScheduler(JNIEnv* env, jclass endpoint) {
  if (env == nullptr || endpoint == nullptr || env->ExceptionCheck()) return false;
  JNINativeMethod methods[] = {{
      const_cast<char*>("nativeScheduleActivity"),
      const_cast<char*>("(Landroid/os/IBinder;Landroid/os/IBinder;"
                        "Landroid/os/IBinder;Landroid/content/Intent;"
                        "Landroid/content/pm/ActivityInfo;)Z"),
      reinterpret_cast<void*>(&ScheduleResolved),
  }, {
      const_cast<char*>("nativeScheduleFinishActivity"),
      const_cast<char*>("(Landroid/os/IBinder;Landroid/os/IBinder;"
                        "Landroid/os/IBinder;)Z"),
      reinterpret_cast<void*>(&ScheduleFinish),
  }};
  return env->RegisterNatives(endpoint, methods, 2) == JNI_OK;
}

}  // namespace darwin_art::framework::wm
