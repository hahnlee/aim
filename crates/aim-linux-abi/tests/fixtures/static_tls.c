// A static bionic executable: no linker64, so libc itself lays out TLS
// from AT_PHDR and reads AT_PAGESZ (bionic's page_size()) at startup.
#include <stdio.h>
#include <unistd.h>

static __thread int initialized = 42;
static __thread int zeroed;

int main(void) {
  zeroed += 7;
  printf("page %ld tls %d %d\n", sysconf(_SC_PAGESIZE), initialized, zeroed);
  return 0;
}
