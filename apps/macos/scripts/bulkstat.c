// SPDX-License-Identifier: Apache-2.0
//
// bulkstat: lists a folder the way Finder does, with getattrlistbulk(2) (names, types, sizes,
// dates and ids in one call per batch), and prints how many entries it saw.
//
//     cc -O2 -o bulkstat bulkstat.c && bulkstat <folder>

#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/attr.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: bulkstat <folder>\n");
        return 2;
    }
    int fd = open(argv[1], O_RDONLY | O_DIRECTORY);
    if (fd < 0) { perror("open"); return 1; }
    struct attrlist al = {
        .bitmapcount = ATTR_BIT_MAP_COUNT,
        .commonattr = ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_NAME | ATTR_CMN_OBJTYPE | ATTR_CMN_MODTIME |
                      ATTR_CMN_FILEID | ATTR_CMN_FLAGS | ATTR_CMN_ACCESSMASK,
        .fileattr = ATTR_FILE_DATALENGTH,
    };
    static char buf[256 * 1024];
    long total = 0;
    for (;;) {
        int n = getattrlistbulk(fd, &al, buf, sizeof buf, 0);
        if (n < 0) { perror("getattrlistbulk"); return 1; }
        if (n == 0) break;
        char *p = buf;
        for (int i = 0; i < n; i++) {
            uint32_t len;  // each entry starts with its own length
            memcpy(&len, p, sizeof len);
            total++;
            p += len;
        }
    }
    printf("%ld entries\n", total);
    return 0;
}
