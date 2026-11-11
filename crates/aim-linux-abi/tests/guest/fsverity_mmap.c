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
#include <pthread.h>
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
// A verified private mapping that a second thread faults in, writes,
// discards with MADV_DONTNEED and remaps, while the main thread forks.
static unsigned char *volatile racing;
static volatile int race_stop;
static void *race(void *arg) {
 int fd=*(int *)arg;
 while(!race_stop) {
  unsigned char *p=mmap(0,16384,PROT_READ|PROT_WRITE,MAP_PRIVATE,fd,0);if(p==MAP_FAILED)_exit(80);
  if(p[17]!=37)_exit(81);p[17]=91;racing=p;
  if(madvise(p,16384,MADV_DONTNEED)!=0||p[17]!=37)_exit(82);p[17]=91;
  racing=0;if(munmap(p,16384)!=0)_exit(83);
 }
 return 0;
}
// In each child, the page the thread had reads as it was copied (the file's
// verified bytes or the thread's write), keeps that through an mprotect,
// and is verified again after MADV_DONTNEED: its page state agrees with
// the copied memory.
static int fork_while_paging(void) {
 int fd=open("/data/verity-data",O_RDONLY);CHECK(fd>=0);
 pthread_t thread;CHECK(pthread_create(&thread,0,race,&fd)==0);
 int checked=0;
 for(int n=0;n<100;n++) {
  int report[2];CHECK(pipe(report)==0);
  pid_t child=fork();CHECK(child>=0);
  if(child==0) {
   unsigned char *p=racing;int seen=p!=0;
   if(p) {
    unsigned char v=p[17];if(v!=37&&v!=91)_exit(84);
    if(mprotect(p,16384,PROT_READ|PROT_WRITE)!=0||p[17]!=v)_exit(85);
    if(madvise(p,16384,MADV_DONTNEED)!=0||p[17]!=37)_exit(86);
   }
   if(write(report[1],&seen,sizeof(seen))!=sizeof(seen))_exit(87);
   _exit(0);
  }
  close(report[1]);int seen=0;CHECK(read(report[0],&seen,sizeof(seen))==sizeof(seen));close(report[0]);
  int status;CHECK(waitpid(child,&status,0)==child);
  if(!WIFEXITED(status)||WEXITSTATUS(status)!=0){fprintf(stderr,"fork %d: child status %#x\n",n,status);return 1;}
  checked+=seen;
 }
 race_stop=1;CHECK(pthread_join(thread,0)==0);CHECK(close(fd)==0);CHECK(checked>0);
 return 0;
}
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
 CHECK(fork_while_paging()==0);
 puts("VERITY_MMAP_SIGBUS_ADDRESS_COW_PASS");return 0;
}
