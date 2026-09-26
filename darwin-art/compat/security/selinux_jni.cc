#include "selinux_jni.h"

// android_os_SELinux.cpp decides at registration whether SELinux is enabled
// (is_selinux_enabled() != 1) and every native then takes its disabled
// branch. This host has no SELinux, so these are exactly those branches:
// no contexts exist, labelling reports failure, access checks and
// restorecon succeed. getGenfsLabelsVersion reads the device policy through
// libselinux and has no disabled branch; it stays unregistered.
namespace darwin_art::security {
namespace {
jboolean IsEnabled(JNIEnv*, jclass) { return JNI_FALSE; }
// security_getenforce() fails without selinuxfs.
jboolean IsEnforced(JNIEnv*, jclass) { return JNI_FALSE; }
jstring NoContext(JNIEnv*, jclass) { return nullptr; }
jstring NoPathContext(JNIEnv*, jclass, jstring) { return nullptr; }
jstring NoFdContext(JNIEnv*, jclass, jobject) { return nullptr; }
jstring NoPidContext(JNIEnv*, jclass, jint) { return nullptr; }
jboolean SetFsCreateContext(JNIEnv*, jclass, jstring) { return JNI_FALSE; }
jboolean SetFileContext(JNIEnv*, jclass, jstring, jstring) { return JNI_FALSE; }
jboolean CheckAccess(JNIEnv*, jclass, jstring, jstring, jstring, jstring) { return JNI_TRUE; }
jboolean Restorecon(JNIEnv*, jclass, jstring, jint) { return JNI_TRUE; }
}  // namespace

bool RegisterSELinuxNatives(JNIEnv* env) {
  jclass selinux = env->FindClass("android/os/SELinux");
  if (selinux == nullptr) return false;
  const JNINativeMethod methods[] = {
      {const_cast<char*>("checkSELinuxAccess"),
       const_cast<char*>("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Z"),
       reinterpret_cast<void*>(CheckAccess)},
      {const_cast<char*>("getContext"), const_cast<char*>("()Ljava/lang/String;"),
       reinterpret_cast<void*>(NoContext)},
      {const_cast<char*>("getFileContext"), const_cast<char*>("(Ljava/lang/String;)Ljava/lang/String;"),
       reinterpret_cast<void*>(NoPathContext)},
      {const_cast<char*>("getPeerContext"),
       const_cast<char*>("(Ljava/io/FileDescriptor;)Ljava/lang/String;"),
       reinterpret_cast<void*>(NoFdContext)},
      {const_cast<char*>("getFileContext"),
       const_cast<char*>("(Ljava/io/FileDescriptor;)Ljava/lang/String;"),
       reinterpret_cast<void*>(NoFdContext)},
      {const_cast<char*>("getPidContext"), const_cast<char*>("(I)Ljava/lang/String;"),
       reinterpret_cast<void*>(NoPidContext)},
      {const_cast<char*>("isSELinuxEnforced"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(IsEnforced)},
      {const_cast<char*>("isSELinuxEnabled"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(IsEnabled)},
      {const_cast<char*>("native_restorecon"), const_cast<char*>("(Ljava/lang/String;I)Z"),
       reinterpret_cast<void*>(Restorecon)},
      {const_cast<char*>("setFileContext"),
       const_cast<char*>("(Ljava/lang/String;Ljava/lang/String;)Z"),
       reinterpret_cast<void*>(SetFileContext)},
      {const_cast<char*>("setFSCreateContext"), const_cast<char*>("(Ljava/lang/String;)Z"),
       reinterpret_cast<void*>(SetFsCreateContext)},
      {const_cast<char*>("fileSelabelLookup"),
       const_cast<char*>("(Ljava/lang/String;)Ljava/lang/String;"),
       reinterpret_cast<void*>(NoPathContext)},
  };
  const bool registered =
      env->RegisterNatives(selinux, methods, sizeof(methods) / sizeof(methods[0])) == JNI_OK;
  env->DeleteLocalRef(selinux);
  return registered;
}
}  // namespace darwin_art::security
