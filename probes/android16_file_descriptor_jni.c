#include <fcntl.h>
#include <jni.h>
#include <sys/socket.h>
#include <unistd.h>

#if defined(AIM_FILE_DESCRIPTOR_DEVICE_CONTRACT)
#include <android/file_descriptor_jni.h>
#endif

extern jfieldID IO_fd_fdID;
extern void FileDescriptor_sync(JNIEnv* env, jobject file_descriptor);
extern jboolean FileDescriptor_isSocket(jint fd);
extern jboolean FileDescriptor_getAppend(jint fd);

#if defined(AIM_FILE_DESCRIPTOR_DEVICE_CONTRACT)
JNIEXPORT jint JNICALL aim_file_descriptor_device_contract(
    JNIEnv* env, jobject file_descriptor) {
  const jint descriptor = AFileDescriptor_getFd(env, file_descriptor);
  AFileDescriptor_setFd(env, file_descriptor, descriptor);
  return descriptor;
}
#endif

static jfieldID host_fd_id;

static jint GetHostFd(JNIEnv* env, jobject file_descriptor) {
  return (*env)->GetIntField(env, file_descriptor, host_fd_id);
}

static void Smoke_sync(JNIEnv* env, jclass clazz, jobject file_descriptor) {
  (void)clazz;
  // The product archive initializes this ID from Android's descriptor:I.
  // This host-JVM-only acceptance adapter points the same upstream function at
  // OpenJDK's fd:I so its managed exception and JVM_Sync path can be executed.
  IO_fd_fdID = host_fd_id;
  FileDescriptor_sync(env, file_descriptor);
}

static jboolean Smoke_getAppend(JNIEnv* env, jclass clazz,
                                jobject file_descriptor) {
  (void)clazz;
  return FileDescriptor_getAppend(GetHostFd(env, file_descriptor));
}

static jboolean Smoke_isSocket(JNIEnv* env, jclass clazz,
                               jobject file_descriptor) {
  (void)clazz;
  return FileDescriptor_isSocket(GetHostFd(env, file_descriptor));
}

static jboolean Smoke_socketPair(JNIEnv* env, jclass clazz) {
  (void)env;
  (void)clazz;
  int descriptors[2] = {-1, -1};
  if (socketpair(AF_UNIX, SOCK_STREAM, 0, descriptors) != 0) {
    return JNI_FALSE;
  }
  const jboolean result = FileDescriptor_isSocket(descriptors[0]);
  close(descriptors[0]);
  close(descriptors[1]);
  return result;
}

static JNINativeMethod kMethods[] = {
    {"sync", "(Ljava/io/FileDescriptor;)V", (void*)Smoke_sync},
    {"getAppend", "(Ljava/io/FileDescriptor;)Z", (void*)Smoke_getAppend},
    {"isSocket", "(Ljava/io/FileDescriptor;)Z", (void*)Smoke_isSocket},
    {"socketPair", "()Z", (void*)Smoke_socketPair},
};

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void* reserved) {
  (void)reserved;
  JNIEnv* env = NULL;
  if ((*vm)->GetEnv(vm, (void**)&env, JNI_VERSION_1_6) != JNI_OK) {
    return JNI_ERR;
  }
  jclass file_descriptor = (*env)->FindClass(env, "java/io/FileDescriptor");
  if (file_descriptor == NULL) {
    return JNI_ERR;
  }
  host_fd_id = (*env)->GetFieldID(env, file_descriptor, "fd", "I");
  if (host_fd_id == NULL) {
    return JNI_ERR;
  }
  jclass smoke = (*env)->FindClass(env, "dev/aim/probe/FileDescriptorDarwinSmoke");
  if (smoke == NULL ||
      (*env)->RegisterNatives(env, smoke, kMethods,
                             sizeof(kMethods) / sizeof(kMethods[0])) != JNI_OK) {
    return JNI_ERR;
  }
  return JNI_VERSION_1_6;
}
