// SPDX-License-Identifier: GPL-3.0-only
//
// Per-cell sleep probe: a sleeping parent must let a runnable child run.
//
// The parent creates a non-blocking pipe, forks, and sleeps for 20 ms.  The
// child increments a counter, writes the final value to the pipe and exits
// with status 42.  Because the parent is asleep and the child is runnable, the
// kernel must switch cells at the sleep rather than park only the parent's
// thread.  On return the parent checks:
//
// - the pipe already holds the child's counter value, so the child made
//   progress before the parent resumed;
// - the value is exactly the loop count, so the child finished its work;
// - the parent's own clock shows at least the 20 ms deadline;
// - the child had already exited, so `waitpid(WNOHANG)` reaps it and its
//   status is 42.
//
// A pass prints `SLEEP-CELL-OK` and exits 0.  This probe has one image: the
// child is a fork of the parent, not a second executable.

typedef unsigned int size_t;
typedef int ssize_t;

#define CLOCK_MONOTONIC 1
#define O_NONBLOCK 0o4000
#define WNOHANG 1

/* Loop count the child must complete and report.  Keep the child's run time
 * far below the 20 ms sleep: the point is that it ran at all, not that it is
 * CPU-bound. */
#define CHILD_COUNT 2000UL
/* The parent's sleep; the child has the whole interval to run. */
#define SLEEP_MS 20
/* No-hang bound for a heavily loaded JIT host; the lower bound is the check. */
#define ELAPSED_LIMIT_MS 60000
#define CHILD_STATUS 42

struct timespec {
    long long tv_sec;
    int tv_nsec;
    int _pad;
};

_Static_assert(sizeof(struct timespec) == 16, "n32 timespec is 16 bytes");

extern int pipe2(int fds[2], int flags);
extern int fork(void);
extern int waitpid(int pid, int *status, int options);
extern int nanosleep(const struct timespec *request, struct timespec *remaining);
extern int clock_gettime(int clock, struct timespec *now);
extern int close(int fd);
extern ssize_t read(int fd, void *buf, size_t count);
extern ssize_t write(int fd, const void *buf, size_t count);
extern int getpid(void);
extern void _exit(int status) __attribute__((noreturn));

static int failures;
static int checks;

static unsigned long text_length(const char *text) {
    unsigned long length = 0;
    while (text[length] != 0) {
        length++;
    }
    return length;
}

static void put_text(const char *text) {
    (void)write(1, text, text_length(text));
}

static void put_dec(unsigned long value) {
    char buffer[20];
    int length = 0;
    if (value == 0) {
        put_text("0");
        return;
    }
    while (value != 0 && length < (int)sizeof(buffer)) {
        buffer[length++] = (char)('0' + (value % 10));
        value /= 10;
    }
    for (int index = 0; index < length / 2; index++) {
        char swap = buffer[index];
        buffer[index] = buffer[length - 1 - index];
        buffer[length - 1 - index] = swap;
    }
    (void)write(1, buffer, (size_t)length);
}

static long long monotonic_ms(void) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) {
        return -1;
    }
    return now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

static void check(int condition, const char *name) {
    checks++;
    if (!condition) {
        put_text("SLEEP-CELL-PROBE: check=FAIL ");
        put_text(name);
        put_text("\n");
        failures++;
    }
}

/* Parse the decimal counter the child writes.  Returns 0 on bad input. */
static unsigned long parse_dec(const char *buffer, int length) {
    unsigned long value = 0;
    for (int index = 0; index < length; index++) {
        if (buffer[index] < '0' || buffer[index] > '9') {
            return 0;
        }
        value = value * 10 + (unsigned long)(buffer[index] - '0');
    }
    return value;
}

int main(void) {
    int fds[2];
    int status = -1;

    check(getpid() == 1, "parent is not pid 1");
    check(pipe2(fds, O_NONBLOCK) == 0, "pipe2 failed");

    int pid = fork();
    if (pid == 0) {
        // Child: the pipe write end is inherited through the copied
        // descriptor table.  The read end is not needed.
        (void)close(fds[0]);
        unsigned long counter = 0;
        for (unsigned long index = 0; index < CHILD_COUNT; index++) {
            counter += 1;
        }
        char digits[20];
        int length = 0;
        unsigned long value = counter;
        do {
            digits[length++] = (char)('0' + (value % 10));
            value /= 10;
        } while (value != 0 && length < (int)sizeof(digits));
        for (int index = 0; index < length / 2; index++) {
            char swap = digits[index];
            digits[index] = digits[length - 1 - index];
            digits[length - 1 - index] = swap;
        }
        // Report only through the pipe.  Serial output would burn a large
        // part of the parent's 20 ms sleep on the emulated 38400-baud line.
        (void)write(fds[1], digits, (size_t)length);
        _exit(CHILD_STATUS);
    }

    check(pid > 1, "fork did not return a child pid");
    (void)close(fds[1]);

    // Measure the sleep itself, not fork plus sleep: an early return must not
    // be hidden by the time spent setting up the child.
    long long before_ms = monotonic_ms();
    struct timespec interval = {0, SLEEP_MS * 1000000, 0};
    check(nanosleep(&interval, 0) == 0, "nanosleep failed");
    long long after_ms = monotonic_ms();
    long long elapsed_ms = (before_ms >= 0 && after_ms >= before_ms)
                               ? after_ms - before_ms
                               : -1;

    // The child must have written before the parent woke.  A blocking read
    // would hide the bug, so the pipe is O_NONBLOCK: a missing value is
    // EAGAIN, not a wait that lets the child run late.
    char buffer[32];
    ssize_t count = read(fds[0], buffer, sizeof(buffer));
    check(count > 0, "child wrote no counter before the parent woke");
    unsigned long child_counter =
        count > 0 ? parse_dec(buffer, (int)count) : 0;
    check(child_counter == CHILD_COUNT, "child counter did not reach the loop count");

    check(elapsed_ms >= SLEEP_MS, "parent resumed before its sleep deadline");
    check(elapsed_ms < ELAPSED_LIMIT_MS, "parent sleep overran the relaxed bound");

    // The child exited during the sleep, so it is already a zombie and
    // WNOHANG reaps it without another scheduling point.
    int reaped = waitpid(pid, &status, WNOHANG);
    if (reaped == 0) {
        // Keep the failure reportable and do not leave a live child behind.
        reaped = waitpid(pid, &status, 0);
        check(0, "child had not exited when the parent woke");
    }
    check(reaped == pid, "waitpid returned the wrong pid");
    check((status & 0x7f) == 0, "child did not exit normally");
    check(((status >> 8) & 0xff) == CHILD_STATUS, "child status is not 42");

    put_text("SLEEP-CELL-PROBE: counter=");
    put_dec(child_counter);
    put_text(" expected=");
    put_dec(CHILD_COUNT);
    put_text(" elapsed=");
    put_dec(elapsed_ms < 0 ? 0UL : (unsigned long)elapsed_ms);
    put_text("ms checks=");
    put_dec((unsigned long)checks);
    put_text(" failures=");
    put_dec((unsigned long)failures);
    put_text("\n");

    if (failures == 0) {
        put_text("SLEEP-CELL-OK\n");
        return 0;
    }
    put_text("SLEEP-CELL-FAILED\n");
    return 1;
}
