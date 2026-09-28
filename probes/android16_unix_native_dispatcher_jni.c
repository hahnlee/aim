#include <errno.h>
#include <fcntl.h>
#include <jni.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

extern jint Java_sun_nio_fs_UnixNativeDispatcher_init(JNIEnv*, jclass);
extern jbyteArray Java_sun_nio_fs_UnixNativeDispatcher_getcwd(JNIEnv*, jclass);
extern jint Java_sun_nio_fs_UnixNativeDispatcher_open0(JNIEnv*, jclass, jlong, jint, jint);
extern void Java_sun_nio_fs_UnixNativeDispatcher_close(JNIEnv*, jclass, jint);
extern jint Java_sun_nio_fs_UnixNativeDispatcher_write(JNIEnv*, jclass, jint, jlong, jint);
extern jint Java_sun_nio_fs_UnixNativeDispatcher_read(JNIEnv*, jclass, jint, jlong, jint);
extern jint Java_sun_nio_fs_UnixNativeDispatcher_dup(JNIEnv*, jclass, jint);
extern jlong Java_sun_nio_fs_UnixNativeDispatcher_fopen0(JNIEnv*, jclass, jlong, jlong);
extern void Java_sun_nio_fs_UnixNativeDispatcher_fclose(JNIEnv*, jclass, jlong);
extern jint Java_sun_nio_fs_UnixNativeDispatcher_stat1(JNIEnv*, jclass, jlong);
extern void Java_sun_nio_fs_UnixNativeDispatcher_mkdir0(JNIEnv*, jclass, jlong, jint);
extern void Java_sun_nio_fs_UnixNativeDispatcher_rmdir0(JNIEnv*, jclass, jlong);
extern void Java_sun_nio_fs_UnixNativeDispatcher_rename0(JNIEnv*, jclass, jlong, jlong);
extern void Java_sun_nio_fs_UnixNativeDispatcher_link0(JNIEnv*, jclass, jlong, jlong);
extern void Java_sun_nio_fs_UnixNativeDispatcher_unlink0(JNIEnv*, jclass, jlong);
extern void Java_sun_nio_fs_UnixNativeDispatcher_symlink0(JNIEnv*, jclass, jlong, jlong);
extern jbyteArray Java_sun_nio_fs_UnixNativeDispatcher_readlink0(JNIEnv*, jclass, jlong);
extern jbyteArray Java_sun_nio_fs_UnixNativeDispatcher_realpath0(JNIEnv*, jclass, jlong);
extern jlong Java_sun_nio_fs_UnixNativeDispatcher_opendir0(JNIEnv*, jclass, jlong);
extern jbyteArray Java_sun_nio_fs_UnixNativeDispatcher_readdir(JNIEnv*, jclass, jlong);
extern void Java_sun_nio_fs_UnixNativeDispatcher_closedir(JNIEnv*, jclass, jlong);
extern void Java_sun_nio_fs_UnixNativeDispatcher_access0(JNIEnv*, jclass, jlong, jint);
extern jboolean Java_sun_nio_fs_UnixNativeDispatcher_exists0(JNIEnv*, jclass, jlong);
extern jlong Java_sun_nio_fs_UnixNativeDispatcher_pathconf0(JNIEnv*, jclass, jlong, jint);
extern jbyteArray Java_sun_nio_fs_UnixNativeDispatcher_strerror(JNIEnv*, jclass, jint);
extern jbyteArray Java_sun_nio_fs_UnixNativeDispatcher_getpwuid(JNIEnv*, jclass, jint);
extern jbyteArray Java_sun_nio_fs_UnixNativeDispatcher_getgrgid(JNIEnv*, jclass, jint);

static jlong Address(const void* pointer) {
  return (jlong)(intptr_t)pointer;
}

static jboolean HasBytes(JNIEnv* env, jbyteArray value) {
  return value != NULL && (*env)->GetArrayLength(env, value) > 0;
}

static jint Smoke_run(JNIEnv* env, jclass clazz, jstring root_string) {
  (void)clazz;
  const jint capabilities = Java_sun_nio_fs_UnixNativeDispatcher_init(env, NULL);
  if ((*env)->ExceptionCheck(env)) return -1;

  jbyteArray cwd = Java_sun_nio_fs_UnixNativeDispatcher_getcwd(env, NULL);
  if ((*env)->ExceptionCheck(env) || !HasBytes(env, cwd)) return -2;
  (*env)->DeleteLocalRef(env, cwd);

  const char* root = (*env)->GetStringUTFChars(env, root_string, NULL);
  if (root == NULL) return -3;
  char directory[PATH_MAX];
  char first[PATH_MAX];
  char second[PATH_MAX];
  char hardlink[PATH_MAX];
  char symlink_path[PATH_MAX];
  if (snprintf(directory, sizeof(directory), "%s/native", root) >= (int)sizeof(directory) ||
      snprintf(first, sizeof(first), "%s/a.txt", directory) >= (int)sizeof(first) ||
      snprintf(second, sizeof(second), "%s/b.txt", directory) >= (int)sizeof(second) ||
      snprintf(hardlink, sizeof(hardlink), "%s/hard.txt", directory) >= (int)sizeof(hardlink) ||
      snprintf(symlink_path, sizeof(symlink_path), "%s/link.txt", directory) >=
          (int)sizeof(symlink_path)) {
    (*env)->ReleaseStringUTFChars(env, root_string, root);
    return -4;
  }

  Java_sun_nio_fs_UnixNativeDispatcher_mkdir0(env, NULL, Address(directory), 0700);
  if ((*env)->ExceptionCheck(env)) goto fail;
  jint fd = Java_sun_nio_fs_UnixNativeDispatcher_open0(
      env, NULL, Address(first), O_CREAT | O_TRUNC | O_RDWR, 0600);
  if ((*env)->ExceptionCheck(env) || fd < 0) goto fail;
  const char payload[] = "aim";
  if (Java_sun_nio_fs_UnixNativeDispatcher_write(
          env, NULL, fd, Address(payload), (jint)(sizeof(payload) - 1)) !=
      (jint)(sizeof(payload) - 1)) goto fail_with_fd;
  Java_sun_nio_fs_UnixNativeDispatcher_close(env, NULL, fd);
  fd = -1;
  if ((*env)->ExceptionCheck(env) ||
      !Java_sun_nio_fs_UnixNativeDispatcher_exists0(env, NULL, Address(first)) ||
      Java_sun_nio_fs_UnixNativeDispatcher_stat1(env, NULL, Address(first)) == 0)
    goto fail;

  fd = Java_sun_nio_fs_UnixNativeDispatcher_open0(env, NULL, Address(first), O_RDONLY, 0);
  if ((*env)->ExceptionCheck(env) || fd < 0) goto fail;
  jint duplicate = Java_sun_nio_fs_UnixNativeDispatcher_dup(env, NULL, fd);
  if ((*env)->ExceptionCheck(env) || duplicate < 0) goto fail_with_fd;
  char readback[sizeof(payload)] = {0};
  if (Java_sun_nio_fs_UnixNativeDispatcher_read(
          env, NULL, duplicate, Address(readback), (jint)(sizeof(payload) - 1)) !=
          (jint)(sizeof(payload) - 1) ||
      memcmp(readback, payload, sizeof(payload) - 1) != 0) {
    Java_sun_nio_fs_UnixNativeDispatcher_close(env, NULL, duplicate);
    goto fail_with_fd;
  }
  Java_sun_nio_fs_UnixNativeDispatcher_close(env, NULL, duplicate);
  Java_sun_nio_fs_UnixNativeDispatcher_close(env, NULL, fd);
  fd = -1;
  if ((*env)->ExceptionCheck(env)) goto fail;

  const char read_mode[] = "r";
  jlong stream = Java_sun_nio_fs_UnixNativeDispatcher_fopen0(
      env, NULL, Address(first), Address(read_mode));
  if ((*env)->ExceptionCheck(env) || stream == 0) goto fail;
  Java_sun_nio_fs_UnixNativeDispatcher_fclose(env, NULL, stream);
  if ((*env)->ExceptionCheck(env)) goto fail;

  Java_sun_nio_fs_UnixNativeDispatcher_access0(env, NULL, Address(first), R_OK);
  if ((*env)->ExceptionCheck(env) ||
      Java_sun_nio_fs_UnixNativeDispatcher_pathconf0(
          env, NULL, Address(first), _PC_NAME_MAX) <= 0) goto fail;
  Java_sun_nio_fs_UnixNativeDispatcher_rename0(
      env, NULL, Address(first), Address(second));
  if ((*env)->ExceptionCheck(env)) goto fail;
  Java_sun_nio_fs_UnixNativeDispatcher_link0(
      env, NULL, Address(second), Address(hardlink));
  if ((*env)->ExceptionCheck(env)) goto fail;
  const char relative_target[] = "b.txt";
  Java_sun_nio_fs_UnixNativeDispatcher_symlink0(
      env, NULL, Address(relative_target), Address(symlink_path));
  if ((*env)->ExceptionCheck(env)) goto fail;
  jbyteArray target = Java_sun_nio_fs_UnixNativeDispatcher_readlink0(
      env, NULL, Address(symlink_path));
  if ((*env)->ExceptionCheck(env) || !HasBytes(env, target)) goto fail;
  (*env)->DeleteLocalRef(env, target);
  jbyteArray resolved = Java_sun_nio_fs_UnixNativeDispatcher_realpath0(
      env, NULL, Address(second));
  if ((*env)->ExceptionCheck(env) || !HasBytes(env, resolved)) goto fail;
  (*env)->DeleteLocalRef(env, resolved);

  jlong dir = Java_sun_nio_fs_UnixNativeDispatcher_opendir0(
      env, NULL, Address(directory));
  if ((*env)->ExceptionCheck(env) || dir == 0) goto fail;
  int entries = 0;
  for (;;) {
    jbyteArray entry = Java_sun_nio_fs_UnixNativeDispatcher_readdir(env, NULL, dir);
    if ((*env)->ExceptionCheck(env)) {
      Java_sun_nio_fs_UnixNativeDispatcher_closedir(env, NULL, dir);
      goto fail;
    }
    if (entry == NULL) break;
    ++entries;
    (*env)->DeleteLocalRef(env, entry);
  }
  Java_sun_nio_fs_UnixNativeDispatcher_closedir(env, NULL, dir);
  if ((*env)->ExceptionCheck(env) || entries < 5) goto fail;

  jbyteArray error = Java_sun_nio_fs_UnixNativeDispatcher_strerror(env, NULL, ENOENT);
  jbyteArray user = Java_sun_nio_fs_UnixNativeDispatcher_getpwuid(env, NULL, (jint)getuid());
  jbyteArray group = Java_sun_nio_fs_UnixNativeDispatcher_getgrgid(env, NULL, (jint)getgid());
  if ((*env)->ExceptionCheck(env) || !HasBytes(env, error) ||
      !HasBytes(env, user) || !HasBytes(env, group)) goto fail;
  (*env)->DeleteLocalRef(env, error);
  (*env)->DeleteLocalRef(env, user);
  (*env)->DeleteLocalRef(env, group);

  Java_sun_nio_fs_UnixNativeDispatcher_unlink0(env, NULL, Address(symlink_path));
  Java_sun_nio_fs_UnixNativeDispatcher_unlink0(env, NULL, Address(hardlink));
  Java_sun_nio_fs_UnixNativeDispatcher_unlink0(env, NULL, Address(second));
  Java_sun_nio_fs_UnixNativeDispatcher_rmdir0(env, NULL, Address(directory));
  if ((*env)->ExceptionCheck(env)) goto fail;
  (*env)->ReleaseStringUTFChars(env, root_string, root);
  return capabilities;

fail_with_fd:
  if (fd >= 0) Java_sun_nio_fs_UnixNativeDispatcher_close(env, NULL, fd);
fail:
  (*env)->ReleaseStringUTFChars(env, root_string, root);
  return -5;
}

static JNINativeMethod kMethods[] = {
    {"run", "(Ljava/lang/String;)I", (void*)Smoke_run},
};

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void* reserved) {
  (void)reserved;
  JNIEnv* env = NULL;
  if ((*vm)->GetEnv(vm, (void**)&env, JNI_VERSION_1_6) != JNI_OK) return JNI_ERR;
  jclass smoke = (*env)->FindClass(
      env, "dev/aim/probe/UnixNativeDispatcherDarwinSmoke");
  if (smoke == NULL ||
      (*env)->RegisterNatives(env, smoke, kMethods,
                             sizeof(kMethods) / sizeof(kMethods[0])) != JNI_OK)
    return JNI_ERR;
  return JNI_VERSION_1_6;
}
