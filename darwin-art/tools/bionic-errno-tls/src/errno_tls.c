#include "darwin_art_bionic_errno.h"

#include <errno.h>
#include <stddef.h>

typedef struct ErrnoMapping {
  int32_t darwin_errno;
  int32_t android_errno;
} ErrnoMapping;

static _Thread_local int32_t gAndroidErrno;

static const ErrnoMapping kMappings[] = {
#include "darwin_to_android.inc"
};

_Static_assert(sizeof(int) == 4, "Bionic arm64 errno cell is a 32-bit int");
_Static_assert(sizeof(int32_t) == sizeof(int), "errno cell representation drift");

static int Translate(int darwin_errno, int32_t* android_errno) {
  if (android_errno == NULL) return 0;
  for (size_t index = 0; index < sizeof(kMappings) / sizeof(kMappings[0]); ++index) {
    if (kMappings[index].darwin_errno == darwin_errno) {
      *android_errno = kMappings[index].android_errno;
      return 1;
    }
  }
  return 0;
}

int32_t* darwin_art_bionic___errno(void) {
  return &gAndroidErrno;
}

int32_t darwin_art_bionic_errno_load(void) {
  return gAndroidErrno;
}

void darwin_art_bionic_errno_store(int32_t android_errno) {
  gAndroidErrno = android_errno;
}

int darwin_art_bionic_errno_from_darwin(int darwin_errno,
                                        int32_t* android_errno) {
  const int saved_host_errno = errno;
  const int translated = Translate(darwin_errno, android_errno);
  errno = saved_host_errno;
  return translated;
}

int darwin_art_bionic_errno_set_from_darwin(int darwin_errno) {
  const int saved_host_errno = errno;
  int32_t translated_errno;
  const int translated = Translate(darwin_errno, &translated_errno);
  if (translated) gAndroidErrno = translated_errno;
  errno = saved_host_errno;
  return translated;
}

int darwin_art_bionic_errno_to_darwin(int32_t android_errno, int* darwin_errno) {
  if (darwin_errno == NULL) return 0;
  for (size_t index = 0; index < sizeof(kMappings) / sizeof(kMappings[0]); ++index) {
    if (kMappings[index].android_errno == android_errno) {
      *darwin_errno = kMappings[index].darwin_errno;
      return 1;
    }
  }
  return 0;
}

int darwin_art_bionic_errno_capture_host(void) {
  const int saved_host_errno = errno;
  int32_t translated_errno;
  const int translated = Translate(saved_host_errno, &translated_errno);
  if (translated) gAndroidErrno = translated_errno;
  errno = saved_host_errno;
  return translated;
}

void darwin_art_bionic_errno_publish_result(int32_t android_errno) {
  if (android_errno != 0) gAndroidErrno = android_errno;
}

static int NameEquals(const char* left, const char* right) {
  if (left == NULL || right == NULL) return 0;
  while (*left == *right && *left != '\0') {
    ++left;
    ++right;
  }
  return *left == *right;
}

DarwinArtBionicErrnoFunction darwin_art_bionic_errno_resolve(
    const char* import_name) {
  return NameEquals(import_name, "__errno")
             ? (DarwinArtBionicErrnoFunction)darwin_art_bionic___errno
             : NULL;
}
