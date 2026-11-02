/* SPDX-License-Identifier: GPL-3.0-only
 *
 * Cooperative user threads on the raw native gate.
 *
 * The kernel has no pthreads yet; this probe uses `SYS_THREAD_CREATE`,
 * `SYS_YIELD` and `SYS_THREAD_EXIT` directly through small assembly stubs
 * (`native.S`).  It links relibc for `write`, `__errno_location` and `_start`,
 * so the main thread's `$28` is relibc's own 16-byte TCB.
 *
 * The worker gets a static stack and a static TCB, both in `.bss` and inside
 * the image's recorded user range.  Main and the worker increment shared
 * counters, the worker sets a flag and retires with `SYS_THREAD_EXIT`, and main
 * yields until the flag is set.  A pass prints `THREADS-OK` and exits 0.
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
_Static_assert(__builtin_offsetof(struct tcb, errno_value) == 8, "TCB errno offset");

#define CELL_OK 0
#define CELL_E_FAULT 9

#define WORKER_ROUNDS 7

extern ssize_t write(int, const void *, size_t);
extern int *__errno_location(void);
extern long __irix7_thread_create(unsigned long entry, unsigned long arg,
                                  unsigned long stack_top, unsigned long tcb);
extern long __irix7_yield(void);
extern void __irix7_thread_exit(long status);

/* The gate status of the last `__irix7_thread_create`, written by native.S. */
int __irix7_thread_status;

#define errno (*__errno_location())

static int failures;
static int checks;

static _Alignas(16) unsigned char worker_stack[16384];
static struct tcb worker_tcb;
static volatile int worker_flag;
static volatile int worker_count;
static volatile int main_count;
static volatile int worker_saw_argument;
static volatile int worker_stack_used;
static volatile unsigned long worker_tp;
static volatile unsigned long worker_tp_after;

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

static void put_hex(unsigned long value) {
    static const char digits[] = "0123456789abcdef";
    char buffer[10];
    buffer[0] = '0';
    buffer[1] = 'x';
    for (int index = 0; index < 8; index++) {
        buffer[2 + index] = digits[(value >> (28 - 4 * index)) & 0xF];
    }
    (void)write(1, buffer, sizeof(buffer));
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
        put_text("THREAD-PROBE: check=FAIL ");
        put_text(name);
        put_text("\n");
        failures++;
    }
}

static unsigned long read_tp(void) {
    unsigned long tp;
    __asm__ volatile("move %0, $28" : "=r"(tp));
    return tp;
}

/* The new thread's entry point.  It must not return: `$ra` is zero and the
 * thread ends with the native exit call. */
static void worker(void *argument) {
    struct tcb *tcb = (struct tcb *)argument;
    int marker = 0;

    worker_tp = read_tp();
    worker_saw_argument = argument == (void *)&worker_tcb;
    worker_stack_used = (unsigned long)&marker >= (unsigned long)worker_stack &&
                        (unsigned long)&marker < (unsigned long)worker_stack + sizeof(worker_stack);

    tcb->self = (unsigned int)(unsigned long)tcb;
    tcb->dtv = 0;
    tcb->errno_value = 99;

    for (int round = 0; round < WORKER_ROUNDS; round++) {
        worker_count++;
        (void)__irix7_yield();
    }

    worker_tp_after = read_tp();
    tcb->libc_data = 0x5EED;
    worker_flag = 1;
    __irix7_thread_exit(0);
}

int main(void) {
    unsigned long main_tp = read_tp();
    unsigned long main_tp_after;
    unsigned long stack_top =
        ((unsigned long)worker_stack + sizeof(worker_stack)) & ~(unsigned long)15;
    unsigned long tid;
    int main_errno;

    put_text("THREAD-PROBE: start\n");

    check(main_tp != 0, "main has a TCB");
    check(__errno_location() == (int *)(main_tp + 8), "main errno is TP+8");

    /* A yield with no peer is a successful no-op. */
    check(__irix7_yield() == 0, "yield with no peer returns 0");

    /* The kernel rejects a stack top that is not in a mapped user range. */
    tid = (unsigned long)__irix7_thread_create((unsigned long)worker,
                                               (unsigned long)&worker_tcb, 0,
                                               (unsigned long)&worker_tcb);
    check(tid == 0 && __irix7_thread_status == CELL_E_FAULT,
          "create with no stack is EFAULT");

    errno = 55;
    tid = (unsigned long)__irix7_thread_create((unsigned long)worker,
                                               (unsigned long)&worker_tcb, stack_top,
                                               (unsigned long)&worker_tcb);
    check(__irix7_thread_status == CELL_OK, "thread_create succeeds");
    check(tid == 2, "the first worker is tid 2");

    while (!worker_flag) {
        main_count++;
        if (__irix7_yield() != 0) {
            break;
        }
    }
    main_errno = errno;
    main_tp_after = read_tp();

    check(worker_flag == 1, "the worker set the flag");
    check(worker_count == WORKER_ROUNDS, "the worker ran its rounds");
    check(main_count == WORKER_ROUNDS + 1, "main ran between every worker yield");
    check(worker_saw_argument, "the worker received its argument");

    check(worker_tp == (unsigned long)&worker_tcb, "the worker $28 is its TCB");
    check(worker_tp_after == (unsigned long)&worker_tcb,
          "the worker $28 survived every trap");
    check(main_tp_after == main_tp, "main $28 survived every trap");
    check(main_tp != (unsigned long)&worker_tcb, "the two TCBs are distinct");

    check(worker_stack_used, "the worker ran on the supplied static stack");
    check(worker_tcb.self == (unsigned long)&worker_tcb, "worker TCB self points at itself");
    check(worker_tcb.dtv == 0, "worker TCB dtv is zero");
    check(worker_tcb.errno_value == 99, "the worker wrote its own errno");
    check(main_errno == 55, "main errno survived the worker");
    check(worker_tcb.libc_data == 0x5EED, "the worker wrote its libc word");

    put_text("THREAD-PROBE: tid=");
    put_dec(tid);
    put_text(" worker=");
    put_dec((unsigned long)worker_count);
    put_text(" main=");
    put_dec((unsigned long)main_count);
    put_text("\n");

    put_text("THREAD-PROBE: main-tp=");
    put_hex(main_tp);
    put_text(" worker-tp=");
    put_hex(worker_tp);
    put_text("\n");

    if (failures == 0) {
        put_text("THREADS-OK\n");
    } else {
        put_text("THREADS-FAILED\n");
    }
    put_text("THREAD-PROBE: checks=");
    put_dec((unsigned long)checks);
    put_text(" failures=");
    put_dec((unsigned long)failures);
    put_text("\n");

    return failures == 0 ? 0 : 1;
}
