#include "application_binding.h"
#include "../display/configuration.h"
#include "../pm/declared_providers.h"
#include "../system/application_shared_memory.h"
#include "../compat/policy_binding.h"
namespace darwin_art::framework::am {
bool DispatchApplicationBinding(JNIEnv* env, jobject endpoint, jobject info,
    jobject resources, jstring process_name, const char* encoded) {
  if (env->ExceptionCheck() || endpoint == nullptr || info == nullptr ||
      resources == nullptr || env->PushLocalFrame(64) < 0) return false;
  auto fail = [&]() { env->PopLocalFrame(nullptr); return false; };
  jclass resource_type = env->GetObjectClass(resources);
  jmethodID configuration = env->GetMethodID(resource_type, "getConfiguration",
                                             "()Landroid/content/res/Configuration;");
  if (configuration == nullptr) return fail();
  jobject config = env->CallObjectMethod(resources, configuration);
  config = display::ConfigurationForApplicationTask(env, endpoint, config);
  if (config == nullptr || env->ExceptionCheck()) return fail();
  jclass compat_type = env->FindClass("android/content/res/CompatibilityInfo");
  if (compat_type == nullptr) return fail();
  jfieldID default_compat = env->GetStaticFieldID(
      compat_type, "DEFAULT_COMPATIBILITY_INFO", "Landroid/content/res/CompatibilityInfo;");
  if (default_compat == nullptr) return fail();
  jobject compatibility_info = env->GetStaticObjectField(compat_type, default_compat);
  jobject providers = pm::DeclaredProviders(env, info,
      encoded);
  if (providers == nullptr || env->ExceptionCheck()) return fail();
  jclass list_type = env->FindClass("android/content/pm/ProviderInfoList");
  if (list_type == nullptr) return fail();
  jmethodID list_ctor = env->GetMethodID(list_type, "<init>", "(Ljava/util/List;)V");
  if (list_ctor == nullptr) return fail();
  jobject provider_list = env->NewObject(list_type, list_ctor, providers);
  if (provider_list == nullptr || env->ExceptionCheck()) return fail();
  jclass bundle_type = env->FindClass("android/os/Bundle");
  if (bundle_type == nullptr) return fail();
  jmethodID bundle_ctor = env->GetMethodID(bundle_type, "<init>", "()V");
  if (bundle_ctor == nullptr) return fail();
  jobject settings = env->NewObject(bundle_type, bundle_ctor);
  compat::ApplicationChanges changes;
  if (!compat::EvaluateApplicationChanges(env, info, &changes)) return fail();
  jobject application_memory =
      system::DuplicateApplicationSharedMemoryReader(env);
  if (settings == nullptr || application_memory == nullptr ||
      env->ExceptionCheck()) return fail();
  jclass endpoint_type = env->GetObjectClass(endpoint);
  jmethodID bind = env->GetMethodID(endpoint_type, "bindApplication",
      "(Ljava/lang/String;Landroid/content/pm/ApplicationInfo;Ljava/lang/String;"
      "Ljava/lang/String;ZLandroid/content/pm/ProviderInfoList;Landroid/content/ComponentName;"
      "Landroid/app/ProfilerInfo;Landroid/os/Bundle;Landroid/app/IInstrumentationWatcher;"
      "Landroid/app/IUiAutomationConnection;IZZZZLandroid/content/res/Configuration;"
      "Landroid/content/res/CompatibilityInfo;Ljava/util/Map;Landroid/os/Bundle;"
      "Ljava/lang/String;Landroid/content/AutofillOptions;Landroid/content/ContentCaptureOptions;"
      "[J[JLandroid/os/SharedMemory;Ljava/io/FileDescriptor;JJ)V");
  if (bind == nullptr) return fail();
  jvalue args[29]{};
  args[0].l = process_name;
  args[1].l = info;
  args[5].l = provider_list;
  args[16].l = config;
  args[17].l = compatibility_info;
  args[19].l = settings;
  args[23].l = changes.disabled;
  args[24].l = changes.loggable;
  // serializedSystemFontMap is nullable SharedMemory. The following raw
  // FileDescriptor is not: the generated AIDL proxy always writes it.
  args[26].l = application_memory;
  env->CallVoidMethodA(endpoint, bind, args);
  jthrowable dispatch_failure = env->ExceptionOccurred();
  if (dispatch_failure != nullptr) env->ExceptionClear();
  system::CloseApplicationSharedMemoryReader(env, application_memory);
  if (dispatch_failure != nullptr) {
    if (env->ExceptionCheck()) env->ExceptionClear();
    env->Throw(dispatch_failure);
    return fail();
  }
  if (env->ExceptionCheck()) return fail();

  env->PopLocalFrame(nullptr);
  return true;
}
}
