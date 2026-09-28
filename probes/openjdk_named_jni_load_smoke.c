#define _DARWIN_C_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <string.h>

static int fail(const char* operation) {
  const char* error = dlerror();
  fprintf(stderr, "%s: %s\n", operation, error == NULL ? "failed" : error);
  return 1;
}

int main(int argc, char** argv) {
  if (argc != 3) {
    fprintf(stderr, "usage: %s RUNTIME OWNER\n", argv[0]);
    return 2;
  }

  void* runtime = dlopen(argv[1], RTLD_NOW | RTLD_GLOBAL);
  if (runtime == NULL) return fail("RTLD_GLOBAL runtime");

  dlerror();
  void* jvm = dlsym(RTLD_DEFAULT, "JVM_GetLastErrorString");
  if (jvm == NULL) return fail("global JVM_GetLastErrorString");
  Dl_info provider;
  if (dladdr(jvm, &provider) == 0 || provider.dli_fname == NULL ||
      strcmp(provider.dli_fname, argv[1]) != 0) {
    fprintf(stderr, "JVM_GetLastErrorString is not owned by runtime\n");
    return 1;
  }

  void* owner = dlopen(argv[2], RTLD_NOW | RTLD_LOCAL);
  if (owner == NULL) return fail("RTLD_LOCAL named-JNI owner");
  dlerror();
  if (dlsym(owner, "Java_java_io_FileInputStream_available0") == NULL) {
    return fail("named-JNI lookup");
  }

  puts("openjdk-owner-load: RTLD_GLOBAL(runtime)->RTLD_LOCAL(owner)=PASS");
  return 0;
}
