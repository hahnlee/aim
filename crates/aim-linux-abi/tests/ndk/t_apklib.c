// A native library loaded straight from an APK (stored and page-aligned,
// as `android:extractNativeLibs="false"` apps load theirs): the layer
// finds the ELF inside the zip and rewrites its code at load time. argv[1]
// is the APK member's path.
#include <dlfcn.h>
#include <unistd.h>

#include "check.h"

static const char* path;

static void load_from_apk(void) {
  void* h = dlopen(path, RTLD_NOW);
  if (h == NULL) printf("dlopen: %s\n", dlerror());
  CHECK(h != NULL);
  long (*tid)(void) = (long (*)(void))dlsym(h, "apk_lib_tid");
  long (*calls)(void) = (long (*)(void))dlsym(h, "apk_lib_calls");
  CHECK(tid != NULL && calls != NULL);
  CHECK(tid() == gettid() && tid() == gettid());
  CHECK(calls() == 2);
}

int main(int argc, char** argv) {
  path = argv[1];
  RUN(load_from_apk);
  DONE();
}
