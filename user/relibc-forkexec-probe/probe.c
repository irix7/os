// SPDX-License-Identifier: GPL-3.0-only
//
// Fork/exec/wait probe for the native cell process model.
//
// The parent forks, checks that the child has not run yet, waits, and receives
// status 7 from a second image executed through the ramfs path `/bin/child`.
// It also checks descriptor inheritance and close-on-exec across exec.
//
// The marker `FORK-EXEC-OK` is printed only when every check passes.

typedef unsigned int size_t;
typedef int ssize_t;

#define O_RDONLY 0
#define WNOHANG 1
#define F_GETFD 1
#define F_SETFD 2
#define FD_CLOEXEC 1

extern int fork(void);
extern int execve(const char *path, char *const argv[], char *const envp[]);
extern int waitpid(int pid, int *status, int options);
extern int getpid(void);
extern int getppid(void);
extern int open(const char *path, int flags, ...);
extern ssize_t read(int fd, void *buf, size_t count);
extern ssize_t write(int fd, const void *buf, size_t count);
extern long lseek(int fd, long offset, int whence);
extern int fcntl(int fd, int cmd, ...);
extern int sched_yield(void);
extern void _exit(int status) __attribute__((noreturn));

extern char **environ;

static int failures;

static void out(const char *text, size_t length) {
    (void)write(1, text, length);
}

static void check(int condition, const char *message) {
    if (condition) {
        return;
    }
    failures += 1;
    out("FORK-PROBE-FAIL: ", 17);
    size_t length = 0;
    while (message[length] != 0) {
        length += 1;
    }
    out(message, length);
    out("\n", 1);
}

int main(void) {
    char buffer[64];
    char *argv[2];
    int status = -1;

    check(getpid() == 1, "parent is not pid 1");
    check(getppid() == 0, "parent has a parent");

    int inherited = open("/hello.txt", O_RDONLY);
    int cloexec = open("/hello.txt", O_RDONLY);
    check(inherited == 3, "inherited fd is not 3");
    check(cloexec == 4, "close-on-exec fd is not 4");
    check(fcntl(cloexec, F_SETFD, FD_CLOEXEC) == 0, "F_SETFD failed");

    int pid = fork();
    if (pid == 0) {
        // Child: the descriptor table was copied, close-on-exec is still per
        // slot, and exec replaces the image.
        argv[0] = (char *)"/bin/child";
        argv[1] = 0;
        execve("/bin/child", argv, environ);
        _exit(127);
    }

    check(pid > 1, "fork did not return a child pid");

    // The kernel is cooperative: the child cannot have run while the parent
    // was runnable, so a no-hang probe must report zero.
    int probe = waitpid(pid, &status, WNOHANG);
    check(probe == 0, "WNOHANG reaped a child that had not run");

    int waited = waitpid(pid, &status, 0);
    check(waited == pid, "waitpid returned the wrong pid");
    check((status & 0x7f) == 0, "child did not exit normally");
    check(((status >> 8) & 0xff) == 7, "child status is not 7");

    // The child and parent share the open file description, so the child's
    // read moved the offset.  Rewind before reading the fixture again.
    check(lseek(inherited, 0, 0) == 0, "cannot rewind the inherited file");
    ssize_t count = read(inherited, buffer, sizeof(buffer));
    check(count == 28, "parent lost the inherited descriptor");

    // A second child exits before the parent waits, so the parent reaps a
    // zombie rather than being woken inside exit.  The child inherits the
    // same descriptors; the exit and reap must release exactly one child
    // reference and leave the parent's descriptor usable.
    int late = fork();
    if (late == 0) {
        _exit(9);
    }
    check(late > 1, "second fork did not return a child pid");
    int late_status = -1;
    int late_waited = 0;
    for (int attempt = 0; attempt < 1000 && late_waited == 0; attempt++) {
        (void)sched_yield();
        late_waited = waitpid(late, &late_status, WNOHANG);
    }
    check(late_waited == late, "waitpid did not reap the late child");
    check((late_status & 0x7f) == 0, "late child did not exit normally");
    check(((late_status >> 8) & 0xff) == 9, "late child status is not 9");
    check(lseek(inherited, 0, 0) == 0, "cannot rewind after the late child");
    count = read(inherited, buffer, sizeof(buffer));
    check(count == 28, "parent lost the descriptor after the late child");

    if (failures == 0) {
        out("FORK-EXEC-OK\n", 13);
        return 0;
    }
    out("FORK-EXEC-FAILED\n", 17);
    return 1;
}
