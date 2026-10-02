// SPDX-License-Identifier: Apache-2.0
//
// semantics: checks, in a scratch folder on a mounted volume, the file system behaviour Mac apps
// depend on: extended attributes and AppleDouble files, atomic-save renames, swap and exclusive
// renames, exchangedata, clones, full fsync, shared writable mmap, locks, open-unlink, links.
// Also prints the capabilities the volume advertises, which apps read to pick a save strategy.
//
//     cc -O2 -o semantics semantics.c && semantics /Volumes/Something/scratch-folder

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/attr.h>
#include <sys/clonefile.h>
#include <sys/file.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/xattr.h>
#include <unistd.h>

static char dir[1024];

static const char *p(const char *name) {
    static char buf[4][1200];
    static int i;
    i = (i + 1) % 4;
    snprintf(buf[i], sizeof buf[i], "%s/%s", dir, name);
    return buf[i];
}

static void put(const char *name, const char *text) {
    int fd = open(p(name), O_CREAT | O_TRUNC | O_WRONLY, 0644);
    if (fd < 0) { printf("  create %s: %s\n", name, strerror(errno)); return; }
    write(fd, text, strlen(text));
    close(fd);
}

/// Reads a small file. Rotates buffers like p(), so that two calls can share one printf: with a
/// single buffer, "a=%s b=%s" printed whichever file was read last, twice.
static const char *get(const char *name) {
    static char buf[4][64];
    static int i;
    i = (i + 1) % 4;
    int fd = open(p(name), O_RDONLY);
    if (fd < 0) return strerror(errno);
    ssize_t n = read(fd, buf[i], sizeof buf[i] - 1);
    close(fd);
    buf[i][n < 0 ? 0 : n] = 0;
    return buf[i];
}

static int exists(const char *name) {
    struct stat st;
    return lstat(p(name), &st) == 0;
}

/// Starts a check from a known state: a.txt holds "A", b.txt holds "B", nothing else.
static void fresh(void) {
    const char *names[] = {"a.txt", "b.txt", "c.txt", "._a.txt", "._b.txt", "clone.txt", "hard.txt", "sym.txt", "save.tmp"};
    for (size_t i = 0; i < sizeof names / sizeof *names; i++) unlink(p(names[i]));
    put("a.txt", "A");
    put("b.txt", "B");
}

static void result(const char *what, int rc) {
    printf("  %-52s %s\n", what, rc == 0 ? "ok" : strerror(errno));
}

static void capabilities(const char *path) {
    struct attrlist al = {.bitmapcount = ATTR_BIT_MAP_COUNT, .volattr = ATTR_VOL_INFO | ATTR_VOL_CAPABILITIES};
    struct {
        uint32_t length;
        vol_capabilities_attr_t caps;
    } __attribute__((packed)) buf;
    if (getattrlist(path, &al, &buf, sizeof buf, 0) != 0) {
        printf("  getattrlist(ATTR_VOL_CAPABILITIES): %s\n", strerror(errno));
        return;
    }
    struct { const char *name; int set; uint32_t bit; } caps[] = {
        {"CASE_SENSITIVE", VOL_CAPABILITIES_FORMAT, VOL_CAP_FMT_CASE_SENSITIVE},
        {"PERSISTENTOBJECTIDS", VOL_CAPABILITIES_FORMAT, VOL_CAP_FMT_PERSISTENTOBJECTIDS},
        {"SYMBOLICLINKS", VOL_CAPABILITIES_FORMAT, VOL_CAP_FMT_SYMBOLICLINKS},
        {"HARDLINKS", VOL_CAPABILITIES_FORMAT, VOL_CAP_FMT_HARDLINKS},
        {"2TB_FILESIZE", VOL_CAPABILITIES_FORMAT, VOL_CAP_FMT_2TB_FILESIZE},
        {"EXTENDED_ATTR", VOL_CAPABILITIES_INTERFACES, VOL_CAP_INT_EXTENDED_ATTR},
        {"NAMEDSTREAMS", VOL_CAPABILITIES_INTERFACES, VOL_CAP_INT_NAMEDSTREAMS},
        {"EXCHANGEDATA", VOL_CAPABILITIES_INTERFACES, VOL_CAP_INT_EXCHANGEDATA},
        {"RENAME_SWAP", VOL_CAPABILITIES_INTERFACES, VOL_CAP_INT_RENAME_SWAP},
        {"RENAME_EXCL", VOL_CAPABILITIES_INTERFACES, VOL_CAP_INT_RENAME_EXCL},
        {"CLONE", VOL_CAPABILITIES_INTERFACES, VOL_CAP_INT_CLONE},
        {"FLOCK", VOL_CAPABILITIES_INTERFACES, VOL_CAP_INT_FLOCK},
        {"ADVLOCK", VOL_CAPABILITIES_INTERFACES, VOL_CAP_INT_ADVLOCK},
        {"ALLOCATE", VOL_CAPABILITIES_INTERFACES, VOL_CAP_INT_ALLOCATE},
    };
    printf("  advertised:");
    for (size_t i = 0; i < sizeof caps / sizeof *caps; i++) {
        int valid = (buf.caps.valid[caps[i].set] & caps[i].bit) != 0;
        int on = (buf.caps.capabilities[caps[i].set] & caps[i].bit) != 0;
        printf(" %s=%s", caps[i].name, !valid ? "?" : on ? "yes" : "no");
    }
    printf("\n");
}

int main(int argc, char **argv) {
    if (argc != 2) { fprintf(stderr, "usage: semantics <scratch folder>\n"); return 2; }
    snprintf(dir, sizeof dir, "%s", argv[1]);
    char parent[1024];
    snprintf(parent, sizeof parent, "%s", dir);
    char *slash = strrchr(parent, '/');
    if (slash && slash != parent) *slash = 0;
    struct statfs sf;
    if (statfs(parent, &sf) == 0) printf("%s (%s, %s)\n", parent, sf.f_fstypename, sf.f_mntfromname);
    capabilities(parent);
    if (mkdir(dir, 0755) != 0 && errno != EEXIST) {
        printf("  mkdir %s: %s\n", dir, strerror(errno));
        return 0;
    }

    fresh();
    result("setxattr user.test", setxattr(p("a.txt"), "user.test", "1", 1, 0, 0));
    printf("  %-52s %s\n", "  AppleDouble ._a.txt created?", exists("._a.txt") ? "yes" : "no");
    char finder[32] = {'T', 'E', 'X', 'T', 't', 't', 'x', 't'};
    result("setxattr com.apple.FinderInfo (32 bytes)", setxattr(p("b.txt"), XATTR_FINDERINFO_NAME, finder, 32, 0, 0));
    result("setxattr com.apple.ResourceFork", setxattr(p("b.txt"), XATTR_RESOURCEFORK_NAME, "rsrc", 4, 0, 0));
    printf("  %-52s %s\n", "  AppleDouble ._b.txt created?", exists("._b.txt") ? "yes" : "no");

    fresh();
    put("save.tmp", "A2");
    result("atomic save: rename(save.tmp, a.txt) over a.txt", rename(p("save.tmp"), p("a.txt")));
    printf("  %-52s a=%s\n", "  after", get("a.txt"));

    fresh();
    result("renamex_np(a, b, RENAME_SWAP)", renamex_np(p("a.txt"), p("b.txt"), RENAME_SWAP));
    printf("  %-52s a=%s b=%s\n", "  after (a swap leaves a=B b=A)", get("a.txt"), get("b.txt"));

    fresh();
    result("renamex_np(a, b, RENAME_EXCL) onto existing b", renamex_np(p("a.txt"), p("b.txt"), RENAME_EXCL));
    printf("  %-52s a=%s b=%s\n", "  after (EEXIST leaves a=A b=B)", get("a.txt"), get("b.txt"));

    fresh();
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    result("exchangedata(a, b)", exchangedata(p("a.txt"), p("b.txt"), 0));
#pragma clang diagnostic pop
    printf("  %-52s a=%s b=%s\n", "  after", get("a.txt"), get("b.txt"));

    fresh();
    result("clonefile(a, clone)", clonefile(p("a.txt"), p("clone.txt"), 0));

    fresh();
    int fd = open(p("a.txt"), O_RDWR);
    result("fcntl F_FULLFSYNC", fcntl(fd, F_FULLFSYNC));
    result("fcntl F_BARRIERFSYNC", fcntl(fd, F_BARRIERFSYNC));
    result("flock LOCK_EX|LOCK_NB", flock(fd, LOCK_EX | LOCK_NB));
    struct flock fl = {.l_type = F_WRLCK, .l_whence = SEEK_SET};
    result("fcntl F_SETLK write lock", fcntl(fd, F_SETLK, &fl));
    ftruncate(fd, 4096);
    char *m = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (m == MAP_FAILED) {
        result("mmap MAP_SHARED PROT_WRITE", -1);
    } else {
        memcpy(m, "mapped", 6);
        result("mmap MAP_SHARED write + msync(MS_SYNC)", msync(m, 4096, MS_SYNC));
        munmap(m, 4096);
    }
    close(fd);
    printf("  %-52s %.6s\n", "  a.txt after the mapped write", get("a.txt"));

    fresh();
    put("gone.txt", "still readable");
    fd = open(p("gone.txt"), O_RDONLY);
    result("unlink while open", unlink(p("gone.txt")));
    char buf[32] = {0};
    ssize_t n = pread(fd, buf, sizeof buf - 1, 0);
    printf("  %-52s %s\n", "  read from the open descriptor", n > 0 ? buf : strerror(errno));
    close(fd);
    result("link (hard link)", link(p("a.txt"), p("hard.txt")));
    result("symlink", symlink("a.txt", p("sym.txt")));
    result("chmod 0600", chmod(p("a.txt"), 0600));
    result("open O_EXLOCK", (fd = open(p("b.txt"), O_RDONLY | O_EXLOCK | O_NONBLOCK)) < 0 ? -1 : close(fd));
    return 0;
}
