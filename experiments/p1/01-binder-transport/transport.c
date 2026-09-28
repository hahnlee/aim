// P1-1: cost of the binder driver's host transport between processes.
//
// Measures, with small (128-byte) inline messages:
//   direct   client <-> server Mach round trip (2 hops): the floor if the
//            driver core ran in the guests;
//   relayed  client -> daemon -> server -> daemon -> client (4 hops): a
//            transaction and its reply through a daemon-hosted driver, with
//            each guest thread blocked on its own reply port the way
//            IPCThreadState blocks in BINDER_WRITE_READ;
//   socket   the relayed path over AF_UNIX socketpairs, for comparison.
//
// Ports reach the forked children through mach_ports_register().

#include <mach/mach.h>
#include <mach/mach_time.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

#define ITER 20000
#define WARM 2000
#define PAYLOAD 128

typedef struct {
    mach_msg_header_t h;
    uint32_t kind;
    uint8_t body[PAYLOAD];
    mach_msg_trailer_t trailer;
} msg_t;

enum { FROM_CLIENT = 1, FROM_SERVER = 2 };

static void check(kern_return_t kr, const char *what) {
    if (kr != KERN_SUCCESS) {
        fprintf(stderr, "%s: %s (%d)\n", what, mach_error_string(kr), kr);
        exit(1);
    }
}

static mach_port_t new_port(void) {
    mach_port_t p;
    check(mach_port_allocate(mach_task_self(), MACH_PORT_RIGHT_RECEIVE, &p), "allocate");
    check(mach_port_insert_right(mach_task_self(), p, p, MACH_MSG_TYPE_MAKE_SEND), "insert");
    return p;
}

// Send `kind` to `dest` and wait for the answer on `reply` (one mach_msg).
static void call(mach_port_t dest, mach_port_t reply, uint32_t kind) {
    msg_t m;
    memset(&m, 0, sizeof m);
    m.h.msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MAKE_SEND_ONCE);
    m.h.msgh_size = sizeof m - sizeof m.trailer;
    m.h.msgh_remote_port = dest;
    m.h.msgh_local_port = reply;
    m.kind = kind;
    check(mach_msg(&m.h, MACH_SEND_MSG | MACH_RCV_MSG, m.h.msgh_size, sizeof m, reply,
                   MACH_MSG_TIMEOUT_NONE, MACH_PORT_NULL),
          "call");
}

static void answer(mach_port_t send_once, uint32_t kind) {
    msg_t m;
    memset(&m, 0, sizeof m);
    m.h.msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_MOVE_SEND_ONCE, 0);
    m.h.msgh_size = sizeof m - sizeof m.trailer;
    m.h.msgh_remote_port = send_once;
    m.kind = kind;
    check(mach_msg(&m.h, MACH_SEND_MSG, m.h.msgh_size, 0, MACH_PORT_NULL,
                   MACH_MSG_TIMEOUT_NONE, MACH_PORT_NULL),
          "answer");
}

// Receive on `port`; false once `child` has exited and nothing arrived for
// a second.
static int receive_while(mach_port_t port, msg_t *m, pid_t child) {
    for (;;) {
        kern_return_t kr = mach_msg(&m->h, MACH_RCV_MSG | MACH_RCV_TIMEOUT, 0, sizeof *m, port,
                                    1000, MACH_PORT_NULL);
        if (kr == KERN_SUCCESS) return 1;
        if (kr != MACH_RCV_TIMED_OUT) check(kr, "receive");
        if (waitpid(child, NULL, WNOHANG) == child) return 0;
    }
}

static mach_port_t inherited(int index) {
    mach_port_array_t ports;
    mach_msg_type_number_t count;
    check(mach_ports_lookup(mach_task_self(), &ports, &count), "lookup");
    return ports[index];
}

static int cmp(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return x < y ? -1 : x > y;
}

static void report(const char *name, double *us, int n) {
    qsort(us, n, sizeof *us, cmp);
    double sum = 0;
    for (int i = 0; i < n; i++) sum += us[i];
    printf("%-8s p50 %6.2f us  p90 %6.2f us  p99 %6.2f us  mean %6.2f us\n", name, us[n / 2],
           us[n * 9 / 10], us[n * 99 / 100], sum / n);
}

static double now_us(void) {
    static mach_timebase_info_data_t tb;
    if (!tb.denom) mach_timebase_info(&tb);
    return (double)mach_absolute_time() * tb.numer / tb.denom / 1000.0;
}

static void client_loop(const char *name, mach_port_t dest) {
    mach_port_t reply = new_port();
    static double us[ITER];
    for (int i = 0; i < WARM + ITER; i++) {
        double t0 = now_us();
        call(dest, reply, FROM_CLIENT);
        if (i >= WARM) us[i - WARM] = now_us() - t0;
    }
    report(name, us, ITER);
}

static void direct(void) {
    mach_port_t server = new_port();
    check(mach_ports_register(mach_task_self(), &server, 1), "register");
    pid_t client = fork();
    if (client == 0) {
        client_loop("direct", inherited(0));
        exit(0);
    }
    msg_t m;
    while (receive_while(server, &m, client)) answer(m.h.msgh_remote_port, m.kind);
}

static void relayed(void) {
    mach_port_t daemon = new_port();
    check(mach_ports_register(mach_task_self(), &daemon, 1), "register");
    pid_t server_pid = fork();
    if (server_pid == 0) {  // server: a looper blocked in its read
        mach_port_t d = inherited(0), reply = new_port();
        msg_t m;
        memset(&m, 0, sizeof m);
        call(d, reply, FROM_SERVER);  // first read
        for (;;) {
            // The answer to our read is the incoming transaction; reply
            // (BC_REPLY + read) in one call.
            call(d, reply, FROM_SERVER);
        }
    }
    pid_t client = fork();
    if (client == 0) {
        client_loop("relayed", inherited(0));
        exit(0);
    }
    // daemon: park the server's read, forward calls and replies.
    mach_port_t parked_server = MACH_PORT_NULL, parked_client = MACH_PORT_NULL;
    int undelivered = 0;  // a call that arrived before the server's read
    msg_t m;
    while (receive_while(daemon, &m, client)) {
        if (m.kind == FROM_CLIENT) {
            parked_client = m.h.msgh_remote_port;
            if (parked_server != MACH_PORT_NULL) {
                answer(parked_server, FROM_CLIENT);  // BR_TRANSACTION to the server
                parked_server = MACH_PORT_NULL;
            } else {
                undelivered = 1;
            }
        } else if (undelivered) {
            // The server's first read finds the queued call at once.
            undelivered = 0;
            answer(m.h.msgh_remote_port, FROM_CLIENT);
        } else {
            if (parked_client != MACH_PORT_NULL) {
                answer(parked_client, FROM_SERVER);  // BR_REPLY to the client
                parked_client = MACH_PORT_NULL;
            }
            parked_server = m.h.msgh_remote_port;  // the server's next read
        }
    }
    kill(server_pid, SIGKILL);
    waitpid(server_pid, NULL, 0);
}

static void sockets(void) {
    int cs[2], ss[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, cs) || socketpair(AF_UNIX, SOCK_STREAM, 0, ss)) {
        perror("socketpair");
        exit(1);
    }
    char buf[PAYLOAD + 4];
    pid_t server = fork();
    if (server == 0) {
        close(cs[0]), close(cs[1]), close(ss[0]);
        while (read(ss[1], buf, sizeof buf) > 0) write(ss[1], buf, sizeof buf);
        exit(0);
    }
    pid_t client = fork();
    if (client == 0) {
        close(ss[0]), close(ss[1]), close(cs[0]);
        static double us[ITER];
        memset(buf, 0, sizeof buf);
        for (int i = 0; i < WARM + ITER; i++) {
            double t0 = now_us();
            write(cs[1], buf, sizeof buf);
            read(cs[1], buf, sizeof buf);
            if (i >= WARM) us[i - WARM] = now_us() - t0;
        }
        report("socket", us, ITER);
        exit(0);
    }
    close(cs[1]), close(ss[1]);
    // The daemon relays both ways.
    for (int i = 0; i < WARM + ITER; i++) {
        read(cs[0], buf, sizeof buf);
        write(ss[0], buf, sizeof buf);
        read(ss[0], buf, sizeof buf);
        write(cs[0], buf, sizeof buf);
    }
    close(ss[0]);
    waitpid(client, NULL, 0);
    waitpid(server, NULL, 0);
}

int main(int argc, char **argv) {
    // Every mode runs in a fresh process; a stuck run ends after 60 s.
    alarm(60);
    const char *mode = argc > 1 ? argv[1] : "";
    if (!strcmp(mode, "direct")) direct();
    else if (!strcmp(mode, "relayed")) relayed();
    else if (!strcmp(mode, "socket")) sockets();
    else {
        fprintf(stderr, "usage: %s direct|relayed|socket\n", argv[0]);
        return 2;
    }
    return 0;
}
