#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec + t.tv_nsec / 1e9; }
int main(int argc, char **argv) { char b[4096]; memset(b, 1, sizeof b); int fd = open(argv[1], O_CREAT | O_RDWR | O_TRUNC, 0644); int n = 200;
  double t = now(); for (int i = 0; i < n; i++) { write(fd, b, sizeof b); fcntl(fd, F_FULLFSYNC); } double full = (now() - t) / n;
  t = now(); for (int i = 0; i < n; i++) { write(fd, b, sizeof b); fsync(fd); } double fs = (now() - t) / n;
  t = now(); for (int i = 0; i < n; i++) { write(fd, b, sizeof b); fcntl(fd, F_BARRIERFSYNC); } double bar = (now() - t) / n;
  printf("APFS 4 KiB append + F_FULLFSYNC %.2f ms, + fsync %.2f ms, + F_BARRIERFSYNC %.2f ms\n", full * 1e3, fs * 1e3, bar * 1e3); close(fd); unlink(argv[1]); return 0; }
