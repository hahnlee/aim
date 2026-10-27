// LTP-style EFAULT checks: invalid, protected and concurrently revoked buffers.
#include <errno.h>
#include <fcntl.h>
#include <linux/capability.h>
#include <linux/futex.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <sys/timerfd.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <unistd.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "%d: %s (errno %d)\n", __LINE__, #x, errno); exit(1); } } while (0)
#define FAULT(x) do { errno = 0; long r_ = (x); CHECK(r_ == -1 && errno == EFAULT); } while (0)

static atomic_int stop;
static void *page;
static size_t page_size;
static void *revoke(void *unused) {
    (void)unused;
    while (!atomic_load(&stop)) {
        CHECK(mprotect(page, page_size, PROT_NONE) == 0);
        CHECK(mprotect(page, page_size, PROT_READ | PROT_WRITE) == 0);
    }
    return NULL;
}

static int descriptor_count(void) {
    int count = 0;
    for (int fd = 0; fd < 256; ++fd) {
        if (fcntl(fd, F_GETFD) >= 0) ++count;
    }
    return count;
}

static void bad_copies(void *bad) {
    FAULT(syscall(SYS_prlimit64, 0, RLIMIT_NOFILE, NULL, bad));
    FAULT(syscall(SYS_prlimit64, 0, RLIMIT_NOFILE, bad, NULL));
    FAULT(syscall(SYS_uname, bad));
    FAULT(syscall(SYS_getresuid, bad, bad, bad));
    FAULT(syscall(SYS_clock_gettime, CLOCK_MONOTONIC, bad));
    FAULT(syscall(SYS_gettimeofday, bad, NULL));
    FAULT(syscall(SYS_rt_sigaction, SIGUSR1, bad, NULL, 8));
    FAULT(syscall(SYS_rt_sigaction, SIGUSR1, NULL, bad, 8));
    FAULT(syscall(SYS_rt_sigprocmask, SIG_SETMASK, bad, NULL, 8));
    FAULT(syscall(SYS_rt_sigprocmask, SIG_SETMASK, NULL, bad, 8));
    FAULT(syscall(SYS_rt_sigpending, bad, 8));
    FAULT(syscall(SYS_rt_sigtimedwait, bad, NULL, &(struct timespec){0}, 8));
    FAULT(syscall(SYS_rt_sigqueueinfo, getpid(), SIGUSR1, bad));
    FAULT(syscall(SYS_sigaltstack, bad, NULL));
    FAULT(syscall(SYS_get_robust_list, 0, bad, bad));
    FAULT(syscall(SYS_prctl, PR_GET_NAME, bad));
    FAULT(syscall(SYS_clone3, bad, 88));
    FAULT(syscall(SYS_setitimer, ITIMER_REAL, bad, NULL));
    FAULT(syscall(SYS_ppoll, bad, 1, &(struct timespec){0}, NULL, 8));
    FAULT(syscall(SYS_pselect6, 1, bad, NULL, NULL, &(struct timespec){0}, NULL));
    struct __user_cap_header_struct header = { _LINUX_CAPABILITY_VERSION_3, 0 };
    FAULT(syscall(SYS_capget, &header, bad));
    FAULT(syscall(SYS_futex, bad, FUTEX_WAIT_PRIVATE, 0, &(struct timespec){0}, NULL, 0));
    int timer = timerfd_create(CLOCK_MONOTONIC, TFD_CLOEXEC);
    CHECK(timer >= 0);
    FAULT(syscall(SYS_timerfd_gettime, timer, bad));
    CHECK(close(timer) == 0);
    int event = eventfd(0, EFD_CLOEXEC);
    CHECK(event >= 0);
    FAULT(syscall(SYS_write, event, bad, 8));
    int epoll = epoll_create1(EPOLL_CLOEXEC);
    CHECK(epoll >= 0);
    FAULT(syscall(SYS_epoll_ctl, epoll, EPOLL_CTL_ADD, event, bad));
    FAULT(syscall(SYS_epoll_pwait2, epoll, bad, 1, bad, NULL, 8));
    CHECK(close(epoll) == 0 && close(event) == 0);
    int before = descriptor_count();
    FAULT(syscall(SYS_socketpair, AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, bad));
    CHECK(descriptor_count() == before);
    struct sigevent event_spec = { .sigev_notify = SIGEV_NONE };
    FAULT(syscall(SYS_timer_create, CLOCK_MONOTONIC, &event_spec, bad));
    int timer_id = -1;
    CHECK(syscall(SYS_timer_create, CLOCK_MONOTONIC, &event_spec, &timer_id) == 0);
    CHECK(timer_id == 0);
    CHECK(syscall(SYS_timer_delete, timer_id) == 0);
    int socket_fd = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
    CHECK(socket_fd >= 0);
    FAULT(syscall(SYS_sendmsg, socket_fd, bad, 0));
    FAULT(syscall(SYS_setsockopt, socket_fd, SOL_SOCKET, SO_RCVBUF, bad, 4));
    FAULT(syscall(SYS_setsockopt, socket_fd, 0, 49 /* IP_MULTICAST_ALL */, bad, 1));
    socklen_t length = 4;
    FAULT(syscall(SYS_getsockopt, socket_fd, SOL_SOCKET, SO_RCVBUF, bad, &length));
    CHECK(close(socket_fd) == 0);
    int ashmem = open("/dev/ashmem", O_RDWR | O_CLOEXEC);
    CHECK(ashmem >= 0);
    FAULT(syscall(SYS_ioctl, ashmem, 0x81007702 /* ASHMEM_GET_NAME */, bad));
    CHECK(close(ashmem) == 0);
}

static void unexpected_handler(int signo) {
    (void)signo;
    _exit(77);
}

static void bad_signal_stack(void) {
    for (int synchronous = 0; synchronous < 2; ++synchronous) {
        pid_t child = fork();
        CHECK(child >= 0);
        if (child == 0) {
            stack_t alt = { .ss_sp = page, .ss_size = 2 * page_size };
            CHECK(sigaltstack(&alt, NULL) == 0);
            struct sigaction sa = { .sa_handler = unexpected_handler, .sa_flags = SA_ONSTACK };
            CHECK(sigemptyset(&sa.sa_mask) == 0);
            CHECK(sigaction(synchronous ? SIGSEGV : SIGUSR1, &sa, NULL) == 0);
            CHECK(mprotect(page, 2 * page_size, PROT_NONE) == 0);
            if (synchronous) {
                // Frame construction happens inside the host fault handler;
                // the copy's nested SIGBUS must reach its exception fixup.
                (void)*(volatile unsigned char *)page;
            } else {
                CHECK(raise(SIGUSR1) == 0);
            }
            _exit(78);
        }
        int status;
        CHECK(waitpid(child, &status, 0) == child);
        CHECK(WIFSIGNALED(status) && WTERMSIG(status) == SIGSEGV);
    }
}

int main(void) {
    page_size = sysconf(_SC_PAGESIZE);
    page = mmap(NULL, 2 * page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(page != MAP_FAILED);
    CHECK(mprotect(page, 2 * page_size, PROT_NONE) == 0);
    bad_copies(page);
    CHECK(mprotect(page, page_size, PROT_READ | PROT_WRITE) == 0);
    // Every fixed-size structure straddles the inaccessible second page.
    // The default ashmem name fits, but its trailing NUL is on PROT_NONE.
    int ashmem = open("/dev/ashmem", O_RDWR | O_CLOEXEC);
    CHECK(ashmem >= 0);
    FAULT(syscall(SYS_ioctl, ashmem, 0x81007702, (char *)page + page_size - 10));
    CHECK(close(ashmem) == 0);
    void *crossing = (char *)page + page_size - 8;
    FAULT(syscall(SYS_prlimit64, 0, RLIMIT_NOFILE, NULL, crossing));
    FAULT(syscall(SYS_prlimit64, 0, RLIMIT_NOFILE, crossing, NULL));
    FAULT(syscall(SYS_clock_gettime, CLOCK_MONOTONIC, crossing));
    CHECK(mprotect(page, page_size, PROT_READ) == 0);
    FAULT(syscall(SYS_prlimit64, 0, RLIMIT_NOFILE, NULL, page));
    // A readable VM region whose backing object has no page faults as SIGBUS
    // on direct access; a kernel user copy returns EFAULT.
    int file = open("/data/local/tmp/user-copy-empty", O_RDWR | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
    CHECK(file >= 0);
    void *empty = mmap(NULL, page_size, PROT_READ | PROT_WRITE, MAP_SHARED, file, 0);
    CHECK(empty != MAP_FAILED);
    FAULT(syscall(SYS_prlimit64, 0, RLIMIT_NOFILE, empty, NULL));
    FAULT(syscall(SYS_prlimit64, 0, RLIMIT_NOFILE, NULL, empty));
    CHECK(munmap(empty, page_size) == 0 && close(file) == 0);
    CHECK(unlink("/data/local/tmp/user-copy-empty") == 0);
    CHECK(mprotect(page, page_size, PROT_READ | PROT_WRITE) == 0);
    struct rlimit limit;
    CHECK(getrlimit(RLIMIT_NOFILE, &limit) == 0);
    memcpy(page, &limit, sizeof(limit));
    pthread_t worker;
    CHECK(pthread_create(&worker, NULL, revoke, NULL) == 0);
    for (int i = 0; i < 10000; ++i) {
        long r = syscall(SYS_prlimit64, 0, RLIMIT_NOFILE, page, NULL);
        CHECK(r == 0 || (r == -1 && errno == EFAULT));
        r = syscall(SYS_prlimit64, 0, RLIMIT_NOFILE, NULL, page);
        CHECK(r == 0 || (r == -1 && errno == EFAULT));
    }
    atomic_store(&stop, 1);
    CHECK(pthread_join(worker, NULL) == 0);
    bad_signal_stack();
    CHECK(munmap(page, 2 * page_size) == 0);
    bad_copies(page);
    puts("PASS user memory copies");
    return 0;
}
