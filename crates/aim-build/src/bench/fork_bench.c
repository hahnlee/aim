// Guest process-creation latency for `cargo aim bench`:
//
//   fork_bench fork N       N x fork(); the child _exit(0)s; waitpid();
//   fork_bench spawn N      N x fork(); the child execs /system/bin/true;
//                           waitpid().
//
// Prints one line of round-trip times in microseconds, timed in the guest:
// "fork: n=N p50=.. p90=.. p99=.. mean=.. us".

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static double now_us(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e6 + t.tv_nsec / 1e3;
}

static int cmp(const void* a, const void* b) {
    double x = *(const double*)a, y = *(const double*)b;
    return x < y ? -1 : x > y;
}

int main(int argc, char** argv) {
    int spawn = argc == 3 && strcmp(argv[1], "spawn") == 0;
    if (argc != 3 || (!spawn && strcmp(argv[1], "fork") != 0)) {
        fprintf(stderr, "usage: fork_bench fork|spawn N\n");
        return 2;
    }
    int n = atoi(argv[2]);
    int warm = n / 10 + 1;
    double* us = malloc(n * sizeof *us);
    for (int i = 0; i < warm + n; i++) {
        double t0 = now_us();
        pid_t pid = fork();
        if (pid < 0) {
            perror("fork");
            return 1;
        }
        if (pid == 0) {
            if (spawn) {
                execl("/system/bin/true", "true", (char*)NULL);
                _exit(127);
            }
            _exit(0);
        }
        int status;
        if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            fprintf(stderr, "fork_bench: child %d failed: status %#x\n", pid, status);
            return 1;
        }
        double t1 = now_us();
        if (i >= warm) us[i - warm] = t1 - t0;
    }
    qsort(us, n, sizeof *us, cmp);
    double sum = 0;
    for (int i = 0; i < n; i++) sum += us[i];
    printf("%s: n=%d p50=%.1f p90=%.1f p99=%.1f mean=%.1f us\n", argv[1], n, us[n / 2],
           us[n * 9 / 10], us[n * 99 / 100], sum / n);
    return 0;
}
