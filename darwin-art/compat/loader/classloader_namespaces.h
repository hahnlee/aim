#pragma once

#include <jni.h>

#include <string>

namespace darwin_art::loader {

void InitializeClassLoaderNamespaces();
void ResetClassLoaderNamespaces();

bool CreateClassLoaderNamespace(JNIEnv* env, int32_t target_sdk_version,
                                jobject class_loader, bool is_shared,
                                jstring dex_path, jstring library_search_path,
                                jstring library_permitted_path,
                                jstring uses_library_list,
                                std::string* error);

bool FindClassLoaderLibrary(JNIEnv* env, jobject class_loader,
                            const char* soname, std::string* path,
                            std::string* error);

}  // namespace darwin_art::loader
