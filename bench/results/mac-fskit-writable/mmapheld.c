// SPDX-License-Identifier: Apache-2.0
// cc -std=c11 -Wall -Wextra -Werror -o /tmp/mmapheld mmapheld.c
// mmapheld <new-file>: exclusively create a 32768-byte file, map it shared/RW,
// store generation 1, then emit "ready" while retaining the original descriptor.
// Send newline-terminated commands on stdin: store <uint64>, read, close, unmap,
// msync, sync, quit. "read" only uses the original descriptor; no command opens
// another descriptor. Stores cover bytes 0..127 and the first 128 bytes of the
// last host VM page. JSONL output is flushed after every record. Neither startup,
// stores, read, close, unmap nor EOF/quit cleanup calls msync, fsync or sync.
#define _DARWIN_C_SOURCE
#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

enum { FILE_SIZE = 32768, MARKER_SIZE = 128 };

static const char *file_path;
static unsigned char *mapping = MAP_FAILED;
static int file_fd = -1;
static size_t last_page_offset;
static uint64_t generation = 1;

static void json_string(const char *value) {
    const unsigned char *p = (const unsigned char *)value;
    putchar('"');
    for (; *p != '\0'; ++p) {
        switch (*p) {
        case '"': fputs("\\\"", stdout); break;
        case '\\': fputs("\\\\", stdout); break;
        case '\b': fputs("\\b", stdout); break;
        case '\f': fputs("\\f", stdout); break;
        case '\n': fputs("\\n", stdout); break;
        case '\r': fputs("\\r", stdout); break;
        case '\t': fputs("\\t", stdout); break;
        default:
            if (*p < 0x20) {
                printf("\\u%04x", (unsigned)*p);
            } else {
                putchar(*p);
            }
        }
    }
    putchar('"');
}

static void record_start(const char *phase) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) == -1) {
        perror("clock_gettime(CLOCK_MONOTONIC)");
        exit(EXIT_FAILURE);
    }
    uint64_t ns = (uint64_t)now.tv_sec * UINT64_C(1000000000)
        + (uint64_t)now.tv_nsec;
    fputs("{\"phase\":", stdout);
    json_string(phase);
    printf(",\"monotonic_ns\":%" PRIu64 ",\"pid\":%ld,\"path\":",
           ns, (long)getpid());
    json_string(file_path);
}

static void record_end(void) {
    fputs("}\n", stdout);
    if (fflush(stdout) == EOF) {
        perror("fflush(stdout)");
        exit(EXIT_FAILURE);
    }
}

static void result_fields(int64_t result, int error_number) {
    printf(",\"result\":%" PRId64 ",\"ok\":%s,\"errno\":%d,\"error\":",
           result, error_number == 0 ? "true" : "false", error_number);
    json_string(error_number == 0 ? "" : strerror(error_number));
}

static void syscall_record(const char *name, int64_t result, int error_number) {
    record_start(name);
    result_fields(result, error_number);
    record_end();
}

static void hex_string(const unsigned char *bytes, size_t count) {
    putchar('"');
    for (size_t i = 0; i < count; ++i) {
        printf("%02x", (unsigned)bytes[i]);
    }
    putchar('"');
}

static void make_marker(unsigned char bytes[MARKER_SIZE], unsigned region) {
    char header[96];
    for (size_t i = 0; i < MARKER_SIZE; ++i) {
        bytes[i] = (unsigned char)((generation >> ((i % 8) * 8))
            ^ (UINT64_C(0xa5) + i * 37 + region * 83));
    }
    int n = snprintf(header, sizeof header,
                     "VOIDFS_MMAPHELD generation=%020" PRIu64 " region=%u ",
                     generation, region);
    if (n < 0 || (size_t)n >= sizeof header || n >= MARKER_SIZE) {
        fputs("cannot format marker\n", stderr);
        exit(EXIT_FAILURE);
    }
    memcpy(bytes, header, (size_t)n);
}

static void store_marker(void) {
    if (mapping == MAP_FAILED) {
        syscall_record("store", -1, EINVAL);
        return;
    }
    unsigned char marker[MARKER_SIZE];
    make_marker(marker, 0);
    memcpy(mapping, marker, sizeof marker);
    make_marker(marker, 1);
    memcpy(mapping + last_page_offset, marker, sizeof marker);
    record_start("store");
    result_fields(0, 0);
    printf(",\"generation\":%" PRIu64 ",\"first_offset\":0,"
           "\"last_page_offset\":%zu,\"marker_size\":%d",
           generation, last_page_offset, MARKER_SIZE);
    record_end();
}

static ssize_t read_window(unsigned char bytes[MARKER_SIZE], off_t offset) {
    size_t count = 0;
    int error_number = 0;
    ssize_t result = 0;
    do {
        result = pread(file_fd, bytes + count, MARKER_SIZE - count,
                       offset + (off_t)count);
        if (result == -1) {
            if (errno == EINTR) {
                continue;
            }
            error_number = errno;
            break;
        }
        if (result == 0) {
            break;
        }
        count += (size_t)result;
    } while (count < MARKER_SIZE);
    record_start("pread");
    result_fields(result == -1 ? -1 : (int64_t)count, error_number);
    printf(",\"fd\":%d,\"offset\":%lld,\"requested\":%d,\"bytes_read\":%zu",
           file_fd, (long long)offset, MARKER_SIZE, count);
    record_end();
    return error_number == 0 ? (ssize_t)count : -1;
}

static void sample_fields(const unsigned char *bytes, ssize_t count,
                          const unsigned char expected[MARKER_SIZE]) {
    printf("{\"bytes\":%zd,\"matches_generation\":%s,\"hex\":", count,
           count == MARKER_SIZE && memcmp(bytes, expected, MARKER_SIZE) == 0
               ? "true" : "false");
    hex_string(bytes, count > 0 ? (size_t)count : 0);
    putchar('}');
}

static void read_markers(void) {
    unsigned char expected[2][MARKER_SIZE];
    unsigned char mapped[2][MARKER_SIZE] = {{0}};
    unsigned char kernel[2][MARKER_SIZE] = {{0}};
    ssize_t kernel_count[2];
    make_marker(expected[0], 0);
    make_marker(expected[1], 1);
    if (mapping != MAP_FAILED) {
        memcpy(mapped[0], mapping, MARKER_SIZE);
        memcpy(mapped[1], mapping + last_page_offset, MARKER_SIZE);
    }
    // Deliberately retain and use exactly the original descriptor. A failed read
    // after "close" is reported as EBADF; it never causes a replacement open.
    kernel_count[0] = read_window(kernel[0], 0);
    kernel_count[1] = read_window(kernel[1], (off_t)last_page_offset);
    record_start("read");
    printf(",\"generation\":%" PRIu64 ",\"fd\":%d,\"mapped\":%s,"
           "\"first_offset\":0,\"last_page_offset\":%zu,\"mapping_first\":",
           generation, file_fd, mapping != MAP_FAILED ? "true" : "false",
           last_page_offset);
    sample_fields(mapped[0], mapping == MAP_FAILED ? -1 : MARKER_SIZE, expected[0]);
    fputs(",\"mapping_last_page\":", stdout);
    sample_fields(mapped[1], mapping == MAP_FAILED ? -1 : MARKER_SIZE, expected[1]);
    fputs(",\"samefd_first\":", stdout);
    sample_fields(kernel[0], kernel_count[0], expected[0]);
    fputs(",\"samefd_last_page\":", stdout);
    sample_fields(kernel[1], kernel_count[1], expected[1]);
    record_end();
}

static void close_file(void) {
    int fd = file_fd;
    // Do not retry close on EINTR: the descriptor's state is unspecified.
    file_fd = -1;
    int result = close(fd);
    int error_number = result == -1 ? errno : 0;
    record_start("close");
    result_fields(result, error_number);
    printf(",\"fd\":%d", fd);
    record_end();
}

static void unmap_file(void) {
    int result;
    int error_number;
    if (mapping == MAP_FAILED) {
        result = -1;
        error_number = EINVAL;
    } else {
        result = munmap(mapping, FILE_SIZE);
        error_number = result == -1 ? errno : 0;
        if (result == 0) {
            mapping = MAP_FAILED;
        }
    }
    syscall_record("munmap", result, error_number);
}

static void protocol_error(const char *command) {
    record_start("command_error");
    result_fields(-1, EINVAL);
    fputs(",\"command\":", stdout);
    json_string(command);
    record_end();
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s <new-file>\n", argv[0]);
        return EXIT_FAILURE;
    }
    file_path = argv[1];
    errno = 0;
    long page_size = sysconf(_SC_PAGESIZE);
    if (page_size < MARKER_SIZE || page_size > FILE_SIZE) {
        syscall_record("sysconf_page_size", page_size,
                       page_size == -1 && errno != 0 ? errno : EINVAL);
        return EXIT_FAILURE;
    }
    last_page_offset = FILE_SIZE - (size_t)page_size;
    syscall_record("sysconf_page_size", page_size, 0);

    file_fd = open(file_path, O_RDWR | O_CREAT | O_EXCL, 0600);
    syscall_record("open", file_fd, file_fd == -1 ? errno : 0);
    if (file_fd == -1) {
        return EXIT_FAILURE;
    }
    int result = ftruncate(file_fd, FILE_SIZE);
    syscall_record("ftruncate", result, result == -1 ? errno : 0);
    if (result == -1) {
        close_file();
        return EXIT_FAILURE;
    }
    mapping = mmap(NULL, FILE_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED, file_fd, 0);
    syscall_record("mmap", mapping == MAP_FAILED ? -1 : 0,
                   mapping == MAP_FAILED ? errno : 0);
    if (mapping == MAP_FAILED) {
        close_file();
        return EXIT_FAILURE;
    }
    store_marker();
    record_start("ready");
    printf(",\"fd\":%d,\"file_size\":%d,\"page_size\":%ld,"
           "\"generation\":%" PRIu64 ",\"last_page_offset\":%zu,"
           "\"marker_size\":%d", file_fd, FILE_SIZE, page_size,
           generation, last_page_offset, MARKER_SIZE);
    record_end();

    char *line = NULL;
    size_t capacity = 0;
    int exit_status = EXIT_SUCCESS;
    for (;;) {
        errno = 0;
        ssize_t length = getline(&line, &capacity, stdin);
        if (length == -1) {
            if (ferror(stdin)) {
                syscall_record("stdin", -1, errno == 0 ? EIO : errno);
                exit_status = EXIT_FAILURE;
            } else {
                syscall_record("eof", 0, 0);
            }
            break;
        }
        while (length > 0 && (line[length - 1] == '\n' || line[length - 1] == '\r')) {
            line[--length] = '\0';
        }
        if (memchr(line, '\0', (size_t)length) != NULL) {
            protocol_error("embedded NUL");
        } else if (strncmp(line, "store ", 6) == 0) {
            char *end;
            errno = 0;
            uintmax_t value = strtoumax(line + 6, &end, 10);
            if (line[6] < '0' || line[6] > '9' || *end != '\0'
                || errno == ERANGE || value > UINT64_MAX || value == 0) {
                protocol_error(line);
            } else if (mapping == MAP_FAILED) {
                syscall_record("store", -1, EINVAL);
            } else {
                generation = (uint64_t)value;
                store_marker();
            }
        } else if (strcmp(line, "read") == 0) {
            read_markers();
        } else if (strcmp(line, "close") == 0) {
            close_file();
        } else if (strcmp(line, "unmap") == 0) {
            unmap_file();
        } else if (strcmp(line, "msync") == 0) {
            result = mapping == MAP_FAILED ? -1 : msync(mapping, FILE_SIZE, MS_SYNC);
            syscall_record("msync", result,
                           mapping == MAP_FAILED ? EINVAL : (result == -1 ? errno : 0));
        } else if (strcmp(line, "sync") == 0) {
            sync();
            // sync(2) returns void and has no error result on macOS.
            syscall_record("sync", 0, 0);
        } else if (strcmp(line, "quit") == 0) {
            syscall_record("quit", 0, 0);
            break;
        } else {
            protocol_error(line);
        }
    }
    free(line);
    if (mapping != MAP_FAILED) {
        unmap_file();
    }
    if (file_fd != -1) {
        close_file();
    }
    return exit_status;
}
