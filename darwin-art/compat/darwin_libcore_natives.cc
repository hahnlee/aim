#include "darwin_libcore_natives.h"
#include "darwin_android_time.h"
#include "darwin_framework_natives.h"
#include <iostream>
#include "darwin_libcore_filesystem_bridge.h"
#if defined(DARWIN_ART_FULL_LIBCORE_LINUX)
#include "AsynchronousCloseMonitor.h"
#include "../tools/bionic-socket-broker-adapter/include/darwin_art_bionic_socket_broker.h"
#include "../_aosp/libnativehelper-full/include/nativehelper/JNIHelp.h"
#include "../_aosp/libnativehelper-full/include/android/file_descriptor_jni.h"
#endif

extern "C" int darwin_art_bionic_fs_stat_core(
    const char*, DarwinArtAndroidStat*);
extern "C" int darwin_art_bionic_fs_mkdir_core(const char*, uint32_t);
extern "C" int darwin_art_bionic_fs_chmod_core(const char*, uint32_t);
extern "C" int32_t darwin_art_bionic_errno_load(void);
extern "C" int darwin_art_bionic_open(const char*, int, uint32_t);
extern "C" int darwin_art_bionic_socket_broker_dup(int);
extern "C" int darwin_art_bionic_socket_broker_fcntl(int, int, intptr_t);
extern "C" int darwin_art_bionic_close(int);
extern "C" intptr_t darwin_art_bionic_read(int, void*, size_t);
extern "C" intptr_t darwin_art_bionic_write(int, const void*, size_t);
extern "C" intptr_t darwin_art_bionic_pread(int, void*, size_t, int64_t);
extern "C" intptr_t darwin_art_bionic_pwrite(int, const void*, size_t, int64_t);
extern "C" int darwin_art_bionic_fstat(int, DarwinArtAndroidStat*);
extern "C" int darwin_art_bionic_ftruncate(int, int64_t);
extern "C" int darwin_art_bionic_stat(const char*, DarwinArtAndroidStat*);
extern "C" int64_t darwin_art_bionic_lseek(int, int64_t, int);
extern "C" int darwin_art_bionic_access(const char*, int);
extern "C" int darwin_art_bionic_remove(const char*);
extern "C" int darwin_art_bionic_rename(const char*, const char*);
extern "C" void* darwin_art_bionic_mmap64(void*, size_t, int, int, int, int64_t);
extern "C" int darwin_art_bionic_munmap(void*, size_t);
extern "C" int darwin_art_bionic_fs_adopt_host_fd_core(int);
extern "C" intptr_t darwin_art_bionic_sendfile(int, int, int64_t*, size_t);

#if defined(DARWIN_ART_FULL_LIBCORE_LINUX)
#include "darwin_os_constants.h"
#include "libcore_darwin_linux.h"

void register_libcore_io_AsynchronousCloseMonitor(JNIEnv* env);
extern "C" void register_java_io_UnixFileSystem(JNIEnv* env);
extern "C" void register_java_io_FileDescriptor(JNIEnv* env);
extern "C" void register_java_io_FileInputStream(JNIEnv* env);
extern "C" void register_java_lang_System(JNIEnv* env);
void register_jdk_internal_misc_VM(JNIEnv* env);
void register_java_lang_invoke_MethodHandle(JNIEnv* env);
void register_java_lang_invoke_VarHandle(JNIEnv* env);
extern "C" void register_java_lang_Runtime(JNIEnv* env);
extern "C" void register_java_sun_nio_fs_UnixNativeDispatcher(JNIEnv* env);
extern "C" void register_sun_nio_ch_IOUtil(JNIEnv* env);
extern "C" void register_sun_nio_ch_FileChannelImpl(JNIEnv* env);
extern "C" void register_sun_nio_ch_FileDispatcherImpl(JNIEnv* env);
extern "C" void register_sun_nio_ch_NativeThread(JNIEnv* env);
extern "C" int darwin_art_restore_sun_nio_ch_NativeThread_signal();
void register_libcore_io_Memory(JNIEnv* env);

class DarwinArtLibcoreJniConstants {
 public:
  static void Initialize(JNIEnv* env);
};
#endif

#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <pwd.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/utsname.h>
#include <unistd.h>
#include <zlib.h>

#include <cerrno>
#include <algorithm>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <iterator>
#include <poll.h>
#include <string>
#include <vector>

// java.util.zip.Deflater is loaded by the OpenJDK zip classes through the
// standard JNI symbol path (unlike Inflater, which is registered by OnLoad).
// Keep the state native and expose the AOSP entry points so apps do not fall
// back to a missing-symbol exception when they construct a GZIP stream.
struct DarwinDeflaterState {
  z_stream stream{};
};

extern "C" JNIEXPORT jlong Java_java_util_zip_Deflater_init(
    JNIEnv*, jclass, jint level, jint strategy, jboolean nowrap) {
  auto* state = new (std::nothrow) DarwinDeflaterState();
  if (state == nullptr) return 0;
  const int window_bits = nowrap == JNI_TRUE ? -MAX_WBITS : MAX_WBITS;
  if (deflateInit2(&state->stream, level, Z_DEFLATED, window_bits, MAX_MEM_LEVEL,
                   strategy) != Z_OK) {
    delete state;
    return 0;
  }
  return reinterpret_cast<jlong>(state);
}

extern "C" JNIEXPORT void Java_java_util_zip_Deflater_setDictionary(
    JNIEnv* env, jobject, jlong address, jbyteArray dictionary, jint offset,
    jint length) {
  auto* state = reinterpret_cast<DarwinDeflaterState*>(address);
  if (state == nullptr || dictionary == nullptr || offset < 0 || length < 0 ||
      offset > env->GetArrayLength(dictionary) ||
      length > env->GetArrayLength(dictionary) - offset) {
    return;
  }
  std::vector<jbyte> bytes(static_cast<std::size_t>(length));
  env->GetByteArrayRegion(dictionary, offset, length, bytes.data());
  if (!env->ExceptionCheck()) {
    deflateSetDictionary(&state->stream,
                         reinterpret_cast<const Bytef*>(bytes.data()),
                         static_cast<uInt>(bytes.size()));
  }
}

extern "C" JNIEXPORT void Java_java_util_zip_Deflater_setDictionaryBuffer(
    JNIEnv*, jobject, jlong address, jlong dictionary, jint length) {
  auto* state = reinterpret_cast<DarwinDeflaterState*>(address);
  if (state == nullptr || dictionary == 0 || length < 0) return;
  deflateSetDictionary(
      &state->stream,
      reinterpret_cast<const Bytef*>(static_cast<uintptr_t>(dictionary)),
      static_cast<uInt>(length));
}

extern "C" JNIEXPORT jint Java_java_util_zip_Deflater_deflateBytes(
    JNIEnv* env, jobject, jlong address, jbyteArray output, jint offset,
    jint length, jint flush) {
  auto* state = reinterpret_cast<DarwinDeflaterState*>(address);
  if (state == nullptr || output == nullptr || offset < 0 || length < 0 ||
      offset > env->GetArrayLength(output) ||
      length > env->GetArrayLength(output) - offset) {
    return 0;
  }
  std::vector<jbyte> bytes(static_cast<std::size_t>(length));
  state->stream.next_in = nullptr;
  state->stream.avail_in = 0;
  state->stream.next_out = reinterpret_cast<Bytef*>(bytes.data());
  state->stream.avail_out = static_cast<uInt>(bytes.size());
  const int result = deflate(&state->stream, flush);
  if (result != Z_OK && result != Z_STREAM_END && result != Z_BUF_ERROR) {
    return 0;
  }
  const jint produced = length - static_cast<jint>(state->stream.avail_out);
  if (produced > 0) {
    env->SetByteArrayRegion(output, offset, produced, bytes.data());
  }
  return produced;
}

extern "C" JNIEXPORT jlong Java_java_util_zip_Deflater_deflateBytesBytes(
    JNIEnv* env, jobject, jlong address, jbyteArray input, jint input_offset,
    jint input_length, jbyteArray output, jint output_offset,
    jint output_length, jint flush, jint) {
  auto* state = reinterpret_cast<DarwinDeflaterState*>(address);
  if (state == nullptr || input == nullptr || output == nullptr ||
      input_offset < 0 || input_length < 0 || output_offset < 0 ||
      output_length < 0 || input_offset > env->GetArrayLength(input) ||
      input_length > env->GetArrayLength(input) - input_offset ||
      output_offset > env->GetArrayLength(output) ||
      output_length > env->GetArrayLength(output) - output_offset) {
    return 0;
  }
  std::vector<jbyte> input_bytes(static_cast<std::size_t>(input_length));
  std::vector<jbyte> output_bytes(static_cast<std::size_t>(output_length));
  env->GetByteArrayRegion(input, input_offset, input_length, input_bytes.data());
  if (env->ExceptionCheck()) return 0;
  state->stream.next_in = reinterpret_cast<Bytef*>(input_bytes.data());
  state->stream.avail_in = static_cast<uInt>(input_bytes.size());
  state->stream.next_out = reinterpret_cast<Bytef*>(output_bytes.data());
  state->stream.avail_out = static_cast<uInt>(output_bytes.size());
  const int result = deflate(&state->stream, flush);
  if (result != Z_OK && result != Z_STREAM_END && result != Z_BUF_ERROR) return 0;
  const jint consumed = input_length - static_cast<jint>(state->stream.avail_in);
  const jint produced = output_length - static_cast<jint>(state->stream.avail_out);
  if (produced > 0) {
    env->SetByteArrayRegion(output, output_offset, produced, output_bytes.data());
  }
  const jlong finished = result == Z_STREAM_END ? (1LL << 62) : 0;
  return (static_cast<jlong>(consumed) & 0x7fffffffLL) |
         ((static_cast<jlong>(produced) & 0x7fffffffLL) << 31) | finished;
}

static jlong DeflaterDeflateBuffer(DarwinDeflaterState* state,
                                   const Bytef* input, jint input_length,
                                   Bytef* output, jint output_length,
                                   jint flush) {
  if (state == nullptr || input_length < 0 || output_length < 0 ||
      (input_length != 0 && input == nullptr) ||
      (output_length != 0 && output == nullptr)) {
    return 0;
  }
  state->stream.next_in = const_cast<Bytef*>(input);
  state->stream.avail_in = static_cast<uInt>(input_length);
  state->stream.next_out = output;
  state->stream.avail_out = static_cast<uInt>(output_length);
  const int result = deflate(&state->stream, flush);
  if (result != Z_OK && result != Z_STREAM_END && result != Z_BUF_ERROR) return 0;
  const jint consumed = input_length - static_cast<jint>(state->stream.avail_in);
  const jint produced = output_length - static_cast<jint>(state->stream.avail_out);
  const jlong finished = result == Z_STREAM_END ? (1LL << 62) : 0;
  return (static_cast<jlong>(consumed) & 0x7fffffffLL) |
         ((static_cast<jlong>(produced) & 0x7fffffffLL) << 31) | finished;
}

extern "C" JNIEXPORT jlong Java_java_util_zip_Deflater_deflateBytesBuffer(
    JNIEnv* env, jobject, jlong address, jbyteArray input, jint input_offset,
    jint input_length, jlong output, jint output_offset, jint output_length,
    jint flush) {
  auto* state = reinterpret_cast<DarwinDeflaterState*>(address);
  if (input == nullptr || input_offset < 0 || input_length < 0 || output_offset < 0 ||
      output_length < 0 || input_offset > env->GetArrayLength(input) ||
      input_length > env->GetArrayLength(input) - input_offset || output == 0) return 0;
  std::vector<jbyte> bytes(static_cast<std::size_t>(input_length));
  env->GetByteArrayRegion(input, input_offset, input_length, bytes.data());
  if (env->ExceptionCheck()) return 0;
  return DeflaterDeflateBuffer(
      state, reinterpret_cast<const Bytef*>(bytes.data()), input_length,
      reinterpret_cast<Bytef*>(static_cast<uintptr_t>(output) +
                               static_cast<uintptr_t>(output_offset)),
      output_length, flush);
}

extern "C" JNIEXPORT jlong Java_java_util_zip_Deflater_deflateBufferBytes(
    JNIEnv* env, jobject, jlong address, jlong input, jint input_offset,
    jint input_length, jbyteArray output, jint output_offset,
    jint output_length, jint flush) {
  auto* state = reinterpret_cast<DarwinDeflaterState*>(address);
  if (input == 0 || output == nullptr || input_offset < 0 || input_length < 0 ||
      output_offset < 0 || output_length < 0 ||
      output_offset > env->GetArrayLength(output) ||
      output_length > env->GetArrayLength(output) - output_offset) return 0;
  std::vector<jbyte> bytes(static_cast<std::size_t>(output_length));
  const jlong result = DeflaterDeflateBuffer(
      state,
      reinterpret_cast<const Bytef*>(static_cast<uintptr_t>(input) +
                                     static_cast<uintptr_t>(input_offset)),
      input_length, reinterpret_cast<Bytef*>(bytes.data()), output_length, flush);
  const jint produced = static_cast<jint>((result >> 31) & 0x7fffffffLL);
  if (produced > 0) env->SetByteArrayRegion(output, output_offset, produced, bytes.data());
  return result;
}

extern "C" JNIEXPORT jlong Java_java_util_zip_Deflater_deflateBufferBuffer(
    JNIEnv*, jobject, jlong address, jlong input, jint input_offset,
    jint input_length, jlong output, jint output_offset, jint output_length,
    jint flush) {
  if (input == 0 || output == 0) return 0;
  return DeflaterDeflateBuffer(
      reinterpret_cast<DarwinDeflaterState*>(address),
      reinterpret_cast<const Bytef*>(static_cast<uintptr_t>(input) +
                                     static_cast<uintptr_t>(input_offset)),
      input_length,
      reinterpret_cast<Bytef*>(static_cast<uintptr_t>(output) +
                               static_cast<uintptr_t>(output_offset)),
      output_length, flush);
}

extern "C" JNIEXPORT jint Java_java_util_zip_Deflater_getAdler(
    JNIEnv*, jobject, jlong address) {
  const auto* state = reinterpret_cast<const DarwinDeflaterState*>(address);
  return state == nullptr ? 0 : static_cast<jint>(state->stream.adler);
}

extern "C" JNIEXPORT void Java_java_util_zip_Deflater_reset(
    JNIEnv*, jobject, jlong address) {
  auto* state = reinterpret_cast<DarwinDeflaterState*>(address);
  if (state != nullptr) deflateReset(&state->stream);
}

extern "C" JNIEXPORT void Java_java_util_zip_Deflater_end(
    JNIEnv*, jobject, jlong address) {
  auto* state = reinterpret_cast<DarwinDeflaterState*>(address);
  if (state != nullptr) {
    deflateEnd(&state->stream);
    delete state;
  }
}

extern "C" JNIEXPORT jint Java_java_util_zip_CRC32_update(jint crc, jint value) {
  const Bytef byte = static_cast<Bytef>(value);
  return static_cast<jint>(crc32(static_cast<uLong>(crc), &byte, 1));
}

extern "C" JNIEXPORT jint Java_java_util_zip_CRC32_updateBytes0(
    JNIEnv* env, jclass, jint crc, jbyteArray bytes, jint offset, jint length) {
  if (bytes == nullptr || offset < 0 || length < 0 ||
      offset > env->GetArrayLength(bytes) ||
      length > env->GetArrayLength(bytes) - offset) {
    return crc;
  }
  std::vector<jbyte> data(static_cast<std::size_t>(length));
  env->GetByteArrayRegion(bytes, offset, length, data.data());
  if (env->ExceptionCheck()) return crc;
  return static_cast<jint>(crc32(
      static_cast<uLong>(crc), reinterpret_cast<const Bytef*>(data.data()),
      static_cast<uInt>(data.size())));
}

extern "C" JNIEXPORT jint Java_java_util_zip_CRC32_updateByteBuffer0(
    JNIEnv*, jclass, jint crc, jlong address, jint offset, jint length) {
  if (address == 0 || offset < 0 || length < 0) return crc;
  const auto* data = reinterpret_cast<const Bytef*>(
      static_cast<uintptr_t>(address) + static_cast<uintptr_t>(offset));
  return static_cast<jint>(crc32(static_cast<uLong>(crc), data,
                                 static_cast<uInt>(length)));
}

extern "C" JNIEXPORT jint Java_java_util_zip_Adler32_update(JNIEnv*, jclass,
                                                              jint adler, jint value) {
  const Bytef byte = static_cast<Bytef>(value);
  return static_cast<jint>(adler32(static_cast<uLong>(adler), &byte, 1));
}

extern "C" JNIEXPORT jint Java_java_util_zip_Adler32_updateBytes(
    JNIEnv* env, jclass, jint adler, jbyteArray bytes, jint offset, jint length) {
  if (bytes == nullptr || offset < 0 || length < 0 ||
      offset > env->GetArrayLength(bytes) ||
      length > env->GetArrayLength(bytes) - offset) {
    return adler;
  }
  std::vector<jbyte> data(static_cast<std::size_t>(length));
  env->GetByteArrayRegion(bytes, offset, length, data.data());
  if (env->ExceptionCheck()) return adler;
  return static_cast<jint>(adler32(
      static_cast<uLong>(adler), reinterpret_cast<const Bytef*>(data.data()),
      static_cast<uInt>(data.size())));
}

extern "C" JNIEXPORT jint Java_java_util_zip_Adler32_updateByteBuffer(
    JNIEnv*, jclass, jint adler, jlong address, jint offset, jint length) {
  if (address == 0 || offset < 0 || length < 0) return adler;
  const auto* data = reinterpret_cast<const Bytef*>(
      static_cast<uintptr_t>(address) + static_cast<uintptr_t>(offset));
  return static_cast<jint>(adler32(static_cast<uLong>(adler), data,
                                   static_cast<uInt>(length)));
}

// Android OpenJDK's ProcessEnvironment native is implemented by the Darwin
// libcore owner.  Keep the implementation in the Linux-compatibility TU, but
// register it here alongside the other OnLoad-owned OpenJDK classes so the
// linker retains the entry point and ART never falls back to name lookup.
extern "C" jobjectArray Java_java_lang_ProcessEnvironment_environ(
    JNIEnv* env, jclass clazz);

namespace {

bool Register(JNIEnv* env, const char* class_name,
              const JNINativeMethod* methods, jint method_count);

#if defined(DARWIN_ART_FULL_LIBCORE_LINUX)
// SocketInputStream/SocketOutputStream are OpenJDK classes, but their
// upstream native owner is not present in the small libopenjdk image used by
// DarwinART.  Keep these two methods here with the rest of the libcore owner,
// and use the central broker so FileDescriptor values remain guest handles.
constexpr int kAndroidEagain = 11;
constexpr int kAndroidEintr = 4;
constexpr int kAndroidEbadf = 9;
constexpr int kAndroidEpipe = 32;
constexpr int kAndroidEconnreset = 104;
constexpr int kAndroidEio = 5;
constexpr int kAndroidMsgDontWait = 0x40;

int SocketBrokerErrno() {
  const int error = darwin_art_bionic_errno_load();
  return error > 0 ? error : kAndroidEio;
}

enum class SocketWaitResult { kReady, kTimedOut, kInterrupted, kError };

SocketWaitResult WaitForSocket(int fd, short events,
                               bool finite_timeout,
                               std::chrono::steady_clock::time_point deadline,
                               int* error) {
  for (;;) {
    int timeout_ms = -1;
    if (finite_timeout) {
      const auto now = std::chrono::steady_clock::now();
      if (now >= deadline) {
        return SocketWaitResult::kTimedOut;
      }
      const auto remaining = std::chrono::duration_cast<std::chrono::milliseconds>(
          deadline - now);
      // poll() takes an integer millisecond timeout. Round up so a short,
      // non-zero Java timeout gets one genuine readiness attempt.
      timeout_ms = static_cast<int>(std::min<int64_t>(
          std::numeric_limits<int>::max(),
          std::max<int64_t>(1, remaining.count())));
    }
    DarwinArtBionicPollFd descriptor{fd, events, 0};
    AsynchronousCloseMonitor monitor(fd);
    const int result = darwin_art_bionic_socket_broker_poll(
        &descriptor, 1, timeout_ms);
    const int broker_error = SocketBrokerErrno();
    if (monitor.wasSignaled()) {
      if (error != nullptr) *error = kAndroidEintr;
      return SocketWaitResult::kInterrupted;
    }
    if (result > 0) {
      return SocketWaitResult::kReady;
    }
    if (result == 0) {
      return SocketWaitResult::kTimedOut;
    }
    if (broker_error == kAndroidEintr) {
      if (error != nullptr) *error = broker_error;
      return SocketWaitResult::kError;
    }
    if (error != nullptr) *error = broker_error;
    return SocketWaitResult::kError;
  }
}

void ThrowSocketClosed(JNIEnv* env) {
  jniThrowException(env, "java/net/SocketException", "Socket closed");
}

void ThrowSocketInterrupted(JNIEnv* env) {
  jniThrowException(env, "java/io/InterruptedIOException",
                    "Operation interrupted");
}

void ThrowSocketTimeout(JNIEnv* env) {
  jniThrowException(env, "java/net/SocketTimeoutException", "Read timed out");
}

void ThrowSocketFailure(JNIEnv* env, const char* operation, int error) {
  if (error == kAndroidEbadf) {
    ThrowSocketClosed(env);
  } else if (error == kAndroidEconnreset || error == kAndroidEpipe) {
    jniThrowException(env, "sun/net/ConnectionResetException",
                      "Connection reset");
  } else if (error == kAndroidEintr) {
    ThrowSocketInterrupted(env);
  } else {
    jniThrowException(env, "java/net/SocketException",
                      std::strcmp(operation, "socketWrite0") == 0
                          ? "Write failed"
                          : "Read failed");
  }
}

bool GetSocketByteArrayRange(JNIEnv* env, jbyteArray bytes, jint offset,
                             jint length, jbyte** elements) {
  if (bytes == nullptr) {
    jniThrowNullPointerException(env, "null byte array");
    return false;
  }
  if (offset < 0 || length < 0) {
    jniThrowException(env, "java/lang/ArrayIndexOutOfBoundsException",
                      "negative offset or length");
    return false;
  }
  const jsize array_length = env->GetArrayLength(bytes);
  if (offset > array_length || length > array_length - offset) {
    jniThrowException(env, "java/lang/ArrayIndexOutOfBoundsException",
                      "socket byte range exceeds array");
    return false;
  }
  if (length == 0) {
    *elements = nullptr;
    return true;
  }
  *elements = env->GetByteArrayElements(bytes, nullptr);
  return *elements != nullptr;
}

bool GetSocketFd(JNIEnv* env, jobject java_fd, int* fd) {
  if (java_fd == nullptr) {
    jniThrowNullPointerException(env, "null file descriptor");
    return false;
  }
  *fd = AFileDescriptor_getFd(env, java_fd);
  if (*fd < 0) {
    ThrowSocketClosed(env);
    return false;
  }
  return true;
}

jint SocketInputStreamRead0(JNIEnv* env, jobject, jobject java_fd,
                            jbyteArray bytes, jint offset, jint length,
                            jint timeout_ms) {
  jbyte* elements = nullptr;
  if (!GetSocketByteArrayRange(env, bytes, offset, length, &elements)) {
    return -1;
  }
  if (length == 0) return 0;

  int fd = -1;
  if (!GetSocketFd(env, java_fd, &fd)) {
    env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
    return -1;
  }
  const bool finite_timeout = timeout_ms > 0;
  const auto deadline = std::chrono::steady_clock::now() +
                        std::chrono::milliseconds(timeout_ms > 0 ? timeout_ms : 0);
  for (;;) {
    if (finite_timeout) {
      int error = 0;
      const SocketWaitResult wait = WaitForSocket(
          fd, POLLIN, true, deadline, &error);
      if (wait == SocketWaitResult::kTimedOut) {
        env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
        ThrowSocketTimeout(env);
        return -1;
      }
      if (wait == SocketWaitResult::kInterrupted) {
        env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
        ThrowSocketClosed(env);
        return -1;
      }
      if (wait == SocketWaitResult::kError) {
        env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
        ThrowSocketFailure(env, "socketRead0", error);
        return -1;
      }
    }

    AsynchronousCloseMonitor monitor(fd);
    const intptr_t result = darwin_art_bionic_socket_broker_recv(
        fd, elements + offset, static_cast<size_t>(length),
        finite_timeout ? kAndroidMsgDontWait : 0);
    const int error = SocketBrokerErrno();
    if (monitor.wasSignaled()) {
      env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
      ThrowSocketClosed(env);
      return -1;
    }
    if (result >= 0) {
      env->ReleaseByteArrayElements(bytes, elements, 0);
      return static_cast<jint>(result);
    }
    if (error == kAndroidEintr) {
      env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
      ThrowSocketInterrupted(env);
      return -1;
    }
    if (error == kAndroidEagain) {
      // A nonblocking socket can lose readiness between poll and recv. Keep
      // waiting against the same deadline instead of exposing a spurious
      // EAGAIN to java.net.
      if (!finite_timeout) {
        int wait_error = 0;
        const SocketWaitResult wait = WaitForSocket(
            fd, POLLIN, false, deadline, &wait_error);
        if (wait == SocketWaitResult::kReady) continue;
        if (wait == SocketWaitResult::kInterrupted) {
          env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
          ThrowSocketClosed(env);
          return -1;
        }
        if (wait == SocketWaitResult::kError) {
          env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
          ThrowSocketFailure(env, "socketRead0", wait_error);
          return -1;
        }
      }
      continue;
    }
    env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
    ThrowSocketFailure(env, "socketRead0", error);
    return -1;
  }
}

void SocketOutputStreamWrite0(JNIEnv* env, jobject, jobject java_fd,
                              jbyteArray bytes, jint offset, jint length) {
  jbyte* elements = nullptr;
  if (!GetSocketByteArrayRange(env, bytes, offset, length, &elements)) return;
  if (length == 0) return;

  int fd = -1;
  if (!GetSocketFd(env, java_fd, &fd)) {
    env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
    return;
  }
  size_t sent = 0;
  while (sent < static_cast<size_t>(length)) {
    AsynchronousCloseMonitor monitor(fd);
    const intptr_t result = darwin_art_bionic_socket_broker_send(
        fd, elements + offset + sent, static_cast<size_t>(length) - sent, 0);
    const int error = SocketBrokerErrno();
    if (monitor.wasSignaled()) {
      env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
      ThrowSocketClosed(env);
      return;
    }
    if (result > 0) {
      sent += static_cast<size_t>(result);
      continue;
    }
    if (result == 0) {
      env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
      ThrowSocketFailure(env, "socketWrite0", kAndroidEio);
      return;
    }
    if (error == kAndroidEintr) {
      env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
      ThrowSocketInterrupted(env);
      return;
    }
    if (error == kAndroidEagain) {
      int wait_error = 0;
      const SocketWaitResult wait = WaitForSocket(
          fd, POLLOUT, false, std::chrono::steady_clock::time_point::max(),
          &wait_error);
      if (wait == SocketWaitResult::kReady) continue;
      env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
      if (wait == SocketWaitResult::kInterrupted) {
        ThrowSocketClosed(env);
      } else {
        ThrowSocketFailure(env, "socketWrite0", wait_error);
      }
      return;
    }
    env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
    ThrowSocketFailure(env, "socketWrite0", error);
    return;
  }
  env->ReleaseByteArrayElements(bytes, elements, JNI_ABORT);
}
#endif

// java.util.zip.Inflater is part of the Android boot class path and is used
// while framework and APK resources are read.  Keep the Java-facing ABI
// identical to AOSP, but own the z_stream in this provider rather than
// forwarding a guest pointer to the host.  The packed result format is the
// Android contract: input-consumed in bits 0..30, output-produced in bits
// 31..61, finished in bit 62, and needs-dictionary in bit 63.
struct DarwinInflaterState {
  z_stream stream{};
};

jfieldID g_file_key_device = nullptr;
jfieldID g_file_key_inode = nullptr;

void FileKeyInitIds(JNIEnv* env, jclass file_key_class) {
  g_file_key_device = env->GetFieldID(file_key_class, "st_dev", "J");
  g_file_key_inode = env->GetFieldID(file_key_class, "st_ino", "J");
}

void FileKeyInit(JNIEnv* env, jobject file_key, jobject file_descriptor) {
  if (file_key == nullptr || file_descriptor == nullptr) return;
  jclass descriptor_class = env->GetObjectClass(file_descriptor);
  jfieldID descriptor_field = descriptor_class == nullptr
                                  ? nullptr
                                  : env->GetFieldID(descriptor_class,
                                                    "descriptor", "I");
  const jint descriptor = descriptor_field == nullptr
                              ? -1
                              : env->GetIntField(file_descriptor,
                                                 descriptor_field);
  if (descriptor_class != nullptr) env->DeleteLocalRef(descriptor_class);
  // FileDescriptor values in an APK process are Android facade descriptors,
  // not necessarily Darwin descriptors.  Using host fstat here makes the
  // broker's guest fd table look invalid and breaks FileLockTable.  Query the
  // same Android-shaped stat contract used by libcore.io.Linux instead.
  DarwinArtAndroidStat status{};
  if (descriptor < 0 || darwin_art_bionic_fstat(descriptor, &status) != 0) {
    jclass exception = env->FindClass("java/io/IOException");
    if (exception != nullptr) {
      env->ThrowNew(exception, "fstat failed while constructing FileKey");
      env->DeleteLocalRef(exception);
    }
    return;
  }
  if (g_file_key_device == nullptr || g_file_key_inode == nullptr) {
    jclass file_key_class = env->GetObjectClass(file_key);
    if (file_key_class != nullptr) {
      FileKeyInitIds(env, file_key_class);
      env->DeleteLocalRef(file_key_class);
    }
  }
  if (g_file_key_device != nullptr && g_file_key_inode != nullptr &&
      !env->ExceptionCheck()) {
    env->SetLongField(file_key, g_file_key_device,
                      static_cast<jlong>(status.st_dev));
    env->SetLongField(file_key, g_file_key_inode,
                      static_cast<jlong>(status.st_ino));
  }
}

jlong InflaterInit(JNIEnv*, jclass, jboolean nowrap) {
  auto* state = new DarwinInflaterState();
  const int window_bits = nowrap == JNI_TRUE ? -MAX_WBITS : MAX_WBITS;
  if (inflateInit2(&state->stream, window_bits) != Z_OK) {
    delete state;
    return 0;
  }
  return reinterpret_cast<jlong>(state);
}

void InflaterSetDictionary(JNIEnv* env, jclass, jlong address,
                           jbyteArray dictionary, jint offset, jint length) {
  auto* state = reinterpret_cast<DarwinInflaterState*>(address);
  if (state == nullptr || dictionary == nullptr || offset < 0 || length < 0 ||
      offset > env->GetArrayLength(dictionary) ||
      length > env->GetArrayLength(dictionary) - offset) {
    return;
  }
  std::vector<jbyte> bytes(static_cast<std::size_t>(length));
  env->GetByteArrayRegion(dictionary, offset, length, bytes.data());
  if (!env->ExceptionCheck()) {
    inflateSetDictionary(&state->stream,
                         reinterpret_cast<const Bytef*>(bytes.data()),
                         static_cast<uInt>(bytes.size()));
  }
}

jlong InflaterBytesBytes(JNIEnv* env, jobject, jlong address,
                          jbyteArray input, jint input_offset,
                          jint input_length, jbyteArray output,
                          jint output_offset, jint output_length) {
  auto* state = reinterpret_cast<DarwinInflaterState*>(address);
  if (state == nullptr || input == nullptr || output == nullptr ||
      input_offset < 0 || input_length < 0 || output_offset < 0 ||
      output_length < 0 ||
      input_offset > env->GetArrayLength(input) ||
      input_length > env->GetArrayLength(input) - input_offset ||
      output_offset > env->GetArrayLength(output) ||
      output_length > env->GetArrayLength(output) - output_offset) {
    return 0;
  }
  std::vector<jbyte> input_bytes(static_cast<std::size_t>(input_length));
  env->GetByteArrayRegion(input, input_offset, input_length,
                          input_bytes.data());
  if (env->ExceptionCheck()) {
    return 0;
  }
  std::vector<jbyte> output_bytes(static_cast<std::size_t>(output_length));
  state->stream.next_in = reinterpret_cast<Bytef*>(input_bytes.data());
  state->stream.avail_in = static_cast<uInt>(input_bytes.size());
  state->stream.next_out = reinterpret_cast<Bytef*>(output_bytes.data());
  state->stream.avail_out = static_cast<uInt>(output_bytes.size());
  const int result = inflate(&state->stream, Z_NO_FLUSH);
  const jint consumed = input_length - static_cast<jint>(state->stream.avail_in);
  const jint produced = output_length - static_cast<jint>(state->stream.avail_out);
  if (produced > 0) {
    env->SetByteArrayRegion(output, output_offset, produced,
                            output_bytes.data());
  }
  if (result == Z_NEED_DICT) {
    return (static_cast<jlong>(consumed) & 0x7fffffffLL) |
           ((static_cast<jlong>(produced) & 0x7fffffffLL) << 31) |
           (1LL << 63);
  }
  if (result == Z_DATA_ERROR || result == Z_STREAM_ERROR) {
    jclass exception = env->FindClass("java/util/zip/DataFormatException");
    if (exception != nullptr) {
      env->ThrowNew(exception, "invalid compressed data");
      env->DeleteLocalRef(exception);
    }
    return 0;
  }
  const jlong finished = result == Z_STREAM_END ? (1LL << 62) : 0;
  return (static_cast<jlong>(consumed) & 0x7fffffffLL) |
         ((static_cast<jlong>(produced) & 0x7fffffffLL) << 31) | finished;
}

jint InflaterGetAdler(JNIEnv*, jclass, jlong address) {
  const auto* state = reinterpret_cast<const DarwinInflaterState*>(address);
  return state == nullptr ? 0 : static_cast<jint>(state->stream.adler);
}

void InflaterReset(JNIEnv*, jclass, jlong address) {
  auto* state = reinterpret_cast<DarwinInflaterState*>(address);
  if (state != nullptr) {
    inflateReset(&state->stream);
  }
}

void InflaterEnd(JNIEnv*, jclass, jlong address) {
  auto* state = reinterpret_cast<DarwinInflaterState*>(address);
  if (state != nullptr) {
    inflateEnd(&state->stream);
    delete state;
  }
}

bool RegisterInflaterNatives(JNIEnv* env) {
  JNINativeMethod methods[] = {
      {const_cast<char*>("init"), const_cast<char*>("(Z)J"),
       reinterpret_cast<void*>(&InflaterInit)},
      {const_cast<char*>("setDictionary"), const_cast<char*>("(J[BII)V"),
       reinterpret_cast<void*>(&InflaterSetDictionary)},
      {const_cast<char*>("inflateBytesBytes"),
       const_cast<char*>("(J[BII[BII)J"),
       reinterpret_cast<void*>(&InflaterBytesBytes)},
      {const_cast<char*>("getAdler"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&InflaterGetAdler)},
      {const_cast<char*>("reset"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&InflaterReset)},
      {const_cast<char*>("end"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&InflaterEnd)},
  };
  return Register(env, "java/util/zip/Inflater", methods,
                  static_cast<jint>(std::size(methods)));
}

bool RegisterDeflaterNatives(JNIEnv* env) {
  // Android 16's java.util.zip.Deflater resolves init through the libcore
  // OnLoad table (signature: (IIZ)J).  The remaining entry points are kept as
  // exported JNI symbols for runtimes that use the legacy name lookup path;
  // registering only the ABI present in this boot class avoids rejecting the
  // whole class when an older/newer core-oj changes auxiliary signatures.
  JNINativeMethod methods[] = {
      {const_cast<char*>("init"), const_cast<char*>("(IIZ)J"),
       reinterpret_cast<void*>(&Java_java_util_zip_Deflater_init)},
      {const_cast<char*>("deflateBytesBytes"),
       const_cast<char*>("(J[BII[BIIII)J"),
       reinterpret_cast<void*>(&Java_java_util_zip_Deflater_deflateBytesBytes)},
      {const_cast<char*>("setDictionaryBuffer"), const_cast<char*>("(JJI)V"),
       reinterpret_cast<void*>(&Java_java_util_zip_Deflater_setDictionaryBuffer)},
      {const_cast<char*>("deflateBytesBuffer"),
       const_cast<char*>("(J[BIIJIII)J"),
       reinterpret_cast<void*>(&Java_java_util_zip_Deflater_deflateBytesBuffer)},
      {const_cast<char*>("deflateBufferBytes"),
       const_cast<char*>("(JJI[BIIII)J"),
       reinterpret_cast<void*>(&Java_java_util_zip_Deflater_deflateBufferBytes)},
      {const_cast<char*>("deflateBufferBuffer"),
       const_cast<char*>("(JJIJIII)J"),
       reinterpret_cast<void*>(&Java_java_util_zip_Deflater_deflateBufferBuffer)},
      {const_cast<char*>("end"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&Java_java_util_zip_Deflater_end)},
  };
  return Register(env, "java/util/zip/Deflater", methods,
                  static_cast<jint>(std::size(methods)));
}

bool RegisterChecksumNatives(JNIEnv* env) {
  JNINativeMethod crc_methods[] = {
      {const_cast<char*>("update"), const_cast<char*>("(II)I"),
       reinterpret_cast<void*>(&Java_java_util_zip_CRC32_update)},
      {const_cast<char*>("updateBytes0"), const_cast<char*>("(I[BII)I"),
       reinterpret_cast<void*>(&Java_java_util_zip_CRC32_updateBytes0)},
      {const_cast<char*>("updateByteBuffer0"), const_cast<char*>("(IJII)I"),
       reinterpret_cast<void*>(&Java_java_util_zip_CRC32_updateByteBuffer0)},
  };
  JNINativeMethod adler_methods[] = {
      {const_cast<char*>("update"), const_cast<char*>("(II)I"),
       reinterpret_cast<void*>(&Java_java_util_zip_Adler32_update)},
      {const_cast<char*>("updateBytes"), const_cast<char*>("(I[BII)I"),
       reinterpret_cast<void*>(&Java_java_util_zip_Adler32_updateBytes)},
      {const_cast<char*>("updateByteBuffer"), const_cast<char*>("(IJII)I"),
       reinterpret_cast<void*>(&Java_java_util_zip_Adler32_updateByteBuffer)},
  };
  return Register(env, "java/util/zip/CRC32", crc_methods,
                  static_cast<jint>(std::size(crc_methods))) &&
         Register(env, "java/util/zip/Adler32", adler_methods,
                  static_cast<jint>(std::size(adler_methods)));
}

#if !defined(DARWIN_ART_FULL_LIBCORE_LINUX)
void UnixFileSystemInitIds(JNIEnv*, jclass) {}

jint UnixFileSystemGetBooleanAttributes(JNIEnv* env, jobject, jstring path) {
  if (path == nullptr) {
    return 0;
  }
  const char* utf_path = env->GetStringUTFChars(path, nullptr);
  if (utf_path == nullptr) {
    return 0;
  }
  struct stat status {};
  const int result = stat(utf_path, &status);
  const char* basename = std::strrchr(utf_path, '/');
  basename = basename == nullptr ? utf_path : basename + 1;
  constexpr jint kExists = 0x01;
  constexpr jint kRegular = 0x02;
  constexpr jint kDirectory = 0x04;
  constexpr jint kHidden = 0x08;
  jint attributes = 0;
  if (result == 0) {
    attributes |= kExists;
    attributes |= S_ISREG(status.st_mode) ? kRegular : 0;
    attributes |= S_ISDIR(status.st_mode) ? kDirectory : 0;
    attributes |= basename[0] == '.' && basename[1] != '\0' ? kHidden : 0;
  }
  env->ReleaseStringUTFChars(path, utf_path);
  return attributes;
}
#endif

#if !defined(DARWIN_ART_FULL_LIBCORE_LINUX)
void OsConstantsInitConstants(JNIEnv* env, jclass klass) {
  jfieldID processors =
      env->GetStaticFieldID(klass, "_SC_NPROCESSORS_CONF", "I");
  if (processors != nullptr) {
    env->SetStaticIntField(klass, processors, _SC_NPROCESSORS_CONF);
  }
}
extern "C" JNIEXPORT void Java_android_system_OsConstants_initConstants(
    JNIEnv* env, jclass klass) {
  OsConstantsInitConstants(env, klass);
}
#endif

jlong LinuxSysconf(JNIEnv*, jobject, jint name) {
  return static_cast<jlong>(sysconf(name));
}

jobject LinuxStat(JNIEnv* env, jobject, jstring path) {
  if (path == nullptr) {
    return nullptr;
  }
  const char* utf_path = env->GetStringUTFChars(path, nullptr);
  if (utf_path == nullptr) {
    return nullptr;
  }
  struct stat status {};
  const int result = stat(utf_path, &status);
  env->ReleaseStringUTFChars(path, utf_path);
  if (result != 0) {
    jclass exception_class = env->FindClass("android/system/ErrnoException");
    jmethodID constructor =
        exception_class == nullptr
            ? nullptr
            : env->GetMethodID(exception_class, "<init>",
                               "(Ljava/lang/String;I)V");
    jstring function_name = env->NewStringUTF("stat");
    jobject exception =
        constructor == nullptr
            ? nullptr
            : env->NewObject(exception_class, constructor, function_name, errno);
    if (exception != nullptr) {
      env->Throw(static_cast<jthrowable>(exception));
    }
    env->DeleteLocalRef(exception);
    env->DeleteLocalRef(function_name);
    env->DeleteLocalRef(exception_class);
    return nullptr;
  }

  jclass stat_class = env->FindClass("android/system/StructStat");
  jmethodID constructor =
      stat_class == nullptr
          ? nullptr
          : env->GetMethodID(stat_class, "<init>", "(JJIJIIJJJJJJJ)V");
  jobject value =
      constructor == nullptr
          ? nullptr
          : env->NewObject(
                stat_class, constructor, static_cast<jlong>(status.st_dev),
                static_cast<jlong>(status.st_ino),
                static_cast<jint>(status.st_mode),
                static_cast<jlong>(status.st_nlink),
                static_cast<jint>(status.st_uid),
                static_cast<jint>(status.st_gid),
                static_cast<jlong>(status.st_rdev),
                static_cast<jlong>(status.st_size),
                static_cast<jlong>(status.st_atimespec.tv_sec),
                static_cast<jlong>(status.st_mtimespec.tv_sec),
                static_cast<jlong>(status.st_ctimespec.tv_sec),
                static_cast<jlong>(status.st_blksize),
                static_cast<jlong>(status.st_blocks));
  env->DeleteLocalRef(stat_class);
  return value;
}

jstring LinuxGetenv(JNIEnv* env, jobject, jstring name) {
  if (name == nullptr) {
    return nullptr;
  }
  const char* utf_name = env->GetStringUTFChars(name, nullptr);
  if (utf_name == nullptr) {
    return nullptr;
  }
  const char* value = std::getenv(utf_name);
  env->ReleaseStringUTFChars(name, utf_name);
  return value == nullptr ? nullptr : env->NewStringUTF(value);
}

jint LinuxNativeGettid() {
  std::uint64_t tid = 0;
  return pthread_threadid_np(nullptr, &tid) == 0 ? static_cast<jint>(tid) : -1;
}

jint LinuxNativeGetuid() { return static_cast<jint>(getuid()); }

jobject LinuxGetpwuid(JNIEnv* env, jobject, jint uid) {
  long configured_size = sysconf(_SC_GETPW_R_SIZE_MAX);
  std::size_t buffer_size =
      configured_size > 0 ? static_cast<std::size_t>(configured_size) : 16384;
  std::vector<char> buffer(buffer_size);
  passwd entry{};
  passwd* result = nullptr;
  if (getpwuid_r(static_cast<uid_t>(uid), &entry, buffer.data(), buffer.size(),
                 &result) != 0 ||
      result == nullptr) {
    return nullptr;
  }

  jclass struct_passwd = env->FindClass("android/system/StructPasswd");
  if (struct_passwd == nullptr) {
    return nullptr;
  }
  jmethodID constructor = env->GetMethodID(
      struct_passwd, "<init>",
      "(Ljava/lang/String;IILjava/lang/String;Ljava/lang/String;)V");
  if (constructor == nullptr) {
    env->DeleteLocalRef(struct_passwd);
    return nullptr;
  }
  jstring name =
      env->NewStringUTF(entry.pw_name == nullptr ? "" : entry.pw_name);
  jstring directory =
      env->NewStringUTF(entry.pw_dir == nullptr ? "" : entry.pw_dir);
  jstring shell =
      env->NewStringUTF(entry.pw_shell == nullptr ? "" : entry.pw_shell);
  jobject value = env->NewObject(
      struct_passwd, constructor, name, static_cast<jint>(entry.pw_uid),
      static_cast<jint>(entry.pw_gid), directory, shell);
  env->DeleteLocalRef(name);
  env->DeleteLocalRef(directory);
  env->DeleteLocalRef(shell);
  env->DeleteLocalRef(struct_passwd);
  return value;
}

jobject LinuxUname(JNIEnv* env, jobject) {
  utsname host{};
  if (uname(&host) != 0) {
    return nullptr;
  }
  jclass struct_utsname = env->FindClass("android/system/StructUtsname");
  if (struct_utsname == nullptr) {
    return nullptr;
  }
  jmethodID constructor =
      env->GetMethodID(struct_utsname, "<init>",
                       "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/"
                       "String;Ljava/lang/String;Ljava/lang/String;)V");
  if (constructor == nullptr) {
    env->DeleteLocalRef(struct_utsname);
    return nullptr;
  }
  // Preserve Android-visible kernel identity while retaining useful host build
  // details in the fields that do not affect ABI selection.
  jstring sysname = env->NewStringUTF("Linux");
  jstring nodename = env->NewStringUTF(host.nodename);
  jstring release = env->NewStringUTF(host.release);
  jstring version = env->NewStringUTF(host.version);
  jstring machine = env->NewStringUTF("aarch64");
  jobject value = env->NewObject(struct_utsname, constructor, sysname, nodename,
                                 release, version, machine);
  env->DeleteLocalRef(sysname);
  env->DeleteLocalRef(nodename);
  env->DeleteLocalRef(release);
  env->DeleteLocalRef(version);
  env->DeleteLocalRef(machine);
  env->DeleteLocalRef(struct_utsname);
  return value;
}

jint LinuxWriteBytes(JNIEnv* env, jobject, jobject file_descriptor,
                     jobject buffer, jint offset, jint byte_count) {
  if (file_descriptor == nullptr || buffer == nullptr || offset < 0 ||
      byte_count < 0) {
    return -1;
  }
  jclass descriptor_class = env->GetObjectClass(file_descriptor);
  jfieldID descriptor_field =
      env->GetFieldID(descriptor_class, "descriptor", "I");
  const jint descriptor =
      descriptor_field == nullptr
          ? -1
          : env->GetIntField(file_descriptor, descriptor_field);
  env->DeleteLocalRef(descriptor_class);
  if (descriptor < 0) {
    return -1;
  }

  const void* bytes = nullptr;
  jbyteArray byte_array = nullptr;
  jbyte* array_elements = nullptr;
  jclass byte_array_class = env->FindClass("[B");
  if (byte_array_class != nullptr &&
      env->IsInstanceOf(buffer, byte_array_class)) {
    byte_array = reinterpret_cast<jbyteArray>(buffer);
    const jsize length = env->GetArrayLength(byte_array);
    if (offset > length || byte_count > length - offset) {
      env->DeleteLocalRef(byte_array_class);
      return -1;
    }
    array_elements = env->GetByteArrayElements(byte_array, nullptr);
    bytes = array_elements == nullptr ? nullptr : array_elements + offset;
  } else {
    void* direct = env->GetDirectBufferAddress(buffer);
    const jlong capacity = env->GetDirectBufferCapacity(buffer);
    if (direct != nullptr && offset <= capacity &&
        byte_count <= capacity - offset) {
      bytes = static_cast<const std::byte*>(direct) + offset;
    }
  }
  env->DeleteLocalRef(byte_array_class);
  if (bytes == nullptr) {
    return -1;
  }

  ssize_t written;
  do {
    written = write(descriptor, bytes, static_cast<std::size_t>(byte_count));
  } while (written < 0 && errno == EINTR);
  if (array_elements != nullptr) {
    env->ReleaseByteArrayElements(byte_array, array_elements, JNI_ABORT);
  }
  return written < 0 ? -1 : static_cast<jint>(written);
}

jobjectArray SystemSpecialProperties(JNIEnv* env, jclass) {
  jclass string_class = env->FindClass("java/lang/String");
  if (string_class == nullptr) {
    return nullptr;
  }
  jobjectArray properties = env->NewObjectArray(4, string_class, nullptr);
  env->DeleteLocalRef(string_class);
  if (properties == nullptr) {
    return nullptr;
  }

  char current_directory[PATH_MAX];
  const char* directory = getcwd(current_directory, sizeof(current_directory));
  const char* library_path = std::getenv("DARWIN_ART_JAVA_LIBRARY_PATH");
  if (library_path == nullptr) {
    library_path = std::getenv("DYLD_LIBRARY_PATH");
  }
  const std::string values[] = {
      std::string("user.dir=") + (directory == nullptr ? "/" : directory),
      std::string("android.zlib.version=") + ZLIB_VERSION,
      "android.openssl.version=Darwin Security.framework",
      std::string("java.library.path=") +
          (library_path == nullptr ? "" : library_path),
  };
  for (jsize index = 0; index < 4; ++index) {
    jstring value = env->NewStringUTF(values[index].c_str());
    if (value == nullptr) {
      return nullptr;
    }
    env->SetObjectArrayElement(properties, index, value);
    env->DeleteLocalRef(value);
    if (env->ExceptionCheck()) {
      return nullptr;
    }
  }
  return properties;
}

jlong SystemCurrentTimeMillis(JNIEnv*, jclass) {
  return std::chrono::duration_cast<std::chrono::milliseconds>(
             std::chrono::system_clock::now().time_since_epoch())
      .count();
}

void SystemLog(JNIEnv* env, jclass, jchar, jstring message, jthrowable) {
  if (env == nullptr || message == nullptr) return;
  const char* text = env->GetStringUTFChars(message, nullptr);
  if (text != nullptr) {
    std::fprintf(stderr, "%s\n", text);
    env->ReleaseStringUTFChars(message, text);
  }
}

extern "C" void Java_java_lang_System_log(JNIEnv* env, jclass klass, jchar type,
                                             jstring message, jthrowable exception) {
  SystemLog(env, klass, type, message, exception);
}

jlong SystemNanoTime(JNIEnv*, jclass) {
  return darwin_art::AndroidUptimeNanos();
}

#if !defined(DARWIN_ART_FULL_LIBCORE_LINUX)
jboolean FileDescriptorGetAppend(jint descriptor) {
  const int flags = fcntl(descriptor, F_GETFL);
  return flags >= 0 && (flags & O_APPEND) != 0 ? JNI_TRUE : JNI_FALSE;
}

jboolean FileDescriptorIsSocket(jint descriptor) {
  int socket_type = 0;
  socklen_t length = sizeof(socket_type);
  return getsockopt(descriptor, SOL_SOCKET, SO_TYPE, &socket_type, &length) == 0
             ? JNI_TRUE
             : JNI_FALSE;
}
#endif

bool Register(JNIEnv* env, const char* class_name,
              const JNINativeMethod* methods, jint method_count) {
  jclass klass = env->FindClass(class_name);
  if (klass == nullptr) {
    return false;
  }
  const bool registered =
      env->RegisterNatives(klass, methods, method_count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return registered;
}

// The ART runtime owns the complete sun.misc.Unsafe table, but a detached
// libcore bootstrap can load its core-oj Unsafe class after the runtime's
// early registrar ran. Keep this narrow registration at the libcore boundary
// as well. AllocObject is the ART JNI primitive used by AOSP's implementation;
// it deliberately skips constructors while retaining normal VM checks for
// null, interfaces, and abstract classes.
jobject UnsafeAllocateInstance(JNIEnv* env, jobject, jclass clazz) {
  return env->AllocObject(clazz);
}

bool RegisterUnsafeAllocateInstanceNatives(JNIEnv* env) {
  const JNINativeMethod methods[] = {
      {const_cast<char*>("allocateInstance"),
       const_cast<char*>("(Ljava/lang/Class;)Ljava/lang/Object;"),
       reinterpret_cast<void*>(&UnsafeAllocateInstance)},
  };
  return Register(env, "sun/misc/Unsafe", methods,
                  static_cast<jint>(std::size(methods)));
}

// java.io.ObjectStreamClass owns this native because the default
// serialVersionUID calculation must include a class initializer when the
// classfile actually defines <clinit>.  Android's implementation uses the
// JNI method lookup itself (rather than a guessed reflection result), which
// also preserves the target-SDK compatibility behavior for inherited
// initializers.
jclass gObjectStreamNoSuchMethodError = nullptr;

jboolean ObjectStreamClassHasStaticInitializer(JNIEnv* env, jclass,
                                               jclass clazz,
                                               jboolean check_superclass) {
  jmethodID clinit = env->GetStaticMethodID(clazz, "<clinit>", "()V");
  if (clinit == nullptr) {
    // GetStaticMethodID reports the ordinary absence case as
    // NoSuchMethodError.  That is a normal false result here; all other
    // lookup failures must remain visible to the Java caller.
    jthrowable pending = env->ExceptionOccurred();
    env->ExceptionClear();
    const bool is_missing =
        pending != nullptr && gObjectStreamNoSuchMethodError != nullptr &&
        env->IsInstanceOf(pending, gObjectStreamNoSuchMethodError) == JNI_TRUE;
    if (!is_missing && pending != nullptr) {
      env->Throw(pending);
    }
    if (pending != nullptr) env->DeleteLocalRef(pending);
    return JNI_FALSE;
  }

  if (check_superclass == JNI_FALSE) return JNI_TRUE;

  jclass superclass = env->GetSuperclass(clazz);
  if (superclass == nullptr) return JNI_TRUE;
  jmethodID superclass_clinit =
      env->GetStaticMethodID(superclass, "<clinit>", "()V");
  env->DeleteLocalRef(superclass);
  if (superclass_clinit == nullptr) {
    // A superclass without <clinit> is expected and means the child lookup
    // was the defining initializer.  Preserve any unrelated JNI exception.
    jthrowable pending = env->ExceptionOccurred();
    env->ExceptionClear();
    const bool is_missing =
        pending != nullptr && gObjectStreamNoSuchMethodError != nullptr &&
        env->IsInstanceOf(pending, gObjectStreamNoSuchMethodError) == JNI_TRUE;
    if (!is_missing && pending != nullptr) env->Throw(pending);
    if (pending != nullptr) env->DeleteLocalRef(pending);
    return JNI_TRUE;
  }
  // Android intentionally compares method IDs here.  When the VM resolves
  // the same inherited <clinit>, the IDs match and the class contributes no
  // class-defined initializer to its SUID signature.
  return clinit != superclass_clinit ? JNI_TRUE : JNI_FALSE;
}

bool RegisterObjectStreamClassNatives(JNIEnv* env) {
  jclass object_stream_class = env->FindClass("java/io/ObjectStreamClass");
  if (object_stream_class == nullptr) return false;
  jclass no_such_method_error = env->FindClass("java/lang/NoSuchMethodError");
  if (no_such_method_error == nullptr) {
    env->DeleteLocalRef(object_stream_class);
    return false;
  }
  gObjectStreamNoSuchMethodError = reinterpret_cast<jclass>(
      env->NewGlobalRef(no_such_method_error));
  env->DeleteLocalRef(no_such_method_error);
  if (gObjectStreamNoSuchMethodError == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(object_stream_class);
    return false;
  }
  const JNINativeMethod methods[] = {
      {const_cast<char*>("hasStaticInitializer"),
       const_cast<char*>("(Ljava/lang/Class;Z)Z"),
       reinterpret_cast<void*>(&ObjectStreamClassHasStaticInitializer)},
  };
  const bool registered =
      env->RegisterNatives(object_stream_class, methods,
                           static_cast<jint>(std::size(methods))) == JNI_OK;
  env->DeleteLocalRef(object_stream_class);
  return registered && !env->ExceptionCheck();
}

}  // namespace

void register_libcore_math_NativeBN(JNIEnv* env);
void register_libcore_icu_ICU(JNIEnv* env);

namespace darwin_art {

bool RegisterEarlySystemLog(JNIEnv* env) {
  if (env == nullptr) return false;
  jclass klass = env->FindClass("java/lang/System");
  if (klass == nullptr) return false;
  const JNINativeMethod method = {
      const_cast<char*>("log"),
      const_cast<char*>("(CLjava/lang/String;Ljava/lang/Throwable;)V"),
      reinterpret_cast<void*>(&SystemLog),
  };
  const bool ok = env->RegisterNatives(klass, &method, 1) == JNI_OK;
  env->DeleteLocalRef(klass);
  return ok && !env->ExceptionCheck();
}

}  // namespace darwin_art

namespace darwin_art {

extern "C" void register_java_lang_UNIXProcess(JNIEnv* env);
extern "C" void register_java_lang_StrictMath(JNIEnv* env);

bool RegisterLibcoreNatives(JNIEnv* env) {
  // The standalone managed smoke intentionally uses host filesystem paths.
  // A real Android process installs its capability-scoped guest provider
  // before java.io.UnixFileSystem is registered, making silent host fallback
  // impossible for /data and /system paths.
  darwin_art_libcore_install_filesystem_provider(
      &darwin_art_bionic_fs_stat_core, &darwin_art_bionic_fs_mkdir_core,
      &darwin_art_bionic_fs_chmod_core, &darwin_art_bionic_errno_load);
#if defined(DARWIN_ART_FULL_LIBCORE_LINUX)
  using namespace darwin_art::libcore_darwin;
  InstallLinuxSyscallProviders(
      kLinuxSyscallProviderAbiVersion, sizeof(LinuxSyscallProviders),
      {
          .open = &darwin_art_bionic_open,
          .dup = &darwin_art_bionic_socket_broker_dup,
          .fcntl = &darwin_art_bionic_socket_broker_fcntl,
          .close = &darwin_art_bionic_socket_broker_close,
          .read = &darwin_art_bionic_read,
          .write = &darwin_art_bionic_write,
          .pread = &darwin_art_bionic_pread,
          .pwrite = &darwin_art_bionic_pwrite,
          .fstat = &darwin_art_bionic_fstat,
          .ftruncate = &darwin_art_bionic_ftruncate,
          .stat = &darwin_art_bionic_stat,
          .lseek = &darwin_art_bionic_lseek,
          .sendfile = &darwin_art_bionic_sendfile,
          .access = &darwin_art_bionic_access,
          .remove = &darwin_art_bionic_remove,
          .rename = &darwin_art_bionic_rename,
          .adopt_host_fd = &darwin_art_bionic_fs_adopt_host_fd_core,
          .load_errno = &darwin_art_bionic_errno_load,
          .mmap = &darwin_art_bionic_mmap64,
          .munmap = &darwin_art_bionic_munmap,
      });
#endif
  // NativeAllocationRegistry is part of libcore's boot class path rather than
  // framework.jar. Register its free-function ABI from the libcore owner so
  // Cleaner can release RippleShader/VectorDrawable allocations.
  if (!darwin_art::RegisterFrameworkSupportNatives(env)) {
    return false;
  }
  if (!RegisterUnsafeAllocateInstanceNatives(env)) {
    return false;
  }
  if (!RegisterObjectStreamClassNatives(env)) {
    return false;
  }
  if (!RegisterInflaterNatives(env)) {
    return false;
  }
  if (!RegisterDeflaterNatives(env)) {
    return false;
  }
  if (!RegisterChecksumNatives(env)) {
    return false;
  }
  ::register_libcore_math_NativeBN(env);
  if (env->ExceptionCheck()) {
    return false;
  }
  ::register_libcore_icu_ICU(env);
  if (env->ExceptionCheck()) {
    return false;
  }
  register_java_lang_StrictMath(env);
  if (env->ExceptionCheck()) {
    return false;
  }
#if !defined(DARWIN_ART_FULL_LIBCORE_LINUX)
  JNINativeMethod unix_file_system_methods[] = {
      {const_cast<char*>("initIDs"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&UnixFileSystemInitIds)},
      {const_cast<char*>("getBooleanAttributes0"),
       const_cast<char*>("(Ljava/lang/String;)I"),
       reinterpret_cast<void*>(&UnixFileSystemGetBooleanAttributes)},
  };
  if (!Register(env, "java/io/UnixFileSystem", unix_file_system_methods,
                static_cast<jint>(std::size(unix_file_system_methods)))) {
    return false;
  }
#else
  register_java_io_UnixFileSystem(env);
  if (env->ExceptionCheck()) {
    return false;
  }
#endif
  register_java_lang_UNIXProcess(env);
  if (env->ExceptionCheck()) {
    return false;
  }
#if !defined(DARWIN_ART_FULL_LIBCORE_LINUX)
  JNINativeMethod os_constants_methods[] = {
      {const_cast<char*>("initConstants"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&OsConstantsInitConstants)},
  };
#endif
#if !defined(DARWIN_ART_FULL_LIBCORE_LINUX)
  JNINativeMethod linux_methods[] = {
      {const_cast<char*>("getenv"),
       const_cast<char*>("(Ljava/lang/String;)Ljava/lang/String;"),
       reinterpret_cast<void*>(&LinuxGetenv)},
      {const_cast<char*>("getpwuid"),
       const_cast<char*>("(I)Landroid/system/StructPasswd;"),
       reinterpret_cast<void*>(&LinuxGetpwuid)},
      {const_cast<char*>("nativeGettid"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&LinuxNativeGettid)},
      {const_cast<char*>("nativeGetuid"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&LinuxNativeGetuid)},
      {const_cast<char*>("sysconf"), const_cast<char*>("(I)J"),
       reinterpret_cast<void*>(&LinuxSysconf)},
      {const_cast<char*>("stat"),
       const_cast<char*>("(Ljava/lang/String;)Landroid/system/StructStat;"),
       reinterpret_cast<void*>(&LinuxStat)},
      {const_cast<char*>("uname"),
       const_cast<char*>("()Landroid/system/StructUtsname;"),
       reinterpret_cast<void*>(&LinuxUname)},
      {const_cast<char*>("writeBytes"),
       const_cast<char*>("(Ljava/io/FileDescriptor;Ljava/lang/Object;II)I"),
       reinterpret_cast<void*>(&LinuxWriteBytes)},
  };
#endif
#if !defined(DARWIN_ART_FULL_LIBCORE_LINUX)
  JNINativeMethod system_methods[] = {
      {const_cast<char*>("currentTimeMillis"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&SystemCurrentTimeMillis)},
      {const_cast<char*>("nanoTime"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&SystemNanoTime)},
      {const_cast<char*>("specialProperties"),
       const_cast<char*>("()[Ljava/lang/String;"),
       reinterpret_cast<void*>(&SystemSpecialProperties)},
      {const_cast<char*>("log"),
       const_cast<char*>("(CLjava/lang/String;Ljava/lang/Throwable;)V"),
       reinterpret_cast<void*>(&SystemLog)},
  };
#endif
#if !defined(DARWIN_ART_FULL_LIBCORE_LINUX)
  JNINativeMethod file_descriptor_methods[] = {
      {const_cast<char*>("getAppend"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&FileDescriptorGetAppend)},
      {const_cast<char*>("isSocket"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&FileDescriptorIsSocket)},
  };
#endif
  const auto register_linux = [&]() {
#if defined(DARWIN_ART_FULL_LIBCORE_LINUX)
    // Android's Register.cpp initializes the asynchronous-close signal owner
    // immediately before libcore.io.Linux. Keep that order so every supported
    // blocking Darwin syscall can install a live monitor before Java closes it.
    register_libcore_io_AsynchronousCloseMonitor(env);
    if (env->ExceptionCheck()) {
      return false;
    }
    return libcore_darwin::RegisterLinuxNatives(env);
#else
    return Register(env, "libcore/io/Linux", linux_methods,
                    static_cast<jint>(std::size(linux_methods)));
#endif
  };
  const auto register_system = [&]() {
#if defined(DARWIN_ART_FULL_LIBCORE_LINUX)
    register_java_lang_System(env);
    if (env->ExceptionCheck()) {
      return false;
    }
    register_jdk_internal_misc_VM(env);
    register_java_lang_invoke_MethodHandle(env);
    register_java_lang_invoke_VarHandle(env);
    return !env->ExceptionCheck();
#else
    return Register(env, "java/lang/System", system_methods, 3);
#endif
  };
  const auto register_os_constants = [&]() {
#if defined(DARWIN_ART_FULL_LIBCORE_LINUX)
    register_android_system_OsConstants(env);
    return !env->ExceptionCheck();
#else
    return Register(env, "android/system/OsConstants", os_constants_methods, 1);
#endif
  };
  const auto register_openjdk_file_mapping = [&]() {
#if defined(DARWIN_ART_FULL_LIBCORE_LINUX)
    // Preserve the relative order from Android 16 libopenjdk's OnLoad.cpp.
    // Each call owns the complete upstream method table for its Java class;
    // partial or repeated RegisterNatives owners are forbidden.
    register_sun_nio_ch_IOUtil(env);
    if (env->ExceptionCheck()) {
      return false;
    }
    register_sun_nio_ch_FileChannelImpl(env);
    if (env->ExceptionCheck()) {
      return false;
    }
    register_sun_nio_ch_FileDispatcherImpl(env);
    if (env->ExceptionCheck()) {
      return false;
    }
    register_java_io_FileInputStream(env);
    if (env->ExceptionCheck()) {
      return false;
    }
    // Android's libopenjdk OnLoad normally owns these registrations. That
    // registrar is intentionally absent from the DarwinART libopenjdk image,
    // so install the exact OpenJDK method descriptors here. In particular,
    // socketRead0 includes the java.net timeout argument. Android removed the
    // old OpenJDK init() native from these classes, so do not register it.
    const JNINativeMethod socket_input_methods[] = {
        {const_cast<char*>("socketRead0"),
         const_cast<char*>("(Ljava/io/FileDescriptor;[BIII)I"),
         reinterpret_cast<void*>(&SocketInputStreamRead0)},
    };
    if (!Register(env, "java/net/SocketInputStream", socket_input_methods,
                  static_cast<jint>(std::size(socket_input_methods)))) {
      return false;
    }
    const JNINativeMethod socket_output_methods[] = {
        {const_cast<char*>("socketWrite0"),
         const_cast<char*>("(Ljava/io/FileDescriptor;[BII)V"),
         reinterpret_cast<void*>(&SocketOutputStreamWrite0)},
    };
    if (!Register(env, "java/net/SocketOutputStream", socket_output_methods,
                  static_cast<jint>(std::size(socket_output_methods)))) {
      return false;
    }
    register_sun_nio_ch_NativeThread(env);
    if (env->ExceptionCheck()) {
      return false;
    }
    const JNINativeMethod file_key_methods[] = {
        {const_cast<char*>("init"),
         const_cast<char*>("(Ljava/io/FileDescriptor;)V"),
         reinterpret_cast<void*>(&FileKeyInit)},
    };
    return Register(env, "sun/nio/ch/FileKey", file_key_methods,
                    static_cast<jint>(std::size(file_key_methods)));
#else
    return true;
#endif
  };
  const auto register_file_descriptor = [&]() {
#if defined(DARWIN_ART_FULL_LIBCORE_LINUX)
    register_java_io_FileDescriptor(env);
    return !env->ExceptionCheck();
#else
    return Register(env, "java/io/FileDescriptor", file_descriptor_methods, 2);
#endif
  };
  const auto register_libcore_memory = [&]() {
#if defined(DARWIN_ART_FULL_LIBCORE_LINUX)
    // StartMinimal has already registered ART's complementary seven array
    // methods. Android libjavacore owns the disjoint scalar/bulk eighteen.
    register_libcore_io_Memory(env);
    if (env->ExceptionCheck()) {
      return false;
    }
    DarwinArtLibcoreJniConstants::Initialize(env);
    return !env->ExceptionCheck();
#else
    return true;
#endif
  };
  return RegisterLibcoreCharacterNatives(env) &&
         // Android's OpenJDK OnLoad registers System before owners whose
         // FindClass/GetFieldID paths may initialize java.io or NIO classes.
         register_system() &&
         register_os_constants() &&
         register_linux() &&
         register_file_descriptor() &&
         register_openjdk_file_mapping() && register_libcore_memory();
}

bool RegisterManagedLoadNatives(JNIEnv* env) {
#if defined(DARWIN_ART_FULL_LIBCORE_LINUX)
  if (env == nullptr) {
    return false;
  }
  // Match Android 16 OnLoad.cpp: Runtime follows Math, then the complete
  // UnixNativeDispatcher table. Each registrar is one atomic JNI table.
  register_java_lang_Runtime(env);
  if (env->ExceptionCheck()) {
    return false;
  }
  JNINativeMethod process_environment_methods[] = {
      {const_cast<char*>("environ"), const_cast<char*>("()[[B"),
       reinterpret_cast<void*>(&Java_java_lang_ProcessEnvironment_environ)},
  };
  if (!Register(env, "java/lang/ProcessEnvironment",
                process_environment_methods,
                static_cast<jint>(std::size(process_environment_methods)))) {
    return false;
  }
    register_java_sun_nio_fs_UnixNativeDispatcher(env);
    if (env->ExceptionCheck()) {
      return false;
    }
    // ART's boot-native registration may revisit java.lang.System while the
    // managed-load set is being installed. Re-assert the complete AOSP
    // libopenjdk table last so System.log remains available during VMClassLoader
    // and java.nio initialization on a fresh runtime.
    register_java_lang_System(env);
  return !env->ExceptionCheck();
#else
  (void)env;
  return true;
#endif
}

bool ShutdownLibcoreNatives() {
#if defined(DARWIN_ART_FULL_LIBCORE_LINUX)
  return darwin_art_restore_sun_nio_ch_NativeThread_signal() == 0;
#else
  return true;
#endif
}

}  // namespace darwin_art
