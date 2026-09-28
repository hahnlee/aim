// Loads a library of the image through the original linker, running its
// constructors, and looks up a symbol: `dlopen_library PATH [SYMBOL]`.
#include <dlfcn.h>
#include <stdio.h>

int main(int argc, char** argv) {
  if (argc < 2) return 2;
  void* h = dlopen(argv[1], RTLD_NOW);
  if (h == NULL) {
    printf("dlopen failed: %s\n", dlerror());
    return 1;
  }
  printf("ok loaded %s\n", argv[1]);
  if (argc > 2) {
    void* s = dlsym(h, argv[2]);
    if (s == NULL) {
      printf("dlsym failed: %s\n", dlerror());
      return 1;
    }
    printf("ok symbol %s\n", argv[2]);
  }
  return 0;
}
