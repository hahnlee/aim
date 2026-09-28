#include "runtime_app_resources.h"

namespace aim_app_resources {
namespace {

bool InstallInitialDisplayConfiguration(JNIEnv* env, jobject resources,
                                        jint window_scale) {
  jclass resources_class = env->FindClass("android/content/res/Resources");
  jmethodID get_configuration =
      resources_class == nullptr
          ? nullptr
          : env->GetMethodID(resources_class, "getConfiguration",
                             "()Landroid/content/res/Configuration;");
  jobject configuration =
      get_configuration == nullptr
          ? nullptr
          : env->CallObjectMethod(resources, get_configuration);
  jclass configuration_class =
      env->FindClass("android/content/res/Configuration");
  jfieldID window_configuration_field =
      configuration_class == nullptr
          ? nullptr
          : env->GetFieldID(configuration_class, "windowConfiguration",
                            "Landroid/app/WindowConfiguration;");
  jobject window_configuration =
      window_configuration_field == nullptr || configuration == nullptr
          ? nullptr
          : env->GetObjectField(configuration, window_configuration_field);
  jclass window_configuration_class =
      env->FindClass("android/app/WindowConfiguration");
  jclass rect_class = env->FindClass("android/graphics/Rect");
  jmethodID rect_constructor =
      rect_class == nullptr
          ? nullptr
          : env->GetMethodID(rect_class, "<init>", "(IIII)V");
  jobject bounds = rect_constructor == nullptr
                       ? nullptr
                       : env->NewObject(rect_class, rect_constructor, 0, 0,
                                        360 * window_scale,
                                        640 * window_scale);
  jmethodID set_bounds =
      window_configuration_class == nullptr
          ? nullptr
          : env->GetMethodID(window_configuration_class, "setBounds",
                             "(Landroid/graphics/Rect;)V");
  jmethodID set_max_bounds =
      window_configuration_class == nullptr
          ? nullptr
          : env->GetMethodID(window_configuration_class, "setMaxBounds",
                             "(Landroid/graphics/Rect;)V");
  if (window_configuration == nullptr || bounds == nullptr ||
      set_bounds == nullptr || set_max_bounds == nullptr ||
      env->ExceptionCheck()) {
    return false;
  }
  env->CallVoidMethod(window_configuration, set_bounds, bounds);
  env->CallVoidMethod(window_configuration, set_max_bounds, bounds);

  jclass resources_manager_class =
      env->FindClass("android/app/ResourcesManager");
  jmethodID get_instance =
      resources_manager_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(resources_manager_class, "getInstance",
                                   "()Landroid/app/ResourcesManager;");
  jobject resources_manager =
      get_instance == nullptr
          ? nullptr
          : env->CallStaticObjectMethod(resources_manager_class, get_instance);
  jmethodID apply_configuration =
      resources_manager_class == nullptr
          ? nullptr
          : env->GetMethodID(
                resources_manager_class, "applyConfigurationToResources",
                "(Landroid/content/res/Configuration;"
                "Landroid/content/res/CompatibilityInfo;)Z");
  if (resources_manager == nullptr || apply_configuration == nullptr ||
      env->ExceptionCheck()) {
    return false;
  }
  env->CallBooleanMethod(resources_manager, apply_configuration, configuration,
                         nullptr);
  return !env->ExceptionCheck();
}

}  // namespace

int prepare(JNIEnv* env, jclass probe_resources_class,
            bool use_framework_resources, jint window_scale,
            const char* framework_res_apk, const char* app_apk_path,
            Bundle* out) {
  if (env == nullptr || probe_resources_class == nullptr || out == nullptr ||
      (use_framework_resources && framework_res_apk == nullptr)) {
    return 27;
  }
  *out = {};
  out->activity_info_class =
      env->FindClass("android/content/pm/ActivityInfo");
  out->application_class = env->FindClass("android/app/Application");
  out->asset_manager_class = env->FindClass("android/content/res/AssetManager");
  out->apk_assets_class = env->FindClass("android/content/res/ApkAssets");
  jmethodID activity_info_constructor =
      out->activity_info_class == nullptr
          ? nullptr
          : env->GetMethodID(out->activity_info_class, "<init>", "()V");
  jmethodID application_constructor =
      out->application_class == nullptr
          ? nullptr
          : env->GetMethodID(out->application_class, "<init>", "()V");
  out->activity_info =
      activity_info_constructor == nullptr
          ? nullptr
          : env->NewObject(out->activity_info_class, activity_info_constructor);
  out->application =
      application_constructor == nullptr
          ? nullptr
          : env->NewObject(out->application_class, application_constructor);
  jmethodID asset_manager_constructor =
      out->asset_manager_class == nullptr
          ? nullptr
          : env->GetMethodID(out->asset_manager_class, "<init>", "(Z)V");
  out->asset_manager =
      asset_manager_constructor == nullptr
          ? nullptr
          : env->NewObject(out->asset_manager_class, asset_manager_constructor,
                           JNI_TRUE);

  if (use_framework_resources && out->apk_assets_class != nullptr &&
      out->asset_manager != nullptr) {
    jmethodID load_from_path = env->GetStaticMethodID(
        out->apk_assets_class, "loadFromPath",
        "(Ljava/lang/String;)Landroid/content/res/ApkAssets;");
    out->framework_res_path = env->NewStringUTF(framework_res_apk);
    out->framework_apk_assets =
        load_from_path == nullptr || out->framework_res_path == nullptr
            ? nullptr
            : env->CallStaticObjectMethod(out->apk_assets_class, load_from_path,
                                          out->framework_res_path);
    // Framework-only probes have no application APK.  Do not call
    // ApkAssets.loadFromPath("") in that mode: AOSP treats the empty path as
    // an actual asset request and throws IOException before the first frame.
    if (app_apk_path != nullptr && app_apk_path[0] != '\0') {
      out->app_apk_path = env->NewStringUTF(app_apk_path);
      jmethodID load_app = env->GetStaticMethodID(
          out->apk_assets_class, "loadFromPath",
          "(Ljava/lang/String;)Landroid/content/res/ApkAssets;");
      out->app_apk_assets =
          load_app == nullptr || out->app_apk_path == nullptr
              ? nullptr
              : env->CallStaticObjectMethod(out->apk_assets_class, load_app,
                                            out->app_apk_path);
    }
    const jint asset_count = out->framework_apk_assets == nullptr
                                 ? 0
                                 : (out->app_apk_assets == nullptr ? 1 : 2);
    out->configured_apk_assets =
        asset_count == 0
            ? nullptr
            : env->NewObjectArray(asset_count, out->apk_assets_class, nullptr);
    if (out->configured_apk_assets != nullptr) {
      env->SetObjectArrayElement(out->configured_apk_assets, 0,
                                 out->framework_apk_assets);
      if (out->app_apk_assets != nullptr) {
        env->SetObjectArrayElement(out->configured_apk_assets, 1,
                                   out->app_apk_assets);
      }
    }
  } else if (out->apk_assets_class != nullptr) {
    out->configured_apk_assets =
        env->NewObjectArray(0, out->apk_assets_class, nullptr);
  }

  jfieldID apk_assets_field =
      out->asset_manager_class == nullptr
          ? nullptr
          : env->GetFieldID(out->asset_manager_class, "mApkAssets",
                            "[Landroid/content/res/ApkAssets;");
  if (!use_framework_resources && out->asset_manager != nullptr &&
      apk_assets_field != nullptr && out->configured_apk_assets != nullptr) {
    env->SetObjectField(out->asset_manager, apk_assets_field,
                        out->configured_apk_assets);
  } else if (use_framework_resources && out->asset_manager != nullptr &&
             apk_assets_field != nullptr && out->configured_apk_assets != nullptr) {
    jfieldID asset_manager_object =
        env->GetFieldID(out->asset_manager_class, "mObject", "J");
    jmethodID native_set_apk_assets = env->GetStaticMethodID(
        out->asset_manager_class, "nativeSetApkAssets",
        "(J[Landroid/content/res/ApkAssets;ZZ)V");
    if (asset_manager_object != nullptr && native_set_apk_assets != nullptr) {
      const jlong native_asset_manager =
          env->GetLongField(out->asset_manager, asset_manager_object);
      env->CallStaticVoidMethod(out->asset_manager_class, native_set_apk_assets,
                                native_asset_manager,
                                out->configured_apk_assets, JNI_TRUE, JNI_FALSE);
      if (!env->ExceptionCheck()) {
        env->SetObjectField(out->asset_manager, apk_assets_field,
                            out->configured_apk_assets);
      }
    }
  }

  jmethodID configure_display_scale = env->GetStaticMethodID(
      probe_resources_class, "configureDisplayScale", "(I)V");
  if (configure_display_scale != nullptr) {
    env->CallStaticVoidMethod(probe_resources_class, configure_display_scale,
                              window_scale);
  }
  jmethodID probe_resources_constructor = env->GetMethodID(
      probe_resources_class, "<init>",
      "(Landroid/content/res/AssetManager;Z)V");
  out->probe_resources =
      probe_resources_constructor == nullptr || out->asset_manager == nullptr ||
              env->ExceptionCheck()
          ? nullptr
          : env->NewObject(probe_resources_class, probe_resources_constructor,
                           out->asset_manager,
                           use_framework_resources ? JNI_TRUE : JNI_FALSE);
  if (out->probe_resources != nullptr && !env->ExceptionCheck() &&
      !InstallInitialDisplayConfiguration(env, out->probe_resources,
                                          window_scale)) {
    return 27;
  }
  if (use_framework_resources && out->asset_manager != nullptr &&
      out->probe_resources != nullptr && !env->ExceptionCheck()) {
    // Zygote normally initializes these process-wide singletons before any
    // application or background worker can call Resources.getSystem(). The
    // Darwin process has no Zygote, so publish the already validated pinned
    // framework resources at the equivalent bootstrap boundary.
    jfieldID system_assets = env->GetStaticFieldID(
        out->asset_manager_class, "sSystem",
        "Landroid/content/res/AssetManager;");
    jclass resources_class = env->FindClass("android/content/res/Resources");
    jfieldID system_resources =
        resources_class == nullptr
            ? nullptr
            : env->GetStaticFieldID(resources_class, "mSystem",
                                    "Landroid/content/res/Resources;");
    if (system_assets != nullptr && system_resources != nullptr &&
        !env->ExceptionCheck()) {
      env->SetStaticObjectField(out->asset_manager_class, system_assets,
                                out->asset_manager);
      env->SetStaticObjectField(resources_class, system_resources,
                                out->probe_resources);
    }
    env->DeleteLocalRef(resources_class);
  }
  if (out->activity_info == nullptr || out->application == nullptr ||
      out->asset_manager == nullptr || out->configured_apk_assets == nullptr ||
      out->probe_resources == nullptr || env->ExceptionCheck()) {
    return 27;
  }
  return 0;
}

void release(JNIEnv* env, Bundle* bundle) {
  if (env == nullptr || bundle == nullptr) {
    return;
  }
  env->DeleteLocalRef(bundle->probe_resources);
  env->DeleteLocalRef(bundle->configured_apk_assets);
  env->DeleteLocalRef(bundle->framework_res_path);
  env->DeleteLocalRef(bundle->app_apk_path);
  env->DeleteLocalRef(bundle->app_apk_assets);
  env->DeleteLocalRef(bundle->framework_apk_assets);
  env->DeleteLocalRef(bundle->apk_assets_class);
  env->DeleteLocalRef(bundle->asset_manager);
  env->DeleteLocalRef(bundle->asset_manager_class);
  env->DeleteLocalRef(bundle->application);
  env->DeleteLocalRef(bundle->application_class);
  env->DeleteLocalRef(bundle->activity_info);
  env->DeleteLocalRef(bundle->activity_info_class);
  *bundle = {};
}

}  // namespace aim_app_resources
