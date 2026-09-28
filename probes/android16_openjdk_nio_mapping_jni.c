#include <stdint.h>
#include <signal.h>
#include <sys/uio.h>

#include <jni.h>
#include <nativehelper/JNIHelp.h>

extern void register_sun_nio_ch_IOUtil(JNIEnv* env);
extern void register_sun_nio_ch_NativeThread(JNIEnv* env);
extern int aim_install_sun_nio_ch_NativeThread_signal(void);
extern int aim_restore_sun_nio_ch_NativeThread_signal(void);

static volatile sig_atomic_t g_prior_sigio_count;

static void PriorSigioHandler(int signal_number) {
  (void)signal_number;
  ++g_prior_sigio_count;
}

extern jlong FileChannelImpl_initIDs(JNIEnv*, jclass);
extern jlong FileChannelImpl_map0(JNIEnv*, jobject, jint, jlong, jlong);
extern jint FileChannelImpl_unmap0(JNIEnv*, jobject, jlong, jlong);
extern jlong FileChannelImpl_position0(JNIEnv*, jobject, jobject, jlong);
extern jlong FileChannelImpl_transferTo0(JNIEnv*, jobject, jobject, jlong,
                                        jlong, jobject);

extern void FileDispatcherImpl_closeIntFD(JNIEnv*, jclass, jint);
extern void FileDispatcherImpl_preClose0(JNIEnv*, jclass, jobject);
extern void FileDispatcherImpl_close0(JNIEnv*, jclass, jobject);
extern void FileDispatcherImpl_release0(JNIEnv*, jobject, jobject, jlong,
                                        jlong);
extern jint FileDispatcherImpl_lock0(JNIEnv*, jobject, jobject, jboolean,
                                    jlong, jlong, jboolean);
extern jlong FileDispatcherImpl_size0(JNIEnv*, jobject, jobject);
extern jint FileDispatcherImpl_truncate0(JNIEnv*, jobject, jobject, jlong);
extern jint FileDispatcherImpl_force0(JNIEnv*, jobject, jobject, jboolean);
extern jlong FileDispatcherImpl_writev0(JNIEnv*, jclass, jobject, jlong, jint);
extern jint FileDispatcherImpl_pwrite0(JNIEnv*, jclass, jobject, jlong, jint,
                                      jlong);
extern jint FileDispatcherImpl_write0(JNIEnv*, jclass, jobject, jlong, jint);
extern jlong FileDispatcherImpl_readv0(JNIEnv*, jclass, jobject, jlong, jint);
extern jint FileDispatcherImpl_pread0(JNIEnv*, jclass, jobject, jlong, jint,
                                     jlong);
extern jint FileDispatcherImpl_read0(JNIEnv*, jclass, jobject, jlong, jint);

// Production NIO virtual-FD acceptance seam. The real runtime resolves these
// symbols to the Bionic filesystem facade; this small managed probe provider
// proves the production redirect selects one vector operation for a virtual
// descriptor without passing its token to Darwin's libc.
int aim_bionic_fs_owns_fd_core(int fd) {
  return fd == 0x30000001;
}

intptr_t aim_bionic_readv(int fd, const struct iovec* vectors, int count) {
  if (fd != 0x30000001 || vectors == NULL || count != 2) return -1;
  if (vectors[0].iov_len < 1 || vectors[1].iov_len < 2 ||
      vectors[0].iov_base == NULL || vectors[1].iov_base == NULL) return -1;
  ((char*)vectors[0].iov_base)[0] = 'v';
  ((char*)vectors[1].iov_base)[0] = 'f';
  ((char*)vectors[1].iov_base)[1] = 'd';
  return 3;
}

extern jlong FileDispatcherImpl_readv0(JNIEnv*, jclass, jobject, jlong, jint);
static jint VirtualReadv(JNIEnv* env, jclass clazz, jobject file_descriptor) {
  (void)clazz;
  struct iovec vectors[2];
  char first[1] = {0};
  char second[2] = {0, 0};
  vectors[0].iov_base = first;
  vectors[0].iov_len = sizeof(first);
  vectors[1].iov_base = second;
  vectors[1].iov_len = sizeof(second);
  const jlong result = FileDispatcherImpl_readv0(
      env, NULL, file_descriptor, (jlong)(uintptr_t)vectors, 2);
  if (result != 3 || first[0] != 'v' || second[0] != 'f' || second[1] != 'd') {
    return -1;
  }
  return 3;
}

#define METHOD(name, signature, function) \
  {(char*)(name), (char*)(signature), (void*)(function)}

static JNINativeMethod kFileChannelMethods[] = {
    METHOD("initIDs", "()J", FileChannelImpl_initIDs),
    METHOD("map0", "(IJJ)J", FileChannelImpl_map0),
    METHOD("unmap0", "(JJ)I", FileChannelImpl_unmap0),
    METHOD("position0", "(Ljava/io/FileDescriptor;J)J",
           FileChannelImpl_position0),
    METHOD("transferTo0",
           "(Ljava/io/FileDescriptor;JJLjava/io/FileDescriptor;)J",
           FileChannelImpl_transferTo0),
};

static JNINativeMethod kFileDispatcherMethods[] = {
    METHOD("closeIntFD", "(I)V", FileDispatcherImpl_closeIntFD),
    METHOD("preClose0", "(Ljava/io/FileDescriptor;)V",
           FileDispatcherImpl_preClose0),
    METHOD("close0", "(Ljava/io/FileDescriptor;)V",
           FileDispatcherImpl_close0),
    METHOD("release0", "(Ljava/io/FileDescriptor;JJ)V",
           FileDispatcherImpl_release0),
    METHOD("lock0", "(Ljava/io/FileDescriptor;ZJJZ)I",
           FileDispatcherImpl_lock0),
    METHOD("size0", "(Ljava/io/FileDescriptor;)J",
           FileDispatcherImpl_size0),
    METHOD("truncate0", "(Ljava/io/FileDescriptor;J)I",
           FileDispatcherImpl_truncate0),
    METHOD("force0", "(Ljava/io/FileDescriptor;Z)I",
           FileDispatcherImpl_force0),
    METHOD("writev0", "(Ljava/io/FileDescriptor;JI)J",
           FileDispatcherImpl_writev0),
    METHOD("pwrite0", "(Ljava/io/FileDescriptor;JIJ)I",
           FileDispatcherImpl_pwrite0),
    METHOD("write0", "(Ljava/io/FileDescriptor;JI)I",
           FileDispatcherImpl_write0),
    METHOD("readv0", "(Ljava/io/FileDescriptor;JI)J",
           FileDispatcherImpl_readv0),
    METHOD("pread0", "(Ljava/io/FileDescriptor;JIJ)I",
           FileDispatcherImpl_pread0),
    METHOD("read0", "(Ljava/io/FileDescriptor;JI)I",
           FileDispatcherImpl_read0),
};

static jint Peek(JNIEnv* env, jclass clazz, jlong address) {
  (void)env;
  (void)clazz;
  return *(const unsigned char*)(uintptr_t)address;
}

static jint RestoreSignalHandler(JNIEnv* env, jclass clazz) {
  (void)env;
  (void)clazz;
  if (aim_restore_sun_nio_ch_NativeThread_signal() != 0) {
    return -1;
  }
  g_prior_sigio_count = 0;
  if (raise(SIGIO) != 0) {
    return -2;
  }
  return g_prior_sigio_count == 1 ? 1 : 0;
}

static JNINativeMethod kSmokeMethods[] = {
    METHOD("peek", "(J)I", Peek),
    METHOD("restoreSignalHandler", "()I", RestoreSignalHandler),
    METHOD("virtualReadv", "(Ljava/io/FileDescriptor;)I", VirtualReadv),
};

static int Register(JNIEnv* env, const char* name, JNINativeMethod* methods,
                    jint count) {
  jclass clazz = (*env)->FindClass(env, name);
  if (clazz == NULL) {
    return JNI_ERR;
  }
  int result = (*env)->RegisterNatives(env, clazz, methods, count);
  (*env)->DeleteLocalRef(env, clazz);
  return result;
}

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void* reserved) {
  (void)reserved;
  JNIEnv* env = NULL;
  if ((*vm)->GetEnv(vm, (void**)&env, JNI_VERSION_1_6) != JNI_OK) {
    return JNI_ERR;
  }

  // The acceptance-only IOUtil object is source-identical except for the
  // host JDK FileDescriptor field spelling (fd instead of descriptor).
  register_sun_nio_ch_IOUtil(env);
  if ((*env)->ExceptionCheck(env)) {
    return JNI_ERR;
  }
  // Host JDK 17 retains these exact two methods, so exercise the production
  // registrar and its SIGIO handler initialization without an alias table.
  struct sigaction prior;
  prior.sa_handler = PriorSigioHandler;
  prior.sa_flags = 0;
  sigemptyset(&prior.sa_mask);
  if (sigaction(SIGIO, &prior, NULL) != 0) {
    return JNI_ERR;
  }
  if (aim_install_sun_nio_ch_NativeThread_signal() != 0) {
    return JNI_ERR;
  }
  register_sun_nio_ch_NativeThread(env);
  if ((*env)->ExceptionCheck(env)) {
    return JNI_ERR;
  }

  if (Register(env, "dev/aim/probe/NioFileChannel",
               kFileChannelMethods,
               sizeof(kFileChannelMethods) / sizeof(kFileChannelMethods[0])) !=
          JNI_OK ||
      Register(env, "dev/aim/probe/NioFileDispatcher",
               kFileDispatcherMethods,
               sizeof(kFileDispatcherMethods) /
                   sizeof(kFileDispatcherMethods[0])) != JNI_OK ||
      Register(env, "dev/aim/probe/OpenJdkNioMappingSmoke",
               kSmokeMethods,
               sizeof(kSmokeMethods) / sizeof(kSmokeMethods[0])) != JNI_OK) {
    return JNI_ERR;
  }

  jclass channel =
      (*env)->FindClass(env, "dev/aim/probe/NioFileChannel");
  if (channel == NULL) {
    return JNI_ERR;
  }
  FileChannelImpl_initIDs(env, channel);
  (*env)->DeleteLocalRef(env, channel);
  return (*env)->ExceptionCheck(env) ? JNI_ERR : JNI_VERSION_1_6;
}
