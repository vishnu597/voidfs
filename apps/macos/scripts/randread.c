// SPDX-License-Identifier: Apache-2.0
//
// randread: latency of small reads at random, block-aligned offsets of one file.
//
//     cc -O2 -o randread randread.c
//     randread <file> [count=300] [bytes=4096] [nocache=1]
//
// With nocache=1 the file is opened with F_NOCACHE, so every read bypasses the unified buffer
// cache and reaches the file system, as a first touch of cold data would.

#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

static int cmp(const void *a, const void *b) {
    uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
    return x < y ? -1 : x > y;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: randread <file> [count] [bytes] [nocache]\n");
        return 2;
    }
    int count = argc > 2 ? atoi(argv[2]) : 300;
    size_t bytes = argc > 3 ? (size_t)atol(argv[3]) : 4096;
    int nocache = argc > 4 ? atoi(argv[4]) : 1;
    int fd = open(argv[1], O_RDONLY);
    if (fd < 0) { perror("open"); return 1; }
    if (nocache && fcntl(fd, F_NOCACHE, 1) < 0) { perror("F_NOCACHE"); return 1; }
    struct stat st;
    fstat(fd, &st);
    void *buf;
    posix_memalign(&buf, 16384, bytes);
    uint64_t *lat = calloc((size_t)count, sizeof *lat);
    uint64_t blocks = (uint64_t)st.st_size / bytes;
    for (int i = 0; i < count; i++) {
        off_t off = (off_t)(arc4random_uniform((uint32_t)(blocks > UINT32_MAX ? UINT32_MAX : blocks)) * bytes);
        uint64_t t0 = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
        ssize_t n = pread(fd, buf, bytes, off);
        lat[i] = clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - t0;
        if (n != (ssize_t)bytes) { fprintf(stderr, "short read at %lld: %zd\n", (long long)off, n); return 1; }
    }
    qsort(lat, (size_t)count, sizeof *lat, cmp);
    double sum = 0;
    for (int i = 0; i < count; i++) sum += (double)lat[i];
    printf("%d random %zu-byte reads (%s): mean %.2f ms, p50 %.2f ms, p90 %.2f ms, p99 %.2f ms, max %.2f ms\n",
           count, bytes, nocache ? "F_NOCACHE" : "cached", sum / count / 1e6, lat[count / 2] / 1e6,
           lat[count * 9 / 10] / 1e6, lat[count * 99 / 100] / 1e6, lat[count - 1] / 1e6);
    return 0;
}
