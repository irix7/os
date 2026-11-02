/* SPDX-License-Identifier: GPL-3.0-only
 *
 * nanosleep and clock_nanosleep on the cooperative thread gate.
 *
 * This is a relibc-linked n32 C program with no system headers.  `nanosleep`
 * and `clock_nanosleep` reach the kernel through the relibc `irix7-pal`
 * mapping; `SYS_NANOSLEEP` parks the calling thread on a CP0 Count deadline
 * and switches to a runnable peer instead of busy-waiting.
 *
 * The probe proves that:
 *
 * - a worker that sleeps 20 ms does not stop main: main sits in a
 *   yield-and-increment loop and the counter has advanced when the worker
 *   wakes;
 * - the worker's own clock shows at least the requested interval, so the
 *   kernel did not return early;
 * - a thread that sleeps with no peer still returns at its deadline, through
 *   the kernel's timer wait rather than a spin;
 * - `pthread_join` on a sleeping worker completes after the sleep deadline,
 *   which the kernel's `futex: no-peer` line shows was cut from the 100 ms
 *   poll bound to the sleeper's earlier deadline;
 * - `clock_nanosleep` works relative and absolute, and an uninterrupted sleep
 *   leaves the caller's remaining-time word untouched.
 *
 * A pass prints `SLEEP-OK` and exits 0.
 */

typedef unsigned int size_t;
typedef int ssize_t;

typedef void *pthread_t;

struct timespec {
    long long tv_sec;
    int tv_nsec;
    int _pad;
};

_Static_assert(sizeof(struct timespec) == 16, "n32 timespec is 16 bytes");

#define CLOCK_REALTIME 0
#define CLOCK_MONOTONIC 1
#define TIMER_ABSTIME 1

#define EINVAL 22

/* The interval the worker and the solo sleeper request. */
#define SLEEP_MS 20
/* The join rounds use a shorter sleep; three rounds keep the fastest timing
 * usable when the emulator host is loaded. */
#define JOIN_SLEEP_MS 15
/* Generous upper bound for a short sleep on a loaded JIT host. */
#define ELAPSED_LIMIT_MS 1000
/* A join that hung on the futex poll bound would still finish; this bound is
 * a no-hang check, and the serial transcript shows which deadline ended it. */
#define JOIN_LIMIT_MS 1000

extern ssize_t write(int, const void *, size_t);
extern int clock_gettime(int, struct timespec *);
extern int nanosleep(const struct timespec *, struct timespec *);
extern int clock_nanosleep(int, int, const struct timespec *, struct timespec *);
extern int pthread_create(pthread_t *, const void *, void *(*)(void *), void *);
extern int pthread_join(pthread_t, void **);
extern int sched_yield(void);
extern int *__errno_location(void);

#define errno (*__errno_location())

static int failures;
static int checks;

static volatile unsigned long counter;
static volatile int worker_woke;
static volatile int worker_return;

struct job {
    int sleep_ms;
    volatile int started;
    volatile int slept;
    long long before_ms;
    long long after_ms;
    long long elapsed_ms;
};

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

static void put_signed(long long value) {
    if (value < 0) {
        put_text("-");
        put_dec((unsigned long)(-value));
    } else {
        put_dec((unsigned long)value);
    }
}

static void check(int condition, const char *name) {
    checks++;
    if (!condition) {
        put_text("SLEEP-PROBE: check=FAIL ");
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

static struct timespec make_interval(long long millis) {
    struct timespec interval;
    interval.tv_sec = millis / 1000;
    interval.tv_nsec = (int)((millis % 1000) * 1000000);
    interval._pad = 0;
    return interval;
}

/* Add `millis` to a CLOCK_MONOTONIC reading. */
static struct timespec add_millis(struct timespec base, long long millis) {
    struct timespec result = base;
    result.tv_sec += millis / 1000;
    result.tv_nsec += (int)((millis % 1000) * 1000000);
    if (result.tv_nsec >= 1000000000) {
        result.tv_nsec -= 1000000000;
        result.tv_sec += 1;
    }
    return result;
}

/* The new thread's entry point.  `$ra` is zero, so the worker must retire
 * through the pthread return path rather than return to a caller. */
static void *sleeping_worker(void *argument) {
    struct job *job = (struct job *)argument;

    job->started = 1;
    job->before_ms = monotonic_ms();
    (void)nanosleep(&(struct timespec){job->sleep_ms / 1000,
                                       (job->sleep_ms % 1000) * 1000000, 0},
                    0);
    job->after_ms = monotonic_ms();
    job->elapsed_ms = (job->before_ms >= 0 && job->after_ms >= job->before_ms)
                          ? job->after_ms - job->before_ms
                          : -1;
    job->slept = 1;
    worker_woke = 1;
    return (void *)&worker_return;
}

static int start_job(struct job *job, int sleep_ms, pthread_t *thread) {
    job->sleep_ms = sleep_ms;
    job->started = 0;
    job->slept = 0;
    job->before_ms = -1;
    job->after_ms = -1;
    job->elapsed_ms = -1;
    return pthread_create(thread, 0, sleeping_worker, job) == 0;
}

int main(void) {
    struct timespec before_deadline;
    struct timespec absolute_deadline;
    struct timespec past_deadline;
    struct timespec untouched;
    pthread_t thread;
    struct job worker;
    struct job joined;
    void *joined_value = 0;
    long long before;
    long long elapsed;
    long long worker_elapsed;
    long long join_elapsed;

    failures = 0;
    checks = 0;
    counter = 0;
    worker_woke = 0;

    put_text("SLEEP-PROBE: start\n");

    /* nanosleep validates its interval before it reaches the kernel. */
    {
        struct timespec invalid = make_interval(SLEEP_MS);
        invalid.tv_nsec = 1000000000;
        errno = 0;
        check(nanosleep(&invalid, 0) == -1 && errno == EINVAL,
              "a nanosecond field of 1e9 is EINVAL");
        invalid = make_interval(SLEEP_MS);
        invalid.tv_sec = -1;
        errno = 0;
        check(nanosleep(&invalid, 0) == -1 && errno == EINVAL,
              "a negative interval is EINVAL");
    }

    /* A zero interval returns without parking. */
    {
        struct timespec zero = make_interval(0);
        before = monotonic_ms();
        check(nanosleep(&zero, 0) == 0, "a zero interval succeeds");
        elapsed = monotonic_ms() - before;
        check(elapsed >= 0 && elapsed < 200, "a zero interval does not park");
    }

    /* No peer exists yet, so this sleep is the kernel's timer-wait path. */
    before = monotonic_ms();
    check(nanosleep(&(struct timespec){0, SLEEP_MS * 1000000, 0}, 0) == 0,
          "a solo sleep succeeds");
    elapsed = monotonic_ms() - before;
    check(elapsed >= SLEEP_MS, "a solo sleep lasts at least its interval");
    check(elapsed < ELAPSED_LIMIT_MS, "a solo sleep returns near its deadline");
    put_text("SLEEP-PROBE: solo-elapsed=");
    put_signed(elapsed);
    put_text("ms\n");

    /* A worker sleeps while main makes progress and yields to it. */
    check(start_job(&worker, SLEEP_MS, &thread), "sleeping worker created");
    before = monotonic_ms();
    {
        unsigned long spins = 0;
        while (worker.slept == 0 && spins < 4000000) {
            counter++;
            (void)sched_yield();
            spins++;
        }
    }
    elapsed = monotonic_ms() - before;
    check(pthread_join(thread, &joined_value) == 0, "pthread_join the worker");
    check(joined_value == (void *)&worker_return,
          "the worker returned its completion flag");
    worker_elapsed = worker.elapsed_ms;
    check(worker.slept == 1 && worker_woke == 1, "the worker woke and set its flag");
    check(worker.before_ms >= 0 && worker.after_ms >= worker.before_ms,
          "the worker read CLOCK_MONOTONIC on its own stack");
    check(worker_elapsed >= SLEEP_MS, "the worker slept at least its interval");
    check(worker_elapsed < ELAPSED_LIMIT_MS,
          "the worker resumed near its deadline");
    check(counter > 0, "main ran while the worker slept");
    check(elapsed >= SLEEP_MS, "the sleep spanned the requested interval");
    check(elapsed < ELAPSED_LIMIT_MS, "the sleep did not overrun its bound");
    put_text("SLEEP-PROBE: counter=");
    put_dec(counter);
    put_text(" elapsed=");
    put_signed(elapsed);
    put_text("ms worker-elapsed=");
    put_signed(worker_elapsed);
    put_text("ms worker-before=");
    put_signed(worker.before_ms);
    put_text(" worker-after=");
    put_signed(worker.after_ms);
    put_text("\n");

    /* Joining a sleeping worker must be woken by the sleeper's deadline
     * rather than by the 100 ms futex poll bound.  The timer is delivered by
     * a host thread, so host load jitters the timing; repeat the round and
     * report the fastest join.  The transcript's `futex: no-peer` line,
     * followed by a park with the sleeper's remaining budget, is the direct
     * evidence that the deadline ended the wait. */
    join_elapsed = -1;
    for (int round = 0; round < 3; round++) {
        check(start_job(&joined, JOIN_SLEEP_MS, &thread),
              "joined worker created");
        before = monotonic_ms();
        check(pthread_join(thread, &joined_value) == 0, "join immediately");
        elapsed = monotonic_ms() - before;
        check(joined_value == (void *)&worker_return,
              "the joined worker returned its completion flag");
        check(joined.slept == 1 && joined.elapsed_ms >= JOIN_SLEEP_MS,
              "the joined worker slept its interval");
        check(joined.elapsed_ms < JOIN_LIMIT_MS,
              "the joined worker resumed promptly");
        check(elapsed >= JOIN_SLEEP_MS, "the join waited for the sleeper");
        if (join_elapsed < 0 || elapsed < join_elapsed) {
            join_elapsed = elapsed;
        }
    }
    check(join_elapsed < JOIN_LIMIT_MS, "the join returned promptly");
    put_text("SLEEP-PROBE: join-elapsed=");
    put_signed(join_elapsed);
    put_text("ms\n");

    /* clock_nanosleep: relative, absolute, past, and rejected arguments. */
    before = monotonic_ms();
    check(clock_nanosleep(CLOCK_REALTIME, 0,
                          &(struct timespec){0, 10 * 1000000, 0}, 0) == 0,
          "a relative CLOCK_REALTIME sleep succeeds");
    elapsed = monotonic_ms() - before;
    check(elapsed >= 10 && elapsed < ELAPSED_LIMIT_MS,
          "a relative clock_nanosleep lasts its interval");

    check(clock_gettime(CLOCK_MONOTONIC, &before_deadline) == 0,
          "read CLOCK_MONOTONIC for an absolute deadline");
    absolute_deadline = add_millis(before_deadline, 10);
    before = monotonic_ms();
    check(clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &absolute_deadline,
                          0) == 0,
          "an absolute CLOCK_MONOTONIC sleep succeeds");
    elapsed = monotonic_ms() - before;
    check(elapsed >= 10 && elapsed < ELAPSED_LIMIT_MS,
          "an absolute clock_nanosleep waits until its deadline");

    past_deadline = before_deadline;
    past_deadline.tv_sec -= 1;
    before = monotonic_ms();
    check(clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &past_deadline, 0) == 0,
          "an absolute deadline in the past succeeds");
    elapsed = monotonic_ms() - before;
    check(elapsed < 200, "a past absolute deadline returns at once");

    /* No signals exist, so an uninterrupted sleep must leave the remaining
     * time alone. */
    untouched = make_interval(4321);
    check(nanosleep(&(struct timespec){0, 5 * 1000000, 0}, &untouched) == 0,
          "a nanosleep with a remaining-time pointer succeeds");
    check(untouched.tv_sec == 4 && untouched.tv_nsec == 321000000,
          "an uninterrupted nanosleep leaves the remaining time untouched");

    untouched = make_interval(7);
    check(clock_nanosleep(CLOCK_MONOTONIC, 0x10, &untouched, 0) == EINVAL,
          "an unknown flag is EINVAL");
    check(clock_nanosleep(9, 0, &untouched, 0) == EINVAL,
          "an unknown clock is EINVAL");
    check(clock_nanosleep(CLOCK_MONOTONIC, 0,
                          &(struct timespec){0, 1000000000, 0},
                          0) == EINVAL,
          "an out-of-range interval is EINVAL");

    put_text("SLEEP-PROBE: checks=");
    put_dec((unsigned long)checks);
    put_text(" failures=");
    put_dec((unsigned long)failures);
    put_text("\n");

    if (failures == 0) {
        put_text("SLEEP-OK\n");
    } else {
        put_text("SLEEP-FAILED\n");
    }
    return failures == 0 ? 0 : 1;
}
