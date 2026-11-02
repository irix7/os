/* SPDX-License-Identifier: GPL-3.0-only
 *
 * Futex wait/wake on the raw native gate.
 *
 * The kernel's v0 thread gate has no pthreads; this probe drives
 * `SYS_FUTEX_WAIT` and `SYS_FUTEX_WAKE` through small assembly stubs
 * (`native.S`) and a static worker stack and TCB per job.  It links relibc for
 * `write`, `clock_gettime` and `_start`, so `main` runs with relibc's own
 * 16-byte TCB in `$28`.
 *
 * The probe proves the three futex outcomes the kernel supports: a woken
 * waiter returns `CELL_OK` and resumes, a mismatched expected value returns
 * `CELL_E_AGAIN` without blocking, and a wake with no waiter returns 0.  It
 * also proves the poll-rule bound: a negative timeout is a bounded wait, and
 * a wait that reaches its deadline re-checks the word, so a changed value is
 * `CELL_E_AGAIN` and an unchanged one is `CELL_E_TIMEDOUT`.
 *
 * A pass prints `FUTEX-OK` and exits 0.
 */

typedef unsigned int size_t;
typedef int ssize_t;

struct tcb {
    unsigned int self;
    unsigned int dtv;
    int errno_value;
    unsigned int libc_data;
};

_Static_assert(sizeof(struct tcb) == 16, "n32 TCB is 16 bytes");

struct timespec {
    long long tv_sec;
    int tv_nsec;
    int _pad;
};

/* CELL_E_* status values from the native gate. */
#define CELL_OK 0
#define CELL_E_AGAIN 6
#define CELL_E_FAULT 9
#define CELL_E_INVAL 12
#define CELL_E_TIMEDOUT 19

#define CLOCK_MONOTONIC 1

/* The kernel bounds every wait by the 100 ms poll limit. */
#define POLL_BOUND_MS 100
#define TIMEOUT_MS 20

extern ssize_t write(int, const void *, size_t);
extern int clock_gettime(int, struct timespec *);
extern long __irix7_thread_create(unsigned long entry, unsigned long arg,
                                  unsigned long stack_top, unsigned long tcb);
extern long __irix7_yield(void);
extern void __irix7_thread_exit(long status) __attribute__((noreturn));
extern long __irix7_futex_wait(unsigned long addr, unsigned long expected,
                               long timeout_ms);
extern long __irix7_futex_wake(unsigned long addr, unsigned long count);

/* The gate status of the last thread_create or futex call, written by native.S. */
int __irix7_thread_status;
int __irix7_futex_status;

static int failures;
static int checks;

static unsigned int empty_word;
static unsigned int single_word;
static unsigned int pair_word;
static unsigned int timeout_word;
static unsigned int changed_word;
static unsigned int solo_word;

struct job {
    unsigned long addr;
    unsigned int expected;
    int timeout_ms;
    volatile int entered;
    volatile int done;
    volatile int result;
    _Alignas(16) unsigned char stack[8192];
    struct tcb tcb;
};

static struct job single_job;
static struct job pair_job_a;
static struct job pair_job_b;
static struct job timeout_job;
static struct job changed_job;

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

static void check(int condition, const char *name) {
    checks++;
    if (!condition) {
        put_text("FUTEX-PROBE: check=FAIL ");
        put_text(name);
        put_text("\n");
        failures++;
    }
}

static long long monotonic_ms(void) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) {
        return -1;
    }
    return now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

static int wait_status(unsigned long addr, unsigned int expected, int timeout_ms) {
    (void)__irix7_futex_wait(addr, expected, (long)timeout_ms);
    return __irix7_futex_status;
}

static void init_job(struct job *job, unsigned long addr, unsigned int expected,
                     int timeout_ms) {
    job->addr = addr;
    job->expected = expected;
    job->timeout_ms = timeout_ms;
    job->entered = 0;
    job->done = 0;
    job->result = -1;
    job->tcb.self = 0;
    job->tcb.dtv = 0;
    job->tcb.errno_value = 0;
    job->tcb.libc_data = 0;
}

/* The new thread's entry point.  `$ra` is zero, so the worker must retire
 * through the native exit call rather than return. */
static void worker(void *argument) {
    struct job *job = (struct job *)argument;
    job->entered = 1;
    job->result = wait_status(job->addr, job->expected, job->timeout_ms);
    job->done = 1;
    __irix7_thread_exit(0);
}

static int start_job(struct job *job) {
    unsigned long stack_top =
        ((unsigned long)job->stack + sizeof(job->stack)) & ~(unsigned long)15;
    long tid = __irix7_thread_create((unsigned long)worker, (unsigned long)job,
                                     stack_top, (unsigned long)&job->tcb);
    return tid != 0 && __irix7_thread_status == CELL_OK;
}

/* Yield until `flag` is set.  The cap turns a stuck probe into a failure rather
 * than a hang the harness cannot diagnose. */
static void run_until(volatile int *flag) {
    unsigned long spins = 0;
    while (!*flag && spins < 2000000) {
        (void)__irix7_yield();
        spins++;
    }
}

int main(void) {
    long long before;
    long long elapsed;

    failures = 0;
    checks = 0;
    empty_word = 0;
    single_word = 0;
    pair_word = 0;
    timeout_word = 0;
    changed_word = 0;
    solo_word = 0x5EED;

    put_text("FUTEX-PROBE: start\n");

    /* A wake with nothing parked on the address is not an error. */
    check(__irix7_futex_wake((unsigned long)&empty_word, 1) == 0 &&
              __irix7_futex_status == CELL_OK,
          "empty wake returns 0");
    check(__irix7_futex_wake((unsigned long)&empty_word, 0) == 0 &&
              __irix7_futex_status == CELL_OK,
          "a zero count wakes nothing");

    /* The compare happens before the thread parks. */
    check(wait_status((unsigned long)&empty_word, 1, -1) == CELL_E_AGAIN,
          "mismatched expected is EAGAIN");
    check(wait_status((unsigned long)&empty_word, 1, 0) == CELL_E_AGAIN,
          "mismatched expected beats a zero timeout");
    check(wait_status(2, 0, 0) == CELL_E_INVAL, "unaligned address is EINVAL");
    check(wait_status(0x0F000000, 0, 0) == CELL_E_FAULT,
          "unmapped address is EFAULT");

    /* A single waiter: main sets the word and wakes it, then yields. */
    init_job(&single_job, (unsigned long)&single_word, 0, -1);
    check(start_job(&single_job), "single waiter created");
    (void)__irix7_yield();
    check(single_job.entered == 1 && single_job.done == 0,
          "the worker parked in futex_wait");
    single_word = 1;
    check(__irix7_futex_wake((unsigned long)&single_word, 1) == 1 &&
              __irix7_futex_status == CELL_OK,
          "wake returns one waiter");
    check(single_job.done == 0, "the woken worker has not run yet");
    (void)__irix7_yield();
    check(single_job.done == 1 && single_job.result == CELL_OK,
          "the woken worker resumed with CELL_OK");
    check(__irix7_futex_wake((unsigned long)&single_word, 1) == 0 &&
              __irix7_futex_status == CELL_OK,
          "wake after the waiter returned is 0");

    /* Two waiters on one word: the count limits how many are resumed. */
    init_job(&pair_job_a, (unsigned long)&pair_word, 0, -1);
    init_job(&pair_job_b, (unsigned long)&pair_word, 0, -1);
    check(start_job(&pair_job_a), "first pair waiter created");
    (void)__irix7_yield();
    check(pair_job_a.entered == 1 && pair_job_a.done == 0,
          "the first pair waiter parked");
    check(start_job(&pair_job_b), "second pair waiter created");
    (void)__irix7_yield();
    check(pair_job_b.entered == 1 && pair_job_b.done == 0,
          "the second pair waiter parked");
    check(__irix7_futex_wake((unsigned long)&pair_word, 1) == 1 &&
              __irix7_futex_status == CELL_OK,
          "a count of one wakes one of two waiters");
    (void)__irix7_yield();
    check(pair_job_a.done == 1 && pair_job_a.result == CELL_OK,
          "the first pair waiter resumed");
    check(pair_job_b.done == 0, "the second pair waiter stayed parked");
    check(__irix7_futex_wake((unsigned long)&pair_word, 1) == 1 &&
              __irix7_futex_status == CELL_OK,
          "the second wake returns the remaining waiter");
    (void)__irix7_yield();
    check(pair_job_b.done == 1 && pair_job_b.result == CELL_OK,
          "the second pair waiter resumed");
    check(__irix7_futex_wake((unsigned long)&pair_word, 1) == 0 &&
              __irix7_futex_status == CELL_OK,
          "both pair waiters have returned");

    /* A bounded wait with no waker expires with the word unchanged. */
    init_job(&timeout_job, (unsigned long)&timeout_word, 0, TIMEOUT_MS);
    check(start_job(&timeout_job), "timeout waiter created");
    (void)__irix7_yield();
    check(timeout_job.entered == 1 && timeout_job.done == 0,
          "the timeout waiter parked");
    run_until(&timeout_job.done);
    check(timeout_job.done == 1 && timeout_job.result == CELL_E_TIMEDOUT,
          "an unwoken waiter times out with CELL_E_TIMEDOUT");

    /* A wait that reaches its deadline re-checks the word: a changed value is
     * reported as EAGAIN even though no wake was sent. */
    init_job(&changed_job, (unsigned long)&changed_word, 0, TIMEOUT_MS);
    check(start_job(&changed_job), "changed-value waiter created");
    (void)__irix7_yield();
    check(changed_job.entered == 1 && changed_job.done == 0,
          "the changed-value waiter parked");
    changed_word = 1;
    run_until(&changed_job.done);
    check(changed_job.done == 1 && changed_job.result == CELL_E_AGAIN,
          "a changed word without a wake is EAGAIN");

    /* No thread is left to wake this one: a negative timeout is bounded by the
     * poll limit instead of hanging the machine. */
    before = monotonic_ms();
    check(wait_status((unsigned long)&solo_word, 0x5EED, -1) == CELL_E_TIMEDOUT,
          "a negative timeout with no peer is bounded");
    elapsed = monotonic_ms() - before;
    check(elapsed >= POLL_BOUND_MS - 10 && elapsed < 5000,
          "the no-peer bound is about 100 ms");

    before = monotonic_ms();
    check(wait_status((unsigned long)&solo_word, 0x5EED, 0) == CELL_E_TIMEDOUT,
          "a zero timeout probes the matching word");
    elapsed = monotonic_ms() - before;
    check(elapsed < 50, "a zero timeout does not park");

    put_text("FUTEX-PROBE: checks=");
    put_dec((unsigned long)checks);
    put_text(" failures=");
    put_dec((unsigned long)failures);
    put_text("\n");

    if (failures == 0) {
        put_text("FUTEX-OK\n");
    } else {
        put_text("FUTEX-FAILED\n");
    }
    return failures == 0 ? 0 : 1;
}
