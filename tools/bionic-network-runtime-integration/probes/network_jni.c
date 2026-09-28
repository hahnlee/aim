#include <errno.h>
#include <jni.h>
#include <netdb.h>
#include <stdint.h>
#include <sys/socket.h>
#include <unistd.h>

static int PortString(int port, char output[6]) {
  if (port <= 0 || port > 65535)
    return -1;
  char reversed[5];
  int count = 0;
  while (port != 0) {
    reversed[count++] = (char)('0' + port % 10);
    port /= 10;
  }
  for (int index = 0; index < count; ++index) {
    output[index] = reversed[count - index - 1];
  }
  output[count] = '\0';
  return 0;
}

static int SendRequest(int descriptor) {
  static const char request[] =
      "GET /runtime HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
  size_t offset = 0;
  while (offset < sizeof(request) - 1) {
    size_t length = sizeof(request) - 1 - offset;
    if (length > 3)
      length = 3;
    const ssize_t count = send(descriptor, request + offset, length, 0);
    if (count < 0 && errno == EINTR)
      continue;
    if (count <= 0)
      return -1;
    offset += (size_t)count;
  }
  return 0;
}

static int ReceiveResponse(int descriptor) {
  static const char expected[] =
      "HTTP/1.0 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK";
  size_t offset = 0;
  char response[sizeof(expected)];
  for (;;) {
    size_t capacity = sizeof(response) - 1 - offset;
    if (capacity > 3)
      capacity = 3;
    if (capacity == 0)
      break;
    const ssize_t count = recv(descriptor, response + offset, capacity, 0);
    if (count < 0 && errno == EINTR)
      continue;
    if (count < 0)
      return -1;
    if (count == 0)
      break;
    offset += (size_t)count;
  }
  if (offset != sizeof(expected) - 1)
    return -1;
  response[offset] = '\0';
  for (size_t index = 0; index < sizeof(expected); ++index) {
    if (response[index] != expected[index])
      return -1;
  }
  return 0;
}

static jint NativeLoopbackHttp(JNIEnv *env, jclass clazz, jint port) {
  (void)env;
  (void)clazz;
  char service[6];
  if (PortString(port, service) != 0)
    return -1;
  struct addrinfo hints = {
      .ai_flags = AI_NUMERICHOST | AI_NUMERICSERV,
      .ai_family = AF_INET,
      .ai_socktype = SOCK_STREAM,
      .ai_protocol = 0,
  };
  struct addrinfo *results = NULL;
  if (getaddrinfo("127.0.0.1", service, &hints, &results) != 0)
    return -2;
  int status = -3;
  for (const struct addrinfo *address = results; address != NULL;
       address = address->ai_next) {
    const int descriptor =
        socket(address->ai_family, address->ai_socktype, address->ai_protocol);
    if (descriptor < 0)
      continue;
    if (connect(descriptor, address->ai_addr, address->ai_addrlen) == 0 &&
        SendRequest(descriptor) == 0 && ReceiveResponse(descriptor) == 0) {
      status = 42;
    }
    (void)close(descriptor);
    if (status == 42)
      break;
  }
  freeaddrinfo(results);
  return status;
}

static JNINativeMethod kMethods[] = {
    {"nativeLoopbackHttp", "(I)I", (void *)&NativeLoopbackHttp},
};

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM *vm, void *reserved) {
  (void)reserved;
  JNIEnv *env = NULL;
  if ((*vm)->GetEnv(vm, (void **)&env, JNI_VERSION_1_6) != JNI_OK ||
      env == NULL) {
    return JNI_ERR;
  }
  jclass fixture =
      (*env)->FindClass(env, "dev/aim/probe/NetworkRuntimeFixture");
  if (fixture == NULL ||
      (*env)->RegisterNatives(env, fixture, kMethods, 1) != JNI_OK) {
    if (fixture != NULL)
      (*env)->DeleteLocalRef(env, fixture);
    return JNI_ERR;
  }
  (*env)->DeleteLocalRef(env, fixture);
  return JNI_VERSION_1_6;
}
