// SPDX-License-Identifier: Apache-2.0
//
// atomic: an atomic save over a file another descriptor holds open. Writes doc.txt, opens it,
// then saves new bytes the way most editors do (write save.tmp, fsync, rename over doc.txt), and
// reads both: the earlier descriptor must keep the old bytes, a new open the new ones.
//
//     cc -O2 -o atomic atomic.c && atomic <folder on the volume>

#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static char path[1024], tmp[1024];

static void put(const char *p, const char *s) {
    int fd = open(p, O_CREAT | O_TRUNC | O_WRONLY, 0644);
    if (fd < 0 || write(fd, s, strlen(s)) != (ssize_t)strlen(s) || fsync(fd) < 0) perror(p);
    close(fd);
}

static void show(const char *what, int fd) {
    char b[64] = {0};
    ssize_t n = pread(fd, b, sizeof b - 1, 0);
    printf("  %-44s %s\n", what, n < 0 ? "read failed" : b);
}

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: atomic <folder>\n"); return 2; }
    snprintf(path, sizeof path, "%s/doc.txt", argv[1]);
    snprintf(tmp, sizeof tmp, "%s/save.tmp", argv[1]);
    put(path, "old version");
    int old = open(path, O_RDONLY);
    put(tmp, "new version");
    printf("  %-44s %s\n", "rename(save.tmp, doc.txt) over doc.txt", rename(tmp, path) == 0 ? "ok" : "failed");
    show("the descriptor opened before the save reads", old);
    int fresh = open(path, O_RDONLY);
    show("a new open reads", fresh);
    printf("  %-44s %s\n", "save.tmp after", access(tmp, F_OK) == 0 ? "still there" : "gone");
    close(old);
    close(fresh);
    return 0;
}
