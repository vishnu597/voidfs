#include <stdio.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
int main(int argc, char **argv) { int r = renamex_np(argv[1], argv[2], RENAME_SWAP); printf("renamex_np RENAME_SWAP: %s\n", r ? strerror(errno) : "ok"); return 0; }
