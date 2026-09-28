#include <cerrno>
#include <cstdio>
#include <cstring>

#include <jni.h>

extern "C" jint JVM_GetLastErrorString(char* buf, int len);

int main() {
  char buffer[256] = {};
  errno = ENOENT;
  const jint length = JVM_GetLastErrorString(buffer, sizeof(buffer));
  if (length <= 0 || static_cast<size_t>(length) != std::strlen(buffer) ||
      buffer[sizeof(buffer) - 1] != '\0') {
    std::fprintf(stderr, "invalid JVM_GetLastErrorString result: length=%d text=%s\n",
                 length, buffer);
    return 1;
  }

  char sentinel = 'x';
  errno = ENOENT;
  if (JVM_GetLastErrorString(&sentinel, 0) != 0 || sentinel != 'x') {
    std::fputs("zero-length call modified storage\n", stderr);
    return 2;
  }

  std::printf("openjdkjvm-last-error: length=%d nonempty=pass zero=pass\n", length);
  return 0;
}
