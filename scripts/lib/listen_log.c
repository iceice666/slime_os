/*
 * Records every listen() a process tree calls, for scripts/check/check-foxglove-bridge.py.
 *
 * The checker builds this into a shared library with the host's cc and loads it into the bridge with
 * DYLD_INSERT_LIBRARIES on macOS or LD_PRELOAD on Linux. Every process that inherits the environment
 * loads it too, so a listener opened by Python's socket module or by a native library such as zenoh's
 * is recorded. It changes no call's behaviour: each listen() runs as it would, and its outcome is
 * appended, as one line, to the file SLIME_LISTEN_LOG names:
 *
 *     listen pid=PID port=PORT ok=OK
 *
 * PORT is the socket's bound port, or -1 if it has no IPv4 or IPv6 address; OK is 1 if listen()
 * succeeded and 0 if it failed. One write() per line with O_APPEND keeps lines from two processes whole.
 * A program that calls the system directly, or that clears the environment before it starts a child,
 * is not seen; the checker requires the bridge's own servers to be seen on every honest run.
 */

#ifndef __APPLE__
#define _GNU_SOURCE
#include <dlfcn.h>
#endif
#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <unistd.h>

static void note(int fd, int ok) {
    const char *path = getenv("SLIME_LISTEN_LOG");
    if (path == NULL) {
        return;
    }
    struct sockaddr_storage address;
    socklen_t size = sizeof address;
    int port = -1;
    if (getsockname(fd, (struct sockaddr *)&address, &size) == 0) {
        if (address.ss_family == AF_INET) {
            port = ntohs(((struct sockaddr_in *)&address)->sin_port);
        } else if (address.ss_family == AF_INET6) {
            port = ntohs(((struct sockaddr_in6 *)&address)->sin6_port);
        }
    }
    char line[96];
    int length = snprintf(line, sizeof line, "listen pid=%d port=%d ok=%d\n", (int)getpid(), port, ok);
    int out = open(path, O_WRONLY | O_APPEND | O_CREAT, 0600);
    if (out >= 0) {
        (void)write(out, line, (size_t)length);
        close(out);
    }
}

#ifdef __APPLE__
static int recorded_listen(int fd, int backlog) {
    int result = listen(fd, backlog);
    note(fd, result == 0);
    return result;
}

__attribute__((used)) static struct {
    const void *replacement;
    const void *original;
} interposers[] __attribute__((section("__DATA,__interpose"))) = {
    {(const void *)recorded_listen, (const void *)listen},
};
#else
int listen(int fd, int backlog) {
    static int (*original)(int, int);
    if (original == NULL) {
        original = (int (*)(int, int))dlsym(RTLD_NEXT, "listen");
    }
    int result = original(fd, backlog);
    note(fd, result == 0);
    return result;
}
#endif
