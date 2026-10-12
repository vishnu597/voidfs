// SPDX-License-Identifier: Apache-2.0
//
// locks: advisory locks on a volume, each kind on its own file, taken by this process and then
// tried from a child: fcntl F_SETLK (what SQLite uses), flock, and open O_EXLOCK. A lock the
// volume keeps makes the child's try fail with EAGAIN (EWOULDBLOCK) while the parent holds it, and
// succeed once the parent lets go.
//
//     cc -O2 -o locks locks.c && locks <folder on the volume>

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/file.h>
#include <sys/wait.h>
#include <unistd.h>

static char dir[1024];

static int fresh(const char *name) {
    char p[1100];
    snprintf(p, sizeof p, "%s/%s", dir, name);
    unlink(p);
    return open(p, O_CREAT | O_RDWR, 0644);
}

static const char *rc(int r) { return r < 0 ? strerror(errno) : "ok"; }

static int posix(int fd, short type) {
    struct flock fl = {.l_type = type, .l_whence = SEEK_SET};
    return fcntl(fd, F_SETLK, &fl);
}

// The child's try, reported by its exit status: 0 it got the lock, 1 it didn't.
static void child(const char *what, int (*try)(const char *)) {
    pid_t pid = fork();
    if (pid == 0) _exit(try(what) < 0 ? 1 : 0);
    int status;
    waitpid(pid, &status, 0);
    printf("  %-44s %s\n", what, WEXITSTATUS(status) ? "refused (held)" : "granted");
}

static int try_posix(const char *what) {
    (void)what;
    char p[1100];
    snprintf(p, sizeof p, "%s/posix.lock", dir);
    int fd = open(p, O_RDWR);
    return posix(fd, F_WRLCK);
}

static int try_flock(const char *what) {
    (void)what;
    char p[1100];
    snprintf(p, sizeof p, "%s/flock.lock", dir);
    return flock(open(p, O_RDWR), LOCK_EX | LOCK_NB);
}

static int try_exlock(const char *what) {
    (void)what;
    char p[1100];
    snprintf(p, sizeof p, "%s/exlock.lock", dir);
    return open(p, O_RDWR | O_EXLOCK | O_NONBLOCK);
}

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: locks <folder>\n"); return 2; }
    snprintf(dir, sizeof dir, "%s", argv[1]);

    int fd = fresh("posix.lock");
    printf("  %-44s %s\n", "fcntl F_SETLK write lock", rc(posix(fd, F_WRLCK)));
    child("another process's F_SETLK while held", try_posix);
    printf("  %-44s %s\n", "fcntl F_SETLK unlock", rc(posix(fd, F_UNLCK)));
    child("another process's F_SETLK after unlock", try_posix);
    close(fd);

    fd = fresh("flock.lock");
    printf("  %-44s %s\n", "flock LOCK_EX|LOCK_NB", rc(flock(fd, LOCK_EX | LOCK_NB)));
    child("another process's flock while held", try_flock);
    flock(fd, LOCK_UN);
    child("another process's flock after LOCK_UN", try_flock);
    close(fd);

    close(fresh("exlock.lock"));
    char p[1100];
    snprintf(p, sizeof p, "%s/exlock.lock", dir);
    fd = open(p, O_RDWR | O_EXLOCK | O_NONBLOCK);
    printf("  %-44s %s\n", "open O_EXLOCK", rc(fd));
    child("another process's O_EXLOCK while held", try_exlock);
    close(fd);
    child("another process's O_EXLOCK after close", try_exlock);
    return 0;
}
