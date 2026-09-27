// A binder service and its clients over the image's libbinder_ndk:
//
//   binder_ping ping N        N pings of servicemanager (IBinder's
//                             PING_TRANSACTION, answered by its BBinder);
//   binder_ping serve NAME    add service NAME and serve it on this thread;
//   binder_ping call NAME N   N calls of NAME's increment method.
//
// ping and call print one line of round-trip times in microseconds, timed
// in the guest. The service manager and thread pool functions are platform
// API, absent from the NDK: they are looked up in libbinder_ndk at run
// time.

#include <android/binder_ibinder.h>
#include <dlfcn.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define DESCRIPTOR "dev.darwinart.IPing"
#define INCREMENT FIRST_CALL_TRANSACTION

static void* platform(const char* name) {
    void* f = dlsym(RTLD_DEFAULT, name);
    if (f == NULL) {
        fprintf(stderr, "binder_ping: %s not found\n", name);
        exit(1);
    }
    return f;
}

static void* on_create(void* args) { return args; }
static void on_destroy(void* data) { (void)data; }

static binder_status_t on_transact(AIBinder* binder, transaction_code_t code, const AParcel* in,
                                   AParcel* out) {
    (void)binder;
    int32_t v;
    if (code != INCREMENT || AParcel_readInt32(in, &v) != STATUS_OK) return STATUS_UNKNOWN_TRANSACTION;
    return AParcel_writeInt32(out, v + 1);
}

static AIBinder_Class* ping_class(void) {
    return AIBinder_Class_define(DESCRIPTOR, on_create, on_destroy, on_transact);
}

static AIBinder* check_service(const char* name) {
    AIBinder* (*check)(const char*) = platform("AServiceManager_checkService");
    return check(name);
}

static int cmp(const void* a, const void* b) {
    double x = *(const double*)a, y = *(const double*)b;
    return x < y ? -1 : x > y;
}

static double now_us(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e6 + t.tv_nsec / 1e3;
}

static void report(const char* what, double* us, int n) {
    qsort(us, n, sizeof *us, cmp);
    double sum = 0;
    for (int i = 0; i < n; i++) sum += us[i];
    printf("binder %s: n=%d p50=%.1f p90=%.1f p99=%.1f mean=%.1f us\n", what, n, us[n / 2],
           us[n * 9 / 10], us[n * 99 / 100], sum / n);
}

static int increment(AIBinder* b, int32_t v, int32_t* result) {
    AParcel* in = NULL;
    AParcel* out = NULL;
    if (AIBinder_prepareTransaction(b, &in) != STATUS_OK) return -1;
    AParcel_writeInt32(in, v);
    binder_status_t st = AIBinder_transact(b, INCREMENT, &in, &out, 0);
    if (st != STATUS_OK) return st;
    st = AParcel_readInt32(out, result);
    AParcel_delete(out);
    return st;
}

int main(int argc, char** argv) {
    if (argc == 3 && strcmp(argv[1], "serve") == 0) {
        binder_exception_t (*add)(AIBinder*, const char*) = platform("AServiceManager_addService");
        bool (*max_threads)(uint32_t) = platform("ABinderProcess_setThreadPoolMaxThreadCount");
        void (*join)(void) = platform("ABinderProcess_joinThreadPool");
        max_threads(0);
        AIBinder* b = AIBinder_new(ping_class(), NULL);
        if (add(b, argv[2]) != EX_NONE) {
            fprintf(stderr, "binder_ping: addService %s failed\n", argv[2]);
            return 1;
        }
        printf("serving %s\n", argv[2]);
        fflush(stdout);
        join();
        return 1;
    }
    int call = argc == 4 && strcmp(argv[1], "call") == 0;
    if (!call && !(argc == 3 && strcmp(argv[1], "ping") == 0)) {
        fprintf(stderr, "usage: binder_ping ping N | serve NAME | call NAME N\n");
        return 2;
    }
    AIBinder* b = check_service(call ? argv[2] : "manager");
    if (b == NULL) {
        fprintf(stderr, "binder_ping: %s not found\n", call ? argv[2] : "manager");
        return 1;
    }
    if (call && !AIBinder_associateClass(b, ping_class())) {
        fprintf(stderr, "binder_ping: %s is not %s\n", argv[2], DESCRIPTOR);
        return 1;
    }
    int n = atoi(argv[argc - 1]);
    int warm = n / 10;
    double* us = malloc(n * sizeof *us);
    for (int i = 0; i < warm + n; i++) {
        double t0 = now_us();
        int32_t r = 0;
        int st = call ? increment(b, i, &r) : AIBinder_ping(b);
        double t1 = now_us();
        if (st != STATUS_OK || (call && r != i + 1)) {
            fprintf(stderr, "binder_ping: call %d failed: status %d, result %d\n", i, st, r);
            return 1;
        }
        if (i >= warm) us[i - warm] = t1 - t0;
    }
    report(call ? "call" : "ping", us, n);
    return 0;
}
