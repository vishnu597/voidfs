// SPDX-License-Identifier: Apache-2.0
//
// Large extended attribute fixtures and readback for bench/results/fskit-large-xattrs.
// cc -O2 -Wall -o xattr-probe xattr-probe.c
//
//   xattr-probe make <file> <name> <size> <seed>       new file: 43,008 data bytes, a 26-byte
//                                                       user.voidfs.sentinel, and <name> set whole
//   xattr-probe set <file> <name> <size> <seed> <how>  <how>: whole | chunks:<n> (positional
//                                                       fsetxattr, as copyfile_rsrc) | rsrcfd:<n>
//                                                       (write() to <file>/..namedfork/rsrc)
//   xattr-probe dump <file>                            data and every xattr: size, SHA-256, and
//                                                       for a fork, positional 1 MiB reads
//   xattr-probe value <size> <seed>                    the SHA-256 a value should have
//   xattr-probe pathconf <path>                        _PC_XATTR_SIZE_BITS and friends
//
// Every command prints one JSON line. Values are a splitmix64 stream, so a size and a seed name
// their bytes exactly.

#include <CommonCrypto/CommonDigest.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/xattr.h>
#include <unistd.h>

#define DATA_BYTES 43008
#define SENTINEL "user.voidfs.sentinel"
#define SENTINEL_VALUE "large-xattr-probe sentinel"
#define FORK_CHUNK (1 << 20)

static uint8_t *stream(size_t size, uint64_t seed) {
    uint8_t *b = malloc(size ? size : 1);
    if (!b) { perror("malloc"); exit(2); }
    uint64_t x = seed;
    for (size_t i = 0; i < size; i += 8) {
        x += 0x9e3779b97f4a7c15ULL;
        uint64_t z = x;
        z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
        z = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
        z ^= z >> 31;
        for (int k = 0; k < 8 && i + k < size; k++) b[i + k] = (uint8_t)(z >> (8 * k));
    }
    return b;
}

static void hex(const uint8_t *b, size_t n, char out[65]) {
    uint8_t d[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256(b, (CC_LONG)n, d);
    for (int i = 0; i < CC_SHA256_DIGEST_LENGTH; i++) sprintf(out + 2 * i, "%02x", d[i]);
}

static int is_fork(const char *name) { return strcmp(name, XATTR_RESOURCEFORK_NAME) == 0; }

static int set_value(int fd, const char *path, const char *name, const uint8_t *v, size_t size, const char *how) {
    if (strcmp(how, "whole") == 0) {
        int r = fsetxattr(fd, name, v, size, 0, 0);
        int e = r < 0 ? errno : 0;
        printf("{\"call\":\"fsetxattr\",\"name\":\"%s\",\"position\":0,\"bytes\":%zu,\"result\":%d,\"errno\":%d,\"error\":\"%s\"}\n",
               name, size, r, e, e ? strerror(e) : "");
        return e;
    }
    size_t chunk = 0;
    if (sscanf(how, "chunks:%zu", &chunk) == 1 && chunk > 0) {
        int first = 0;
        for (size_t at = 0; at < size || (size == 0 && at == 0); at += chunk) {
            size_t n = size - at < chunk ? size - at : chunk;
            int r = fsetxattr(fd, name, v + at, n, (uint32_t)at, 0);
            int e = r < 0 ? errno : 0;
            printf("{\"call\":\"fsetxattr\",\"name\":\"%s\",\"position\":%zu,\"bytes\":%zu,\"result\":%d,\"errno\":%d,\"error\":\"%s\"}\n",
                   name, at, n, r, e, e ? strerror(e) : "");
            if (e) { first = e; break; }
            if (size == 0) break;
        }
        return first;
    }
    if (sscanf(how, "rsrcfd:%zu", &chunk) == 1 && chunk > 0) {
        char rpath[4096];
        snprintf(rpath, sizeof rpath, "%s/..namedfork/rsrc", path);
        int rfd = open(rpath, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        int e = rfd < 0 ? errno : 0;
        printf("{\"call\":\"open-namedfork\",\"path\":\"%s\",\"result\":%d,\"errno\":%d,\"error\":\"%s\"}\n", rpath, rfd, e, e ? strerror(e) : "");
        if (e) return e;
        for (size_t at = 0; at < size; at += chunk) {
            size_t n = size - at < chunk ? size - at : chunk;
            ssize_t w = write(rfd, v + at, n);
            e = w < 0 ? errno : 0;
            printf("{\"call\":\"write-namedfork\",\"position\":%zu,\"bytes\":%zu,\"result\":%zd,\"errno\":%d,\"error\":\"%s\"}\n", at, n, w, e, e ? strerror(e) : "");
            if (e || (size_t)w != n) break;
        }
        int c = close(rfd);
        int ce = c < 0 ? errno : 0;
        printf("{\"call\":\"close-namedfork\",\"result\":%d,\"errno\":%d}\n", c, ce);
        return e ? e : ce;
    }
    fprintf(stderr, "unknown how: %s\n", how);
    exit(2);
}

static int cmd_make(const char *path, const char *name, size_t size, uint64_t seed) {
    int fd = open(path, O_RDWR | O_CREAT | O_EXCL, 0644);
    if (fd < 0) { perror(path); return 2; }
    uint8_t *data = stream(DATA_BYTES, seed ^ 0xda7aULL);
    if (write(fd, data, DATA_BYTES) != DATA_BYTES) { perror("write"); return 2; }
    free(data);
    if (fsetxattr(fd, SENTINEL, SENTINEL_VALUE, strlen(SENTINEL_VALUE), 0, 0) < 0) { perror("sentinel"); return 2; }
    uint8_t *v = stream(size, seed);
    int e = set_value(fd, path, name, v, size, "whole");
    free(v);
    fsync(fd);
    close(fd);
    return e ? 1 : 0;
}

static int cmd_set(const char *path, const char *name, size_t size, uint64_t seed, const char *how) {
    int fd = open(path, O_RDWR);
    if (fd < 0) { perror(path); return 2; }
    uint8_t *v = stream(size, seed);
    char h[65];
    hex(v, size, h);
    printf("{\"call\":\"expected\",\"name\":\"%s\",\"bytes\":%zu,\"sha256\":\"%s\"}\n", name, size, h);
    int e = set_value(fd, path, name, v, size, how);
    free(v);
    close(fd);
    return e ? 1 : 0;
}

static int cmd_dump(const char *path) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) { int e = errno; printf("{\"path\":\"%s\",\"open_errno\":%d,\"error\":\"%s\"}\n", path, e, strerror(e)); return 1; }
    struct stat st;
    fstat(fd, &st);
    uint8_t *data = malloc(st.st_size ? (size_t)st.st_size : 1);
    ssize_t got = pread(fd, data, (size_t)st.st_size, 0);
    char h[65];
    hex(data, got > 0 ? (size_t)got : 0, h);
    free(data);
    printf("{\"path\":\"%s\",\"dataBytes\":%lld,\"dataSha256\":\"%s\",\"xattrs\":{", path, (long long)st.st_size, h);
    ssize_t ln = flistxattr(fd, NULL, 0, 0);
    int first = 1;
    if (ln > 0) {
        char *names = malloc((size_t)ln);
        ln = flistxattr(fd, names, (size_t)ln, 0);
        for (char *n = names; ln > 0 && n < names + ln; n += strlen(n) + 1) {
            ssize_t size = fgetxattr(fd, n, NULL, 0, 0, 0);
            int se = size < 0 ? errno : 0;
            printf("%s\"%s\":{\"size\":%zd", first ? "" : ",", n, size);
            first = 0;
            if (se) { printf(",\"sizeErrno\":%d}", se); continue; }
            uint8_t *v = malloc(size ? (size_t)size : 1);
            ssize_t r = fgetxattr(fd, n, v, (size_t)size, 0, 0);
            int re = r < 0 ? errno : 0;
            hex(v, r > 0 ? (size_t)r : 0, h);
            printf(",\"read\":%zd,\"readErrno\":%d,\"sha256\":\"%s\"", r, re, re ? "" : h);
            if (is_fork(n)) {
                // copyfile_rsrc's pattern: chunks at advancing positions until a read returns 0.
                CC_SHA256_CTX c;
                CC_SHA256_Init(&c);
                uint8_t *buf = malloc(FORK_CHUNK);
                size_t at = 0;
                int pe = 0, reads = 0;
                for (;;) {
                    ssize_t k = fgetxattr(fd, n, buf, FORK_CHUNK, (uint32_t)at, 0);
                    reads++;
                    if (k < 0) { pe = errno; break; }
                    if (k == 0) break;
                    CC_SHA256_Update(&c, buf, (CC_LONG)k);
                    at += (size_t)k;
                    if (reads > 4096) break;
                }
                uint8_t d[CC_SHA256_DIGEST_LENGTH];
                CC_SHA256_Final(d, &c);
                for (int i = 0; i < CC_SHA256_DIGEST_LENGTH; i++) sprintf(h + 2 * i, "%02x", d[i]);
                printf(",\"positional\":{\"chunk\":%d,\"reads\":%d,\"bytes\":%zu,\"stopErrno\":%d,\"sha256\":\"%s\"}", FORK_CHUNK, reads, at, pe, h);
                free(buf);
            }
            printf("}");
            free(v);
        }
        free(names);
    } else if (ln < 0) {
        printf("\"_listErrno\":%d", errno);
    }
    printf("}}\n");
    close(fd);
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    if (argc == 6 && strcmp(argv[1], "make") == 0)
        return cmd_make(argv[2], argv[3], strtoull(argv[4], NULL, 10), strtoull(argv[5], NULL, 10));
    if (argc == 7 && strcmp(argv[1], "set") == 0)
        return cmd_set(argv[2], argv[3], strtoull(argv[4], NULL, 10), strtoull(argv[5], NULL, 10), argv[6]);
    if (argc == 3 && strcmp(argv[1], "dump") == 0) return cmd_dump(argv[2]);
    if (argc == 4 && strcmp(argv[1], "value") == 0) {
        size_t size = strtoull(argv[2], NULL, 10);
        uint8_t *v = stream(size, strtoull(argv[3], NULL, 10));
        char h[65];
        hex(v, size, h);
        printf("{\"bytes\":%zu,\"seed\":%s,\"sha256\":\"%s\"}\n", size, argv[3], h);
        return 0;
    }
    if (argc == 3 && strcmp(argv[1], "pathconf") == 0) {
        errno = 0;
        long bits = pathconf(argv[2], _PC_XATTR_SIZE_BITS);
        int e = errno;
        printf("{\"path\":\"%s\",\"_PC_XATTR_SIZE_BITS\":%ld,\"errno\":%d}\n", argv[2], bits, e);
        return 0;
    }
    fprintf(stderr, "usage: see the header of xattr-probe.c\n");
    return 2;
}
