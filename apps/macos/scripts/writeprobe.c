// SPDX-License-Identifier: Apache-2.0
//
// writeprobe: when does a write on a mounted volume reach the file system module? Each step prints
// its wall-clock time and how long the call took, so that a module whose write callback is slowed
// (step 5, item 4 measured with a `writedelay` option of the voidfs module, since removed) shows
// which calls wait for it: a write(2) that takes the delay was written through; one that returns
// at once left dirty pages in the kernel, and the call that takes the delay later (fsync, close,
// msync, munmap, sync) is the one that pushed them.
//
//     cc -O2 -o writeprobe writeprobe.c && writeprobe <folder on the volume> <test>
//
// Tests: write (write(2), then fsync, F_FULLFSYNC, close), close (write then close), sync
// (write then sync(2)), mmap (shared writable map: store, msync, munmap, close), mmapclose (map,
// close the descriptor, then store, msync and munmap), unflushed (write, then wait without
// closing, for a kill), dirty (map, store, wait without msync, for a kill).

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec / 1e9;
}

static void stamp(const char *what, double took, int rc) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    struct tm tm;
    localtime_r(&tv.tv_sec, &tm);
    printf("%02d:%02d:%02d.%03d  %-28s %8.1f ms  %s\n", tm.tm_hour, tm.tm_min, tm.tm_sec, (int)(tv.tv_usec / 1000), what, took * 1000,
           rc < 0 ? strerror(errno) : "ok");
    fflush(stdout);
}

#define TIME(what, call) do { double t0 = now(); int rc_ = (int)(call); stamp(what, now() - t0, rc_); } while (0)

static char path[1200];

static int fresh(const char *dir, const char *name) {
    snprintf(path, sizeof path, "%s/%s", dir, name);
    unlink(path);
    int fd = -1;
    TIME("open O_CREAT|O_RDWR", fd = open(path, O_CREAT | O_RDWR, 0644));
    return fd;
}

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: writeprobe <folder> <write|close|sync|mmap|mmapclose|unflushed|dirty> [seconds]\n"); return 2; }
    const char *dir = argv[1], *test = argv[2];
    int wait = argc > 3 ? atoi(argv[3]) : 30;
    char block[4096];
    memset(block, 'w', sizeof block);
    int fd;
    if (!strcmp(test, "write")) {
        fd = fresh(dir, "probe-write");
        TIME("write 4 KiB", write(fd, block, sizeof block));
        TIME("write 4 KiB again", write(fd, block, sizeof block));
        TIME("fsync", fsync(fd));
        TIME("write 4 KiB", write(fd, block, sizeof block));
        TIME("fcntl F_FULLFSYNC", fcntl(fd, F_FULLFSYNC));
        TIME("write 4 KiB", write(fd, block, sizeof block));
        TIME("close", close(fd));
    } else if (!strcmp(test, "close")) {
        fd = fresh(dir, "probe-close");
        TIME("write 4 KiB", write(fd, block, sizeof block));
        TIME("close", close(fd));
    } else if (!strcmp(test, "sync")) {
        fd = fresh(dir, "probe-sync");
        TIME("write 4 KiB", write(fd, block, sizeof block));
        TIME("sync(2)", (sync(), 0));
        TIME("close", close(fd));
    } else if (!strcmp(test, "mmap") || !strcmp(test, "mmapclose") || !strcmp(test, "dirty")) {
        fd = fresh(dir, !strcmp(test, "mmap") ? "probe-mmap" : !strcmp(test, "dirty") ? "probe-dirty" : "probe-mmapclose");
        TIME("ftruncate 64 KiB", ftruncate(fd, 65536));
        char *m = MAP_FAILED;
        TIME("mmap MAP_SHARED rw", (m = mmap(NULL, 65536, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0)) == MAP_FAILED ? -1 : 0);
        if (m == MAP_FAILED) { close(fd); return 1; }
        if (!strcmp(test, "mmapclose")) TIME("close (mapping kept)", close(fd));
        memcpy(m, "mapped page one", 15);
        memcpy(m + 40000, "mapped page ten", 15);
        stamp("stored into two pages", 0, 0);
        if (!strcmp(test, "dirty")) {
            printf("waiting %d s with dirty mapped pages (pid %d)\n", wait, getpid());
            fflush(stdout);
            sleep(wait);
            return 0;
        }
        sleep(1);
        TIME("msync MS_SYNC", msync(m, 65536, MS_SYNC));
        memcpy(m + 8192, "mapped page three", 17);
        stamp("stored into a third page", 0, 0);
        TIME("munmap", munmap(m, 65536));
        if (!strcmp(test, "mmap")) TIME("close", close(fd));
    } else if (!strcmp(test, "mmapread")) {
        // A store through a mapping whose descriptor is closed, unmapped without msync, then
        // read through a new descriptor before the kernel pushed it, and again later.
        fd = fresh(dir, "probe-mmapread");
        TIME("ftruncate 64 KiB", ftruncate(fd, 65536));
        char *m = MAP_FAILED;
        TIME("mmap MAP_SHARED rw", (m = mmap(NULL, 65536, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0)) == MAP_FAILED ? -1 : 0);
        if (m == MAP_FAILED) { close(fd); return 1; }
        TIME("close (mapping kept)", close(fd));
        memcpy(m + 20000, "mapped and unmapped", 19);
        TIME("munmap without msync", munmap(m, 65536));
        for (int pass = 0; pass < 2; pass++) {
            char page[65536];
            int r = open(path, O_RDONLY);
            ssize_t n = r < 0 ? -1 : pread(r, page, sizeof page, 0);
            if (r >= 0) close(r);
            stamp(n == 65536 && !memcmp(page + 20000, "mapped and unmapped", 19) ? "read: the store is there" : "read: the store is MISSING", 0, n < 0 ? -1 : 0);
            if (pass == 0) sleep(wait);
        }
    } else if (!strcmp(test, "unflushed")) {
        fd = fresh(dir, "probe-unflushed");
        for (int i = 0; i < 16; i++) write(fd, block, sizeof block);
        stamp("wrote 64 KiB, not closed", 0, 0);
        printf("waiting %d s with the file open (pid %d)\n", wait, getpid());
        fflush(stdout);
        sleep(wait);
        TIME("close", close(fd));
    } else {
        fprintf(stderr, "unknown test %s\n", test);
        return 2;
    }
    return 0;
}
