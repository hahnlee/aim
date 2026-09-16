#pragma once

#include <jni.h>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>
#include "provider_metadata.h"

namespace darwin_art::framework::pm {


inline jobject DeclaredProviders(JNIEnv* env, jobject application_info, const char* encoded) {
  if (env == nullptr || application_info == nullptr || env->ExceptionCheck()) return nullptr;
  if (!ValidateProviderMetadata(encoded)) {
    jclass error = env->FindClass("java/lang/IllegalArgumentException");
    if (error != nullptr) env->ThrowNew(error, "Invalid installed provider metadata");
    env->DeleteLocalRef(error);
    return nullptr;
  }
  jclass list_type = env->FindClass("java/util/ArrayList");
  if (list_type == nullptr) return nullptr;
  jmethodID constructor = env->GetMethodID(list_type, "<init>", "()V");
  jmethodID add = env->GetMethodID(list_type, "add", "(Ljava/lang/Object;)Z");
  if (constructor == nullptr || add == nullptr) return nullptr;
  jobject list = env->NewObject(list_type, constructor);
  if (list == nullptr || env->ExceptionCheck()) return nullptr;
  if (encoded == nullptr || encoded[0] == '\0' || std::strcmp(encoded, "none") == 0) {
    env->DeleteLocalRef(list_type);
    return list;
  }
  jclass provider_info_class = env->FindClass("android/content/pm/ProviderInfo");
  jmethodID provider_info_constructor =
      provider_info_class == nullptr
          ? nullptr
          : env->GetMethodID(provider_info_class, "<init>", "()V");
  jclass bundle_class = env->FindClass("android/os/Bundle");
  jmethodID bundle_constructor = bundle_class == nullptr
                                     ? nullptr
                                     : env->GetMethodID(bundle_class, "<init>", "()V");
  jmethodID put_string = bundle_class == nullptr
                             ? nullptr
                             : env->GetMethodID(bundle_class, "putString",
                                                "(Ljava/lang/String;Ljava/lang/String;)V");
  jmethodID put_int = bundle_class == nullptr
                          ? nullptr
                          : env->GetMethodID(bundle_class, "putInt",
                                             "(Ljava/lang/String;I)V");
  jmethodID put_boolean = bundle_class == nullptr
                              ? nullptr
                              : env->GetMethodID(bundle_class, "putBoolean",
                                                 "(Ljava/lang/String;Z)V");
  jfieldID info_name = provider_info_class == nullptr
                           ? nullptr
                           : env->GetFieldID(provider_info_class, "name",
                                             "Ljava/lang/String;");
  jfieldID info_package_name = provider_info_class == nullptr
                                   ? nullptr
                                   : env->GetFieldID(provider_info_class, "packageName",
                                                     "Ljava/lang/String;");
  jfieldID info_process_name = provider_info_class == nullptr
                                   ? nullptr
                                   : env->GetFieldID(provider_info_class, "processName",
                                                     "Ljava/lang/String;");
  jfieldID info_authority = provider_info_class == nullptr
                                ? nullptr
                                : env->GetFieldID(provider_info_class, "authority",
                                                  "Ljava/lang/String;");
  jfieldID info_init_order = provider_info_class == nullptr
                                 ? nullptr
                                 : env->GetFieldID(provider_info_class, "initOrder", "I");
  jfieldID info_grant_uri_permissions =
      provider_info_class == nullptr
          ? nullptr
          : env->GetFieldID(provider_info_class, "grantUriPermissions", "Z");
  jfieldID info_application = provider_info_class == nullptr
                                  ? nullptr
                                  : env->GetFieldID(
                                        provider_info_class, "applicationInfo",
                                        "Landroid/content/pm/ApplicationInfo;");
  jfieldID info_metadata = provider_info_class == nullptr
                               ? nullptr
                               : env->GetFieldID(provider_info_class, "metaData",
                                                 "Landroid/os/Bundle;");
  jclass application_info_class = env->GetObjectClass(application_info);
  jfieldID application_package_name = application_info_class == nullptr
                                          ? nullptr
                                          : env->GetFieldID(application_info_class, "packageName",
                                                            "Ljava/lang/String;");
  jobject package_name = application_package_name == nullptr
                             ? nullptr
                             : env->GetObjectField(application_info,
                                                   application_package_name);
  if (provider_info_constructor == nullptr ||
      bundle_constructor == nullptr || info_name == nullptr ||
      info_package_name == nullptr || info_process_name == nullptr ||
      info_authority == nullptr || info_init_order == nullptr ||
      info_grant_uri_permissions == nullptr ||
      info_application == nullptr || info_metadata == nullptr ||
      application_package_name == nullptr || package_name == nullptr ||
      env->ExceptionCheck()) {
    return nullptr;
  }
  for (const std::string& item : Split(encoded, ';')) {
    if (item.empty()) continue;
    const size_t first = item.find('>');
    const size_t second = first == std::string::npos
                              ? std::string::npos
                              : item.find('>', first + 1);
    const size_t third = second == std::string::npos
                             ? std::string::npos
                             : item.find('>', second + 1);
    const size_t fourth = third == std::string::npos
                              ? std::string::npos
                              : item.find('>', third + 1);
    if (first == std::string::npos || second == std::string::npos ||
        third == std::string::npos || fourth == std::string::npos) {
      return nullptr;
    }
    std::string provider_name;
    std::string authority;
    if (!DecodeHex(item.substr(0, first), &provider_name) ||
        !DecodeHex(item.substr(first + 1, second - first - 1), &authority)) {
      return nullptr;
    }
    const unsigned long init_order = std::strtoul(
        item.substr(second + 1, third - second - 1).c_str(), nullptr, 16);
    const bool grant_uri_permissions = item.substr(
        third + 1, fourth - third - 1) == "1";
    jobject info = env->NewObject(provider_info_class, provider_info_constructor);
    if (info == nullptr || env->ExceptionCheck()) return nullptr;
    jobject metadata = env->NewObject(bundle_class, bundle_constructor);
    jstring name_value = env->NewStringUTF(provider_name.c_str());
    jstring authority_value = env->NewStringUTF(authority.c_str());
    if (info != nullptr && metadata != nullptr &&
        name_value != nullptr && authority_value != nullptr &&
        !env->ExceptionCheck()) {
      env->SetObjectField(info, info_name, name_value);
      env->SetObjectField(info, info_package_name, package_name);
      // The installed record currently has no per-provider process override;
      // Android's manifest default is the application package process.
      env->SetObjectField(info, info_process_name, package_name);
      env->SetObjectField(info, info_authority, authority_value);
      env->SetIntField(info, info_init_order, static_cast<jint>(init_order));
      env->SetBooleanField(info, info_grant_uri_permissions,
                           grant_uri_permissions ? JNI_TRUE : JNI_FALSE);
      if (application_info != nullptr) {
        env->SetObjectField(info, info_application, application_info);
      }
      for (const std::string& metadata_item :
           Split(item.substr(fourth + 1), ',')) {
        const size_t colon = metadata_item.find(':');
        const size_t value_colon = colon == std::string::npos
                                       ? std::string::npos
                                       : metadata_item.find(':', colon + 1);
        if (colon == std::string::npos || value_colon == std::string::npos) continue;
        std::string metadata_name;
        if (!DecodeHex(metadata_item.substr(0, colon), &metadata_name)) continue;
        jstring key = env->NewStringUTF(metadata_name.c_str());
        const char kind = metadata_item[colon + 1];
        const std::string value = metadata_item.substr(value_colon + 1);
        if (kind == 's' && put_string != nullptr) {
          std::string string_value;
          if (DecodeHex(value, &string_value)) {
            jstring text = env->NewStringUTF(string_value.c_str());
            env->CallVoidMethod(metadata, put_string, key, text);
            env->DeleteLocalRef(text);
          }
        } else if ((kind == 'i' || kind == 'r') && put_int != nullptr) {
          const jint integer = static_cast<jint>(std::strtoul(value.c_str(), nullptr, 16));
          env->CallVoidMethod(metadata, put_int, key, integer);
        } else if (kind == 'b' && put_boolean != nullptr) {
          env->CallVoidMethod(metadata, put_boolean, key, value == "1");
        }
        env->DeleteLocalRef(key);
      }
      env->SetObjectField(info, info_metadata, metadata);
      env->CallBooleanMethod(list, add, info);
    }
    env->DeleteLocalRef(authority_value);
    env->DeleteLocalRef(name_value);
    env->DeleteLocalRef(metadata);
    env->DeleteLocalRef(info);
    if (env->ExceptionCheck()) {
      return nullptr;
    }
  }
  env->DeleteLocalRef(bundle_class);
  env->DeleteLocalRef(provider_info_class);
  env->DeleteLocalRef(package_name);
  env->DeleteLocalRef(application_info_class);
  env->DeleteLocalRef(list_type);
  return env->ExceptionCheck() ? nullptr : list;
}


}  // namespace darwin_art::framework::pm
