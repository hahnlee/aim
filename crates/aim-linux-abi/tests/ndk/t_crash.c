// Dies of SIGSEGV at address 0x104 after the linker's debuggerd handler
// ran: the layer reports the fault (pc, lr, mapping, frames).
#include <stdio.h>

__attribute__((noinline)) static int load(volatile int* p) { return *p; }

int main(void) {
  printf("crashing\n");
  fflush(stdout);
  return load((volatile int*)0x104);
}
