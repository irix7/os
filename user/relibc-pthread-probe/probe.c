/* SPDX-License-Identifier: GPL-3.0-only
 *
 * pthreads on the cooperative thread and futex gate.
 *
 * This is a relibc-linked n32 C program with no system headers.  The opaque
 * pthread types are the relibc layout from `src/header/bits_threads`:
 * `pthread_t` is a pointer to relibc's Pthread record, `pthread_key_t` is an
 * unsigned long and `pthread_mutex_t` is 12 bytes.  The declarations below are
 * the public POSIX surface the probe exercises.
 *
 * Main and a worker both increment a shared counter under one static-initialised
 * pthread mutex and yield between rounds, so the lock really contends.  The
 * worker stores a pthread-specific value with a destructor, returns a different
 * value from its start routine, and records that its errno is private.  Main
 * joins, checks the returned value, the destructor, the counter and that its
 * own key map and errno were untouched.  A pass prints PTHREADS-OK and exits 0.
 */

typedef unsigned int size_t;
typedef int ssize_t;

typedef void *pthread_t;
typedef unsigned long pthread_key_t;

typedef union {
    unsigned char size[12];
    unsigned int align;
} pthread_mutex_t;

#define PTHREAD_MUTEX_INITIALIZER {{0}}

#define ROUNDS 16
#define WORKER_ERRNO 77
#define MAIN_ERRNO 55

extern ssize_t write(int, const void *, size_t);
extern int *__errno_location(void);

extern int pthread_create(pthread_t *, const void *, void *(*)(void *), void *);
extern int pthread_join(pthread_t, void **);
extern pthread_t pthread_self(void);
extern int pthread_equal(pthread_t, pthread_t);
extern int pthread_mutex_lock(pthread_mutex_t *);
extern int pthread_mutex_unlock(pthread_mutex_t *);
extern int pthread_key_create(pthread_key_t *, void (*)(void *));
extern int pthread_setspecific(pthread_key_t, const void *);
extern void *pthread_getspecific(pthread_key_t);
extern int sched_yield(void);

#define errno (*__errno_location())

static int failures;
static int checks;

static pthread_mutex_t counter_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_key_t worker_key;
static pthread_t main_thread;
static pthread_t worker_thread;
static pthread_t worker_self_before;
static pthread_t worker_self_after;

static volatile unsigned long counter;
static volatile int worker_argument_ok;
static volatile int worker_specific_ok;
static volatile int worker_errno_ok;
static volatile int worker_destructor_runs;

static int worker_argument = 0x5EED;
static int worker_return = 0xC0DE;

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
        put_text("PTHREAD-PROBE: check=FAIL ");
        put_text(name);
        put_text("\n");
        failures++;
    }
}

static void worker_key_destructor(void *value) {
    if (value == (void *)&worker_return) {
        worker_destructor_runs++;
    }
}

static void *worker(void *argument) {
    worker_argument_ok = argument == (void *)&worker_argument;
    worker_self_before = pthread_self();

    check(!pthread_equal(worker_self_before, main_thread),
          "worker self is not the main thread");
    check(pthread_setspecific(worker_key, &worker_return) == 0,
          "worker sets its pthread-specific value");
    worker_specific_ok = pthread_getspecific(worker_key) == (void *)&worker_return;

    errno = WORKER_ERRNO;

    for (int round = 0; round < ROUNDS; round++) {
        if (pthread_mutex_lock(&counter_lock) != 0) {
            check(0, "worker mutex lock");
        }
        counter++;
        if (pthread_mutex_unlock(&counter_lock) != 0) {
            check(0, "worker mutex unlock");
        }
        (void)sched_yield();
    }

    worker_self_after = pthread_self();
    worker_errno_ok = errno == WORKER_ERRNO;
    return (void *)&worker_return;
}

int main(void) {
    void *joined = 0;
    int main_errno;

    put_text("PTHREAD-PROBE: start\n");

    main_thread = pthread_self();
    check(main_thread != 0, "main has a pthread_t");
    check(pthread_key_create(&worker_key, worker_key_destructor) == 0,
          "pthread_key_create");
    check(pthread_getspecific(worker_key) == 0, "main key starts null");

    errno = MAIN_ERRNO;
    check(pthread_create(&worker_thread, 0, worker, &worker_argument) == 0,
          "pthread_create");
    check(worker_thread != 0 && !pthread_equal(worker_thread, main_thread),
          "pthread_create returns a distinct pthread_t");
    check(pthread_equal(worker_thread, worker_self_before),
          "pthread_create's handle is the worker's pthread_self");

    for (int round = 0; round < ROUNDS; round++) {
        if (pthread_mutex_lock(&counter_lock) != 0) {
            check(0, "main mutex lock");
        }
        counter++;
        if (pthread_mutex_unlock(&counter_lock) != 0) {
            check(0, "main mutex unlock");
        }
        (void)sched_yield();
    }

    check(pthread_join(worker_thread, &joined) == 0, "pthread_join");
    main_errno = errno;

    check(joined == (void *)&worker_return, "pthread_join returns the worker value");
    check(counter == 2 * ROUNDS, "both threads incremented under the mutex");
    check(worker_argument_ok, "the worker received its argument");
    check(worker_specific_ok, "the worker read back its pthread-specific value");
    check(worker_destructor_runs == 1, "the worker key destructor ran once");
    check(worker_errno_ok, "the worker errno survived its yields");
    check(main_errno == MAIN_ERRNO, "main errno survived the worker");
    check(pthread_getspecific(worker_key) == 0,
          "the worker's key value did not leak into main");
    check(pthread_equal(pthread_self(), main_thread), "main pthread_self is stable");
    check(pthread_equal(worker_self_before, worker_self_after),
          "worker pthread_self is stable");
    check(!pthread_equal(worker_self_after, main_thread),
          "the two threads report distinct pthread_self values");

    /* A second create/join proves the join released the first worker's TCB
     * and table entry, and that the kernel can reuse its tid. */
    joined = 0;
    check(pthread_create(&worker_thread, 0, worker, &worker_argument) == 0,
          "second pthread_create");

    for (int round = 0; round < ROUNDS; round++) {
        if (pthread_mutex_lock(&counter_lock) != 0) {
            check(0, "main mutex lock on the second round");
        }
        counter++;
        if (pthread_mutex_unlock(&counter_lock) != 0) {
            check(0, "main mutex unlock on the second round");
        }
        (void)sched_yield();
    }

    check(pthread_join(worker_thread, &joined) == 0, "second pthread_join");
    check(joined == (void *)&worker_return, "second join returns the worker value");
    check(counter == 4 * ROUNDS, "both rounds serialised every increment");
    check(worker_destructor_runs == 2, "the key destructor ran for both workers");
    check(pthread_getspecific(worker_key) == 0,
          "the second worker's key value did not leak into main");
    check(errno == MAIN_ERRNO, "main errno survived the second worker");

    put_text("PTHREAD-PROBE: counter=");
    put_dec(counter);
    put_text(" expected=");
    put_dec(4 * ROUNDS);
    put_text("\n");
    put_text("PTHREAD-PROBE: main-self=");
    put_dec((unsigned long)main_thread);
    put_text(" worker-self=");
    put_dec((unsigned long)worker_self_after);
    put_text("\n");

    if (failures == 0) {
        put_text("PTHREADS-OK\n");
    } else {
        put_text("PTHREADS-FAILED\n");
    }
    put_text("PTHREAD-PROBE: checks=");
    put_dec((unsigned long)checks);
    put_text(" failures=");
    put_dec((unsigned long)failures);
    put_text("\n");

    return failures == 0 ? 0 : 1;
}
