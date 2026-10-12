// mmapseq <file> <steps>: steps run in order. m map (64 KiB, shared rw), c close the fd, o open a
// second fd (kept), x close it, 1-9 store marker N into page 0 (offset 1000*N), S msync, u munmap,
// r read through a new fd and report which markers are there, w sleep 2 s, y sync(2).
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
int main(int argc, char **argv) {
    const char *path = argv[1], *steps = argv[2];
    unlink(path);
    int fd = open(path, O_CREAT | O_RDWR, 0644), fd2 = -1;
    ftruncate(fd, 65536);
    char *m = NULL; int stored = 0;
    for (const char *p = steps; *p; p++) {
        char mark[16];
        switch (*p) {
        case 'm': m = mmap(NULL, 65536, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0); break;
        case 'c': close(fd); fd = -1; break;
        case 'o': fd2 = open(path, O_RDWR); break;
        case 'x': close(fd2); fd2 = -1; break;
        case 'S': msync(m, 65536, MS_SYNC); break;
        case 'u': munmap(m, 65536); m = NULL; break;
        case 'w': sleep(2); break;
        case 'y': sync(); break;
        case 'r': {
            char page[65536]; int r = open(path, O_RDONLY); ssize_t n = pread(r, page, sizeof page, 0); close(r);
            printf("read after \"%.*s\":", (int)(p - steps), steps);
            for (int i = 1; i <= 9; i++) if (stored & (1 << i)) { snprintf(mark, sizeof mark, "marker%d", i); printf(" %d%s", i, n == 65536 && !memcmp(page + 1000 * i, mark, 7) ? "" : "(MISSING)"); }
            printf("\n"); break;
        }
        default:
            if (*p >= '1' && *p <= '9') { int i = *p - '0'; snprintf(mark, sizeof mark, "marker%d", i); memcpy(m + 1000 * i, mark, 7); stored |= 1 << i; }
        }
    }
    return 0;
}
