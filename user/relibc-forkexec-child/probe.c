// SPDX-License-Identifier: GPL-3.0-only
//
// Second image for the fork/exec probe.
//
// It inherits fd 3 (a ramfs file) and fd 4 marked close-on-exec.  It prints
// `CHILD-OK`, verifies the inherited descriptor set, and exits 7 so the parent
// can check the wait status.

typedef unsigned int size_t;
typedef int ssize_t;

#define F_GETFD 1

extern int getpid(void);
extern int getppid(void);
extern ssize_t read(int fd, void *buf, size_t count);
extern ssize_t write(int fd, const void *buf, size_t count);
extern int fcntl(int fd, int cmd, ...);

static int failures;

static void out(const char *text, size_t length) {
    (void)write(1, text, length);
}

static void check(int condition, const char *message) {
    if (condition) {
        return;
    }
    failures += 1;
    out("CHILD-FAIL: ", 12);
    size_t length = 0;
    while (message[length] != 0) {
        length += 1;
    }
    out(message, length);
    out("\n", 1);
}

int main(void) {
    char buffer[64];

    check(getpid() == 2, "child pid is not 2");
    check(getppid() == 1, "child parent is not 1");

    ssize_t count = read(3, buffer, sizeof(buffer));
    check(count == 28, "inherited fd 3 is not the fixture file");
    check(fcntl(4, F_GETFD, 0) == -1, "fd 4 survived close-on-exec");

    out("CHILD-OK\n", 9);
    return failures == 0 ? 7 : 9;
}
