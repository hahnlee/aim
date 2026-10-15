/* Authored fixture: enter through the privileged native launcher, then run original code unprivileged. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <linux/capability.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <unistd.h>
static int fail(const char *stage) { perror(stage); return 126; }
int main(int argc,char **argv) {
    if(argc<4) { fprintf(stderr,"usage: credential-drop UID PROOF EXEC [ARGS...]\n");return 126; }
    char *end;errno=0;unsigned long value=strtoul(argv[1],&end,10);
    if(errno||*end||value==0||value>0xffffffffUL) { fprintf(stderr,"invalid target UID\n");return 126; }
    struct __user_cap_header_struct header={_LINUX_CAPABILITY_VERSION_3,0};
    struct __user_cap_data_struct caps[2]={{0},{0}};
    if(syscall(SYS_capget,&header,caps))return fail("pre capget");
    if(getuid()!=0||geteuid()!=0||!(caps[0].effective&(1U<<21))) { fprintf(stderr,"authenticated privileged namespace entry required\n");return 126; }
    int proof=open(argv[2],O_WRONLY|O_CREAT|O_EXCL|O_CLOEXEC,0600);if(proof<0)return fail("proof open");
    if(dprintf(proof,"pre pid=%d uid=%d euid=%d cap_effective=%08x:%08x\n",getpid(),getuid(),geteuid(),caps[1].effective,caps[0].effective)<0)return fail("proof pre write");
    if(setgroups(0,NULL))return fail("setgroups");
    if(setresgid((gid_t)value,(gid_t)value,(gid_t)value))return fail("setresgid");
    if(setresuid((uid_t)value,(uid_t)value,(uid_t)value))return fail("setresuid");
    struct __user_cap_data_struct clear[2]={{0},{0}};
    if(syscall(SYS_capset,&header,clear))return fail("capset clear");
    if(syscall(SYS_capget,&header,caps))return fail("post capget");
    uid_t r,e,s;gid_t gr,ge,gs;
    if(getresuid(&r,&e,&s)||getresgid(&gr,&ge,&gs))return fail("post credentials");
    if(r!=value||e!=value||s!=value||gr!=value||ge!=value||gs!=value||getgroups(0,NULL)!=0||caps[0].effective||caps[1].effective||caps[0].permitted||caps[1].permitted||caps[0].inheritable||caps[1].inheritable) { fprintf(stderr,"credential drop incomplete\n");return 126; }
    if(dprintf(proof,"post pid=%d uid=%u euid=%u suid=%u gid=%u egid=%u sgid=%u groups=0 caps=0\n",getpid(),r,e,s,gr,ge,gs)<0||fsync(proof)||close(proof))return fail("proof finish");
    execv(argv[3],&argv[3]);return fail("original exec");
}
