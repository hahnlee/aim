#include "runtime_app_activity.h"
#include "../runtime/framework/app/process_attachment.h"
#include "../runtime/framework/app/application_binding.h"
#include "../runtime/framework/os/service_process_transport.h"

#include <cstdlib>
#include <cstring>
#include <iostream>
#include <string>
#include <sstream>

#include "mirror/throwable.h"
#include "darwin_binder_wire.h"
#include "darwin_surface_bridge.h"
#include "thread-current-inl.h"

namespace darwin_art_app_activity {
namespace {

std::string JavaString(JNIEnv* env, jstring value) {
  if (value == nullptr) return {};
  const char* utf = env->GetStringUTFChars(value, nullptr);
  if (utf == nullptr) return {};
  std::string result(utf);
  env->ReleaseStringUTFChars(value, utf);
  return result;
}

jstring ResolveInstalledPackage(JNIEnv* env, jclass, jstring package_name) {
  const std::string package = JavaString(env, package_name);
  const char* socket_path = std::getenv("DARWIN_ART_SYSTEM_SERVER_SOCKET");
  const std::string record = darwin_art::QuerySystemPackageRecord(
      env, socket_path, package.c_str());
  return record.empty() ? nullptr : env->NewStringUTF(record.c_str());
}

bool InstallPackageManagerNatives(JNIEnv* env, jobject package_manager) {
  jclass package_manager_class = env->GetObjectClass(package_manager);
  jclass record_endpoint = env->FindClass("dev/darwinart/runtime/pm/PackageRecords");
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeResolveInstalledPackage"),
       const_cast<char*>("(Ljava/lang/String;)Ljava/lang/String;"),
       reinterpret_cast<void*>(&ResolveInstalledPackage)},
  };
  const bool installed = package_manager_class != nullptr && record_endpoint != nullptr &&
                         env->RegisterNatives(record_endpoint, methods, 1) == JNI_OK;
  env->DeleteLocalRef(record_endpoint);
  const char* verify_package = std::getenv("DARWIN_ART_VERIFY_SYSTEM_PACKAGE");
  if (installed && verify_package != nullptr && *verify_package != '\0') {
    jmethodID get_package_info = env->GetMethodID(
        package_manager_class, "getPackageInfo",
        "(Ljava/lang/String;I)Landroid/content/pm/PackageInfo;");
    jstring requested = env->NewStringUTF(verify_package);
    jobject info = get_package_info == nullptr
                       ? nullptr
                       : env->CallObjectMethod(package_manager, get_package_info,
                                               requested, 0);
    env->DeleteLocalRef(requested);
    if (info == nullptr || env->ExceptionCheck()) {
      if (env->ExceptionCheck()) env->ExceptionClear();
      env->DeleteLocalRef(info);
      env->DeleteLocalRef(package_manager_class);
      return false;
    }
    std::cerr << "ART system_server-lite: PackageManager resolved "
              << verify_package << " over Binder\n";
    env->DeleteLocalRef(info);
  }
  env->DeleteLocalRef(package_manager_class);
  return installed;
}

}  // namespace

int prepare(JNIEnv* env, art::Thread* self, jobject* activity_instance_out,
            jclass probe_activity_class, jclass probe_context_class,
            const darwin_art_app_resources::Bundle* resources,
            jobject package_manager, bool run_apk_app,
            bool use_framework_resources, const char* apk_app_package,
            const char* apk_app_activity, bool application_only, Bundle* out) {
  if (env == nullptr || self == nullptr || activity_instance_out == nullptr ||
      probe_activity_class == nullptr || probe_context_class == nullptr ||
      resources == nullptr || package_manager == nullptr || out == nullptr) {
    return 27;
  }
  jobject activity_instance = *activity_instance_out;
  if (!run_apk_app && activity_instance == nullptr) {
    return 27;
  }
  if (!darwin_art::framework::os::RegisterServiceProcessTransport(
          env, probe_context_class) ||
      !InstallPackageManagerNatives(env, package_manager) ||
      env->ExceptionCheck()) {
    std::cerr << "ART Android process: host service JNI setup failed\n";
    return 27;
  }
  *out = {};
  const char* configured_theme =
      run_apk_app ? std::getenv("DARWIN_ART_APK_APP_THEME") : nullptr;
  const jint app_theme = configured_theme == nullptr
                             ? 0
                             : static_cast<jint>(
                                   std::strtoul(configured_theme, nullptr, 0));
  const char* configured_screen_orientation =
      run_apk_app ? std::getenv("DARWIN_ART_APK_APP_SCREEN_ORIENTATION") : nullptr;
  const jint app_screen_orientation =
      configured_screen_orientation == nullptr
          ? -1
          : static_cast<jint>(std::strtol(configured_screen_orientation, nullptr, 10));
  const char* configured_target_sdk =
      run_apk_app ? std::getenv("DARWIN_ART_APK_APP_TARGET_SDK") : nullptr;
  const jint app_target_sdk =
      configured_target_sdk == nullptr
          ? 36
          : static_cast<jint>(std::strtoul(configured_target_sdk, nullptr, 10));
  const char* configured_version_code =
      run_apk_app ? std::getenv("DARWIN_ART_APK_APP_VERSION_CODE") : nullptr;
  const jint app_version_code =
      configured_version_code == nullptr
          ? 0
          : static_cast<jint>(
                std::strtoul(configured_version_code, nullptr, 10));
  const char* configured_version_name =
      run_apk_app ? std::getenv("DARWIN_ART_APK_APP_VERSION_NAME") : nullptr;
  std::string app_label = run_apk_app &&
                                  std::getenv("DARWIN_ART_APK_APP_LABEL") != nullptr
                              ? std::getenv("DARWIN_ART_APK_APP_LABEL")
                              : "Darwin ART APK";
  const char* configured_app_label_res =
      run_apk_app ? std::getenv("DARWIN_ART_APK_APP_LABEL_RES") : nullptr;
  const jint app_label_res =
      configured_app_label_res == nullptr
          ? 0
          : static_cast<jint>(std::strtoul(configured_app_label_res, nullptr, 0));
  const char* configured_activity_label =
      run_apk_app ? std::getenv("DARWIN_ART_APK_ACTIVITY_LABEL") : nullptr;
  std::string activity_label = configured_activity_label == nullptr
                                   ? std::string()
                                   : configured_activity_label;
  const char* configured_activity_label_res =
      run_apk_app ? std::getenv("DARWIN_ART_APK_ACTIVITY_LABEL_RES") : nullptr;
  const jint activity_label_res =
      configured_activity_label_res == nullptr
          ? 0
          : static_cast<jint>(std::strtoul(configured_activity_label_res,
                                           nullptr, 0));
  if (run_apk_app) {
    jclass vm_runtime_class = env->FindClass("dalvik/system/VMRuntime");
    jmethodID get_vm_runtime =
        vm_runtime_class == nullptr
            ? nullptr
            : env->GetStaticMethodID(vm_runtime_class, "getRuntime",
                                     "()Ldalvik/system/VMRuntime;");
    jobject vm_runtime =
        get_vm_runtime == nullptr
            ? nullptr
            : env->CallStaticObjectMethod(vm_runtime_class, get_vm_runtime);
    jmethodID set_vm_target_sdk =
        vm_runtime_class == nullptr
            ? nullptr
            : env->GetMethodID(vm_runtime_class, "setTargetSdkVersion", "(I)V");
    if (vm_runtime == nullptr || set_vm_target_sdk == nullptr ||
        env->ExceptionCheck()) {
      std::cerr << "ART Android window: VM target SDK setup failed\n";
      return 27;
    }
    env->CallVoidMethod(vm_runtime, set_vm_target_sdk, app_target_sdk);
    env->DeleteLocalRef(vm_runtime);
    env->DeleteLocalRef(vm_runtime_class);

  }
  if (app_theme != 0 && resources->activity_info != nullptr) {
    jfieldID activity_theme = env->GetFieldID(
        resources->activity_info_class, "theme", "I");
    if (activity_theme != nullptr) {
      env->SetIntField(resources->activity_info, activity_theme, app_theme);
    }
  }
  if (run_apk_app && configured_screen_orientation != nullptr &&
      resources->activity_info != nullptr) {
    jfieldID activity_orientation = env->GetFieldID(
        resources->activity_info_class, "screenOrientation", "I");
    if (activity_orientation != nullptr && !env->ExceptionCheck()) {
      env->SetIntField(resources->activity_info, activity_orientation,
                       app_screen_orientation);
    }
  }
  if (run_apk_app && resources->activity_info != nullptr) {
    jfieldID activity_label_resource = env->GetFieldID(
        resources->activity_info_class, "labelRes", "I");
    jfieldID non_localized_label = env->GetFieldID(
        resources->activity_info_class, "nonLocalizedLabel",
        "Ljava/lang/CharSequence;");
    if (activity_label_res != 0 && activity_label_resource != nullptr &&
        !env->ExceptionCheck()) {
      // Keep resource-backed labels as a resource reference. Storing the
      // package fallback in nonLocalizedLabel would mask localization in
      // ActivityInfo.loadLabel().
      env->SetIntField(resources->activity_info, activity_label_resource,
                       activity_label_res);
      if (non_localized_label != nullptr && !env->ExceptionCheck()) {
        env->SetObjectField(resources->activity_info, non_localized_label,
                            nullptr);
      }
    }
    if (activity_label_res == 0 && !activity_label.empty()) {
      if (activity_label_resource != nullptr && !env->ExceptionCheck()) {
        env->SetIntField(resources->activity_info, activity_label_resource, 0);
      }
      jclass char_sequence_class = env->FindClass("java/lang/String");
      jstring literal = char_sequence_class == nullptr
                            ? nullptr
                            : env->NewStringUTF(activity_label.c_str());
      if (non_localized_label != nullptr && literal != nullptr &&
          !env->ExceptionCheck()) {
        env->SetObjectField(resources->activity_info, non_localized_label,
                            literal);
      }
      if (literal != nullptr) env->DeleteLocalRef(literal);
      if (char_sequence_class != nullptr)
        env->DeleteLocalRef(char_sequence_class);
    }
  }
  out->activity_class = env->GetSuperclass(probe_activity_class);
  jclass package_manager_class = env->GetObjectClass(package_manager);
  jmethodID configure_package_manager =
      package_manager_class == nullptr
          ? nullptr
          : env->GetMethodID(
                package_manager_class, "configure",
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;"
                "Ljava/lang/String;"
                "Ljava/lang/String;"
                "Landroid/content/pm/ActivityInfo;I"
                "Landroid/content/res/Resources;ILjava/lang/String;)V");
  jstring configured_package = env->NewStringUTF(
      run_apk_app ? apk_app_package : "dev.darwinart.probe");
  jstring configured_activity = env->NewStringUTF(
      run_apk_app ? apk_app_activity : "dev.darwinart.probe.ProbeActivity");
  const char* configured_activities =
      run_apk_app ? std::getenv("DARWIN_ART_APK_APP_ACTIVITIES") : nullptr;
  jstring configured_activity_names = env->NewStringUTF(
      configured_activities == nullptr
          ? "dev.darwinart.probe.ProbeActivity=0x0"
          : configured_activities);
  const char* configured_aliases =
      run_apk_app ? std::getenv("DARWIN_ART_APK_APP_ACTIVITY_ALIASES") : nullptr;
  jstring configured_activity_aliases = env->NewStringUTF(
      configured_aliases == nullptr ? "none" : configured_aliases);
  const char* configured_services =
      run_apk_app ? std::getenv("DARWIN_ART_APK_APP_SERVICES") : "none";
  jstring configured_service_names = env->NewStringUTF(
      configured_services == nullptr ? "none" : configured_services);
  jstring configured_version = env->NewStringUTF(
      configured_version_name == nullptr ? "0" : configured_version_name);
  if (configure_package_manager != nullptr && configured_package != nullptr &&
      configured_activity != nullptr && configured_service_names != nullptr &&
      configured_activity_names != nullptr &&
      configured_activity_aliases != nullptr &&
      configured_version != nullptr &&
      resources->activity_info != nullptr) {
    env->CallVoidMethod(package_manager, configure_package_manager,
                        configured_package, configured_activity,
                        configured_activity_names,
                        configured_activity_aliases,
                        configured_service_names,
                        resources->activity_info, app_target_sdk,
                        resources->probe_resources, app_version_code,
                        configured_version);
  }
  env->DeleteLocalRef(configured_version);
  env->DeleteLocalRef(configured_service_names);
  env->DeleteLocalRef(configured_activity_aliases);
  env->DeleteLocalRef(configured_activity_names);
  env->DeleteLocalRef(configured_activity);
  env->DeleteLocalRef(configured_package);
  env->DeleteLocalRef(package_manager_class);
  if (configure_package_manager == nullptr || configured_version == nullptr ||
      env->ExceptionCheck()) {
    std::cerr << "ART Android window: package metadata setup failed\n";
    return 27;
  }
  jclass intent_class = env->FindClass("android/content/Intent");
  jclass component_name_class =
      env->FindClass("android/content/ComponentName");
  jclass configuration_class =
      env->FindClass("android/content/res/Configuration");
  jmethodID configuration_constructor =
      configuration_class == nullptr
          ? nullptr
          : env->GetMethodID(configuration_class, "<init>",
                             "(Landroid/content/res/Configuration;)V");
  jclass resources_class = env->FindClass("android/content/res/Resources");
  jmethodID get_configuration =
      resources_class == nullptr
          ? nullptr
          : env->GetMethodID(resources_class, "getConfiguration",
                             "()Landroid/content/res/Configuration;");
  jobject resource_configuration =
      get_configuration == nullptr || resources->probe_resources == nullptr
          ? nullptr
          : env->CallObjectMethod(resources->probe_resources,
                                  get_configuration);
  jmethodID probe_context_constructor = env->GetMethodID(
      probe_context_class, "<init>",
      "(Landroid/content/res/Resources;"
      "Landroid/content/pm/PackageManager;Ljava/lang/String;)V");
  if (!run_apk_app) {
    probe_context_constructor = env->GetMethodID(
        probe_context_class, "<init>",
        "(Landroid/content/res/Resources;"
        "Landroid/content/pm/PackageManager;)V");
  }
  jstring context_package = env->NewStringUTF(
      run_apk_app ? apk_app_package : "dev.darwinart.probe");
  out->probe_context =
      probe_context_constructor == nullptr || resources->probe_resources == nullptr
          ? nullptr
          : (run_apk_app
                 ? env->NewObject(probe_context_class, probe_context_constructor,
                                  resources->probe_resources, package_manager,
                                  context_package)
                 : env->NewObject(probe_context_class, probe_context_constructor,
                                  resources->probe_resources, package_manager));
  env->DeleteLocalRef(context_package);
  if (out->probe_context == nullptr || env->ExceptionCheck()) {
    std::cerr << "ART Android window: ProbeContext construction failed\n";
    if (self->IsExceptionPending()) {
      std::cerr << self->GetException()->Dump() << "\n";
    }
    return 27;
  }
  jmethodID set_target_sdk = env->GetMethodID(
      probe_context_class, "setTargetSdkVersion", "(I)V");
  if (set_target_sdk == nullptr || env->ExceptionCheck()) {
    std::cerr << "ART Android window: target SDK setup failed\n";
    return 27;
  }
  env->CallVoidMethod(out->probe_context, set_target_sdk, app_target_sdk);
  if (env->ExceptionCheck()) {
    std::cerr << "ART Android window: target SDK setup threw\n";
    return 27;
  }
  jmethodID configure_compatibility = env->GetStaticMethodID(
      probe_context_class, "configureCompatibility", "(I)V");
  if (configure_compatibility == nullptr || env->ExceptionCheck()) {
    std::cerr << "ART Android window: compatibility setup failed\n";
    return 27;
  }
  env->CallStaticVoidMethod(probe_context_class, configure_compatibility,
                            app_target_sdk);
  if (env->ExceptionCheck()) {
    std::cerr << "ART Android window: compatibility setup threw\n";
    return 27;
  }

  jobject application = resources->application;
  if (run_apk_app) {
    application = darwin_art::framework::app::AwaitApplication(env);
    if (application == nullptr || env->ExceptionCheck()) {
      std::cerr << "ART Android process: framework application binding failed\n";
      if (self->IsExceptionPending()) std::cerr << self->GetException()->Dump() << "\n";
      return 27;
    }
  }

  // ActivityThread creates and attaches the Application before invoking the
  // Activity constructor. Chromium consults process-wide application state
  // directly from ChromeTabbedActivity's constructor, so constructing it in
  // the caller would invert a platform lifecycle guarantee.
  if (run_apk_app && activity_instance == nullptr) {
    out->application = application;
    if (application_only) return 0;
    jmethodID activity_constructor =
        env->GetMethodID(probe_activity_class, "<init>", "()V");
    activity_instance = activity_constructor == nullptr
                            ? nullptr
                            : env->NewObject(probe_activity_class,
                                             activity_constructor);
    if (activity_instance == nullptr || env->ExceptionCheck()) {
      std::cerr << "ART Android owner: Activity construction failed\n";
      if (self->IsExceptionPending()) {
        std::cerr << self->GetException()->Dump() << "\n";
      }
      return 23;
    }
    *activity_instance_out = activity_instance;
  }

  jmethodID intent_constructor =
      intent_class == nullptr
          ? nullptr
          : env->GetMethodID(intent_class, "<init>", "()V");
  jmethodID component_name_constructor =
      component_name_class == nullptr
          ? nullptr
          : env->GetMethodID(component_name_class, "<init>",
                             "(Ljava/lang/String;Ljava/lang/String;)V");
  jmethodID set_component =
      intent_class == nullptr
          ? nullptr
          : env->GetMethodID(intent_class, "setComponent",
                             "(Landroid/content/ComponentName;)"
                             "Landroid/content/Intent;");
  jmethodID set_action =
      intent_class == nullptr
          ? nullptr
          : env->GetMethodID(intent_class, "setAction",
                             "(Ljava/lang/String;)Landroid/content/Intent;");
  jmethodID add_category =
      intent_class == nullptr
          ? nullptr
          : env->GetMethodID(intent_class, "addCategory",
                             "(Ljava/lang/String;)Landroid/content/Intent;");
  jmethodID add_flags =
      intent_class == nullptr
          ? nullptr
          : env->GetMethodID(intent_class, "addFlags",
                             "(I)Landroid/content/Intent;");
  jmethodID set_data_and_type =
      intent_class == nullptr
          ? nullptr
          : env->GetMethodID(intent_class, "setDataAndType",
                             "(Landroid/net/Uri;Ljava/lang/String;)"
                             "Landroid/content/Intent;");
  jmethodID set_data =
      intent_class == nullptr
          ? nullptr
          : env->GetMethodID(intent_class, "setData",
                             "(Landroid/net/Uri;)Landroid/content/Intent;");
  jstring package_name = env->NewStringUTF(
      run_apk_app ? apk_app_package : "dev.darwinart.probe");
  const char* configured_launch_component =
      run_apk_app ? std::getenv("DARWIN_ART_APK_APP_LAUNCH_COMPONENT") : nullptr;
  jstring class_name = env->NewStringUTF(
      run_apk_app && configured_launch_component != nullptr
          ? configured_launch_component
          : (run_apk_app ? apk_app_activity
                         : "dev.darwinart.probe.ProbeActivity"));
  std::string window_title = run_apk_app ? app_label : "Darwin ART Probe";
  if (run_apk_app && app_label_res != 0 && resources->probe_resources != nullptr) {
    jclass resources_class = env->FindClass("android/content/res/Resources");
    jmethodID get_string =
        resources_class == nullptr
            ? nullptr
            : env->GetMethodID(resources_class, "getString",
                               "(I)Ljava/lang/String;");
    jstring resolved =
        get_string == nullptr || env->ExceptionCheck()
            ? nullptr
            : static_cast<jstring>(env->CallObjectMethod(
                  resources->probe_resources, get_string, app_label_res));
    if (resolved != nullptr && !env->ExceptionCheck()) {
      const char* utf = env->GetStringUTFChars(resolved, nullptr);
      if (utf != nullptr) {
        window_title = utf;
        env->ReleaseStringUTFChars(resolved, utf);
      }
    }
    if (env->ExceptionCheck()) env->ExceptionClear();
    if (resolved != nullptr) env->DeleteLocalRef(resolved);
    if (resources_class != nullptr) env->DeleteLocalRef(resources_class);
  }
  if (run_apk_app) {
    // Host presentation consumes a resolved title, while package metadata
    // keeps the original resource reference and literal label separate.
    setenv("DARWIN_ART_APK_WINDOW_TITLE", window_title.c_str(), 1);
    const DarwinArtSurfaceResult title_status =
        darwin_art_surface_set_active_title(window_title.c_str());
    if (title_status != DARWIN_ART_SURFACE_OK) {
      std::cerr << "ART Android window: could not publish resolved title status="
                << title_status << "\n";
      return 31;
    }
  }
  jstring title = env->NewStringUTF(window_title.c_str());
  jobject component_name =
      component_name_constructor == nullptr
          ? nullptr
          : env->NewObject(component_name_class, component_name_constructor,
                           package_name, class_name);
  jobject intent = intent_constructor == nullptr
                       ? nullptr
                       : env->NewObject(intent_class, intent_constructor);
  jobject configuration =
      configuration_constructor == nullptr || resource_configuration == nullptr
          ? nullptr
          : env->NewObject(configuration_class, configuration_constructor,
                           resource_configuration);
  jclass activity_thread_class = env->FindClass("android/app/ActivityThread");
  jfieldID current_activity_thread =
      activity_thread_class == nullptr
          ? nullptr
          : env->GetStaticFieldID(activity_thread_class,
                                  "sCurrentActivityThread",
                                  "Landroid/app/ActivityThread;");
  jobject activity_thread =
      current_activity_thread == nullptr
          ? nullptr
          : env->GetStaticObjectField(activity_thread_class,
                                      current_activity_thread);
  if (activity_thread == nullptr && activity_thread_class != nullptr &&
      !env->ExceptionCheck()) {
    jmethodID activity_thread_constructor =
        env->GetMethodID(activity_thread_class, "<init>", "()V");
    if (activity_thread_constructor != nullptr && !env->ExceptionCheck()) {
      activity_thread = env->NewObject(activity_thread_class,
                                       activity_thread_constructor);
      if (activity_thread != nullptr && !env->ExceptionCheck()) {
        if (!darwin_art::framework::app::AttachApplicationProcess(env, activity_thread)) {
          if (env->ExceptionCheck()) env->ExceptionDescribe();
          return 27;
        }
      }
    }
  }
  if (activity_thread == nullptr || env->ExceptionCheck()) {
    std::cerr << "ART Android process: ActivityThread configuration setup "
                 "failed\n";
    if (env->ExceptionCheck()) env->ExceptionDescribe();
    return 27;
  }
  jclass instrumentation_class = env->FindClass("android/app/Instrumentation");
  jmethodID instrumentation_constructor =
      instrumentation_class == nullptr
          ? nullptr
          : env->GetMethodID(instrumentation_class, "<init>", "()V");
  jobject instrumentation =
      instrumentation_constructor == nullptr
          ? nullptr
          : env->NewObject(instrumentation_class,
                           instrumentation_constructor);
  jclass binder_class = env->FindClass("android/os/Binder");
  jmethodID binder_constructor =
      binder_class == nullptr
          ? nullptr
          : env->GetMethodID(binder_class, "<init>", "()V");
  jobject activity_token =
      binder_constructor == nullptr
          ? nullptr
          : env->NewObject(binder_class, binder_constructor);
  if (intent != nullptr && set_component != nullptr && component_name != nullptr) {
    jobject configured_intent =
        env->CallObjectMethod(intent, set_component, component_name);
    env->DeleteLocalRef(configured_intent);
  }
  const char* launch_action =
      run_apk_app ? std::getenv("DARWIN_ART_APK_APP_INTENT_ACTION") : nullptr;
  const bool default_launcher_intent =
      run_apk_app && (launch_action == nullptr || *launch_action == '\0');
  if (intent != nullptr && set_action != nullptr && run_apk_app) {
    jstring action = env->NewStringUTF(
        default_launcher_intent ? "android.intent.action.MAIN" : launch_action);
    jobject configured_intent = env->CallObjectMethod(intent, set_action, action);
    env->DeleteLocalRef(configured_intent);
    env->DeleteLocalRef(action);
  }
  if (intent != nullptr && add_category != nullptr && default_launcher_intent &&
      !env->ExceptionCheck()) {
    jstring category =
        env->NewStringUTF("android.intent.category.LAUNCHER");
    jobject configured_intent =
        env->CallObjectMethod(intent, add_category, category);
    env->DeleteLocalRef(configured_intent);
    env->DeleteLocalRef(category);
  }
  if (intent != nullptr && add_flags != nullptr && default_launcher_intent &&
      !env->ExceptionCheck()) {
    constexpr jint kFlagActivityNewTask = 0x10000000;
    jobject configured_intent =
        env->CallObjectMethod(intent, add_flags, kFlagActivityNewTask);
    env->DeleteLocalRef(configured_intent);
  }
  const char* launch_uri =
      run_apk_app ? std::getenv("DARWIN_ART_APK_APP_INTENT_URI") : nullptr;
  const char* launch_type =
      run_apk_app ? std::getenv("DARWIN_ART_APK_APP_INTENT_TYPE") : nullptr;
  if (intent != nullptr && launch_uri != nullptr && *launch_uri != '\0' &&
      (set_data != nullptr || set_data_and_type != nullptr)) {
    jclass uri_class = env->FindClass("android/net/Uri");
    jmethodID parse_uri =
        uri_class == nullptr
            ? nullptr
            : env->GetStaticMethodID(uri_class, "parse",
                                     "(Ljava/lang/String;)Landroid/net/Uri;");
    jstring uri_text = env->NewStringUTF(launch_uri);
    jobject uri = parse_uri == nullptr
                      ? nullptr
                      : env->CallStaticObjectMethod(uri_class, parse_uri, uri_text);
    if (uri != nullptr && !env->ExceptionCheck()) {
      const bool has_explicit_type =
          launch_type != nullptr && *launch_type != '\0';
      jstring mime_type = has_explicit_type ? env->NewStringUTF(launch_type)
                                            : nullptr;
      jobject configured_intent = has_explicit_type && set_data_and_type != nullptr
                                      ? env->CallObjectMethod(
                                            intent, set_data_and_type, uri,
                                            mime_type)
                                      : env->CallObjectMethod(intent, set_data,
                                                              uri);
      env->DeleteLocalRef(configured_intent);
      env->DeleteLocalRef(mime_type);
    }
    env->DeleteLocalRef(uri);
    env->DeleteLocalRef(uri_text);
    env->DeleteLocalRef(uri_class);
  }
  static constexpr const char* kActivityAttachSignature =
      "(Landroid/content/Context;Landroid/app/ActivityThread;"
      "Landroid/app/Instrumentation;Landroid/os/IBinder;I"
      "Landroid/app/Application;Landroid/content/Intent;"
      "Landroid/content/pm/ActivityInfo;Ljava/lang/CharSequence;"
      "Landroid/app/Activity;Ljava/lang/String;"
      "Landroid/app/Activity$NonConfigurationInstances;"
      "Landroid/content/res/Configuration;Ljava/lang/String;"
      "Lcom/android/internal/app/IVoiceInteractor;Landroid/view/Window;"
      "Landroid/view/ViewRootImpl$ActivityConfigCallback;"
      "Landroid/os/IBinder;Landroid/os/IBinder;)V";
  jmethodID attach_activity =
      out->activity_class == nullptr
          ? nullptr
          : env->GetMethodID(out->activity_class, "attach",
                             kActivityAttachSignature);
  if (resources->activity_info == nullptr || resources->application == nullptr ||
      intent == nullptr || configuration == nullptr ||
      activity_thread == nullptr || instrumentation == nullptr ||
      activity_token == nullptr || attach_activity == nullptr ||
      env->ExceptionCheck()) {
    std::cerr << "ART Android window: Activity.attach() setup failed\n";
    if (self->IsExceptionPending()) {
      std::cerr << self->GetException()->Dump() << "\n";
    }
    return 27;
  }
  out->context_theme_wrapper_class =
      env->FindClass("android/view/ContextThemeWrapper");
  jmethodID attach_base_context =
      out->context_theme_wrapper_class == nullptr
          ? nullptr
          : env->GetMethodID(out->context_theme_wrapper_class,
                             "attachBaseContext",
                             "(Landroid/content/Context;)V");
  // Activity.attach() invokes the virtual attachBaseContext hook itself.  A
  // pre-call here leaves ContextWrapper.mBase initialized twice and AOSP
  // correctly throws "Base context already set" on the second call.
  if (attach_base_context == nullptr || env->ExceptionCheck()) {
    std::cerr << "ART Android window: base Context preparation failed\n";
    if (self->IsExceptionPending()) {
      std::cerr << self->GetException()->Dump() << "\n";
    }
    return 30;
  }
  env->CallNonvirtualVoidMethod(
      activity_instance, out->activity_class, attach_activity,
      out->probe_context, activity_thread, instrumentation, activity_token,
      static_cast<jint>(1),
      application, intent, resources->activity_info, title, nullptr,
      nullptr, nullptr, configuration, nullptr, nullptr, nullptr, nullptr,
      activity_token, activity_token);
  env->DeleteLocalRef(activity_token);
  env->DeleteLocalRef(binder_class);
  env->DeleteLocalRef(instrumentation);
  env->DeleteLocalRef(instrumentation_class);
  env->DeleteLocalRef(activity_thread);
  env->DeleteLocalRef(activity_thread_class);
  if (env->ExceptionCheck()) {
    std::cerr << "ART Android window: Activity.attach() threw\n"
              << self->GetException()->Dump() << "\n";
    return 30;
  }
  jmethodID get_window = env->GetMethodID(
      out->activity_class, "getWindow", "()Landroid/view/Window;");
  out->window = get_window == nullptr
                    ? nullptr
                    : env->CallObjectMethod(activity_instance, get_window);
  out->window_class = env->FindClass("android/view/Window");
  out->phone_window_class =
      env->FindClass("com/android/internal/policy/PhoneWindow");
  if (out->window == nullptr || out->phone_window_class == nullptr ||
      !env->IsInstanceOf(out->window, out->phone_window_class) ||
      env->ExceptionCheck()) {
    std::cerr << "ART Android window: PhoneWindow attachment failed\n";
    if (self->IsExceptionPending()) {
      std::cerr << self->GetException()->Dump() << "\n";
    }
    return 31;
  }
  jmethodID get_probe_theme = env->GetMethodID(
      probe_context_class, "getTheme",
      "()Landroid/content/res/Resources$Theme;");
  out->probe_theme = get_probe_theme == nullptr
                         ? nullptr
                         : env->CallObjectMethod(out->probe_context,
                                                 get_probe_theme);
  if (use_framework_resources && out->probe_theme != nullptr) {
    jclass theme_class = env->GetObjectClass(out->probe_theme);
    jclass framework_style_class = env->FindClass("android/R$style");
    jfieldID framework_light_no_action_bar =
        framework_style_class == nullptr
            ? nullptr
            : env->GetStaticFieldID(framework_style_class,
                                    "Theme_Material_Light_NoActionBar", "I");
    jmethodID apply_style =
        theme_class == nullptr
            ? nullptr
            : env->GetMethodID(theme_class, "applyStyle", "(IZ)V");
    if (framework_light_no_action_bar != nullptr && apply_style != nullptr) {
      const jint style = env->GetStaticIntField(
          framework_style_class, framework_light_no_action_bar);
      env->CallVoidMethod(out->probe_theme, apply_style, style, JNI_TRUE);
    }
    if (run_apk_app && app_theme != 0 && apply_style != nullptr &&
        !env->ExceptionCheck()) {
      env->CallVoidMethod(out->probe_theme, apply_style, app_theme, JNI_TRUE);
    }
    env->DeleteLocalRef(framework_style_class);
    env->DeleteLocalRef(theme_class);
  }
  jmethodID set_activity_theme =
      env->GetMethodID(out->context_theme_wrapper_class, "setTheme",
                       "(Landroid/content/res/Resources$Theme;)V");
  if (out->probe_theme == nullptr || set_activity_theme == nullptr ||
      env->ExceptionCheck()) {
    std::cerr << "ART Android window: Activity theme setup failed\n";
    return 31;
  }
  env->CallVoidMethod(activity_instance, set_activity_theme, out->probe_theme);
  if (env->ExceptionCheck()) {
    std::cerr << "ART Android window: Activity.setTheme() threw\n"
              << self->GetException()->Dump() << "\n";
    return 31;
  }
  env->DeleteLocalRef(configuration);
  env->DeleteLocalRef(component_name);
  env->DeleteLocalRef(intent);
  env->DeleteLocalRef(title);
  env->DeleteLocalRef(class_name);
  env->DeleteLocalRef(package_name);
  env->DeleteLocalRef(configuration_class);
  env->DeleteLocalRef(resource_configuration);
  env->DeleteLocalRef(resources_class);
  env->DeleteLocalRef(component_name_class);
  env->DeleteLocalRef(intent_class);
  if (run_apk_app) env->DeleteLocalRef(application);
  return 0;
}

void release(JNIEnv* env, Bundle* bundle) {
  if (env == nullptr || bundle == nullptr) {
    return;
  }
  env->DeleteLocalRef(bundle->probe_theme);
  env->DeleteLocalRef(bundle->window);
  env->DeleteLocalRef(bundle->phone_window_class);
  env->DeleteLocalRef(bundle->window_class);
  env->DeleteLocalRef(bundle->context_theme_wrapper_class);
  env->DeleteLocalRef(bundle->activity_class);
  env->DeleteLocalRef(bundle->probe_context);
  *bundle = {};
}

}  // namespace darwin_art_app_activity
