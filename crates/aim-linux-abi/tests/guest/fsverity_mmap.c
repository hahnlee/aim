#define _GNU_SOURCE
#include <sys/mman.h>
#include <sys/wait.h>
#include <signal.h>
#include <setjmp.h>
#include <stdint.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdio.h>
#include <stdlib.h>
#include <errno.h>
static sigjmp_buf jump;
static volatile uintptr_t address;
static volatile sig_atomic_t armed;
static volatile sig_atomic_t expected_signal=SIGBUS;
static volatile sig_atomic_t caught_signal;
static volatile unsigned char bss_tail[17003];
static __thread volatile unsigned int tls_marker=0x12345;
static const unsigned char readonly_bytes[16384] __attribute__((aligned(16384)))={47};
__attribute__((noinline)) static void write_readonly(void){__asm__ volatile("mov w9, #99\nstrb w9, [%0]"::"r"(readonly_bytes):"x9","memory");}
__attribute__((noinline)) static uintptr_t thread_pointer(void){uintptr_t value;__asm__ volatile("mrs %0, tpidr_el0":"=r"(value));return value;}
static void bus(int sig, siginfo_t *info, void *ctx) { (void)ctx; if(sig!=expected_signal||info->si_code!=(sig==SIGBUS?BUS_ADRERR:SEGV_ACCERR))_exit(90);if(!armed)_exit(94);armed=0;caught_signal=sig;address=(uintptr_t)info->si_addr;siglongjmp(jump,1); }
#define CHECK(x) do { if(!(x)) { perror(#x); return 1; } } while(0)
int main(void) {
 CHECK(thread_pointer()!=0);CHECK(tls_marker==0x12345);tls_marker=0x34567;CHECK(tls_marker==0x34567);
 for(size_t i=0;i<sizeof(bss_tail);i++)CHECK(bss_tail[i]==0);bss_tail[17002]=63;CHECK(bss_tail[17002]==63);
 struct sigaction action={.sa_sigaction=bus,.sa_flags=SA_SIGINFO};sigemptyset(&action.sa_mask);CHECK(sigaction(SIGBUS,&action,0)==0);CHECK(sigaction(SIGSEGV,&action,0)==0);
 expected_signal=SIGSEGV;
 if(sigsetjmp(jump,1)==0){armed=1;write_readonly();return 4;}
 CHECK(caught_signal==SIGSEGV&&address==(uintptr_t)readonly_bytes);CHECK(readonly_bytes[0]==47);expected_signal=SIGBUS;

 int fd=open("/data/verity-data",O_RDONLY);CHECK(fd>=0);
 unsigned char *shared=mmap(0,16384,PROT_READ,MAP_SHARED,fd,0);CHECK(shared!=MAP_FAILED);CHECK(shared[17]==37);
 unsigned char *alias=mmap(0,16384,PROT_READ,MAP_SHARED,fd,0);CHECK(alias!=MAP_FAILED);CHECK(alias[17]==37);
 CHECK(mprotect(shared,16384,PROT_READ|PROT_WRITE)==-1&&errno==EACCES);
 CHECK(munmap(shared,16384)==0);CHECK(munmap(alias,16384)==0);
 unsigned char *map=mmap(0,32768,PROT_READ|PROT_WRITE,MAP_PRIVATE,fd,0);CHECK(map!=MAP_FAILED);CHECK(close(fd)==0);
 CHECK(map[17]==37);map[17]=91;CHECK(map[17]==91);
 unsigned char *moved=mremap(map,32768,49152,MREMAP_MAYMOVE);CHECK(moved!=MAP_FAILED);map=moved;CHECK(map[17]==91);
 CHECK(mprotect(map,16384,PROT_READ)==0);CHECK(map[17]==91);
 CHECK(madvise(map,16384,MADV_DONTNEED)==0);CHECK(map[17]==37);
 CHECK(mprotect(map,16384,PROT_READ|PROT_WRITE)==0);map[17]=91;
 pid_t child=fork();CHECK(child>=0);
 if(child==0) {
  if(thread_pointer()==0||tls_marker!=0x34567)_exit(98);tls_marker=0x45678;if(tls_marker!=0x45678)_exit(99);
  int opened=open("/data/verity-data",O_RDONLY);if(opened<0)_exit(95);
  unsigned char *fresh=mmap(0,16384,PROT_READ,MAP_SHARED,opened,0);if(fresh==MAP_FAILED||fresh[17]!=37)_exit(96);
  if(munmap(fresh,16384)!=0||close(opened)!=0)_exit(97);
  if(map[17]!=91)_exit(91);map[17]=73;
  if(sigsetjmp(jump,1)==0) {armed=1;volatile unsigned char byte=map[16384+20];(void)byte;_exit(92);}
  if(address!=(uintptr_t)(map+16384+20)||map[17]!=73)_exit(93);
  _exit(0);
 }
 int status;CHECK(waitpid(child,&status,0)==child);CHECK(WIFEXITED(status)&&WEXITSTATUS(status)==0);CHECK(map[17]==91);CHECK(tls_marker==0x34567);
 if(sigsetjmp(jump,1)==0) {armed=1; volatile unsigned char byte=map[16384+20];(void)byte;return 2; }
 CHECK(address==(uintptr_t)(map+16384+20));CHECK(map[17]==91);
 if(sigsetjmp(jump,1)==0) {armed=1; volatile unsigned char byte=map[32768];(void)byte;return 3; }
 CHECK(address==(uintptr_t)(map+32768));
 unsigned char *shrunk=mremap(map,49152,16384,0);CHECK(shrunk==map);CHECK(map[17]==91);CHECK(munmap(map,16384)==0);
 puts("VERITY_MMAP_SIGBUS_ADDRESS_COW_PASS");return 0;
}
