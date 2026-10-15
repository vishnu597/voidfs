// SPDX-License-Identifier: Apache-2.0
// cc -std=c11 -Wall -Wextra -Werror -O2 -o /tmp/apfs-xattr-sizes apfs-xattr-sizes.c
// apfs-xattr-sizes <new-file>: creates the file, prints its _PC_XATTR_SIZE_BITS, then sets
// com.apple.ResourceFork and user.big to values of 65,537 bytes up to 1 GiB, one at a time,
// reads each one's size back and removes it. Deletes the file at the end. Needs about 1 GiB
// of memory and, briefly, of disk.
#define _DARWIN_C_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/xattr.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s <new-file>\n", argv[0]);
        return 2;
    }
    int fd = open(argv[1], O_CREAT | O_EXCL | O_RDWR, 0644);
    if (fd < 0) {
        perror("open");
        return 1;
    }
    printf("pathconf _PC_XATTR_SIZE_BITS = %ld\n", fpathconf(fd, _PC_XATTR_SIZE_BITS));
    const long sizes[] = {65537, 1L << 20, 16L << 20, (16L << 20) + 1, 64L << 20, 128L << 20, 256L << 20, 1L << 30};
    const char *names[] = {"com.apple.ResourceFork", "user.big"};
    int failed = 0;
    for (size_t n = 0; n < sizeof names / sizeof *names; n++) {
        for (size_t i = 0; i < sizeof sizes / sizeof *sizes; i++) {
            long size = sizes[i];
            char *value = malloc((size_t)size);
            if (!value) {
                printf("%-24s %11ld bytes: no memory\n", names[n], size);
                failed = 1;
                continue;
            }
            for (long j = 0; j < size; j++) value[j] = (char)(j * 31 + 7);
            int set = fsetxattr(fd, names[n], value, (size_t)size, 0, 0);
            int error = errno;
            long back = set == 0 ? (long)fgetxattr(fd, names[n], NULL, 0, 0, 0) : -1;
            printf("%-24s %11ld bytes: set %s%s, size read back %ld\n", names[n], size, set == 0 ? "ok" : "failed: ",
                   set == 0 ? "" : strerror(error), back);
            if (set != 0 || back != size) failed = 1;
            if (set == 0) fremovexattr(fd, names[n], 0);
            free(value);
        }
    }
    close(fd);
    unlink(argv[1]);
    return failed;
}
