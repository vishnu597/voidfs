// bulk <file> <block bytes> <count>: writes count blocks, then fsync and close; prints MiB/s and
// the write(2) latency percentiles.
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec + t.tv_nsec / 1e9; }
static int cmp(const void *a, const void *b) { double x = *(double *)a, y = *(double *)b; return x < y ? -1 : x > y; }
int main(int argc, char **argv) {
    size_t bs = strtoul(argv[2], 0, 0); int count = atoi(argv[3]);
    char *buf = malloc(bs); memset(buf, 'b', bs);
    double *lat = malloc(sizeof(double) * count);
    unlink(argv[1]);
    double t0 = now();
    int fd = open(argv[1], O_CREAT | O_RDWR, 0644);
    for (int i = 0; i < count; i++) { double a = now(); if (write(fd, buf, bs) != (ssize_t)bs) { perror("write"); return 1; } lat[i] = now() - a; }
    double tw = now();
    fsync(fd); close(fd);
    double t1 = now();
    qsort(lat, count, sizeof(double), cmp);
    printf("%zu B x %d: %.1f MiB/s overall (writes %.2f s, fsync+close %.3f s); write p50 %.2f ms p99 %.2f ms max %.2f ms\n", bs, count,
           bs * (double)count / (1 << 20) / (t1 - t0), tw - t0, t1 - tw, lat[count / 2] * 1e3, lat[count * 99 / 100] * 1e3, lat[count - 1] * 1e3);
    return 0;
}
