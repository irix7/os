// SPDX-License-Identifier: GPL-3.0-only
//
// Freestanding n32 C probe for the native IRIX 7 syscall gate.
//
// It verifies the startup auxv, the main-thread TCB convention, `write`, `stat`
// (the 136-byte relibc layout), `getpid`, and two deterministic negative
// results (`fstat(999) -> EBADF`, `stat("/missing") -> ENOENT`).  It is not a
// libc: it links only its own syscall stubs and this file.

#include "abi.h"

struct tcb main_tcb;

const unsigned char status_errno[CELL_STATUS_COUNT] = {
    [0] = 0,
    [1] = 1,   // EPERM
    [2] = 2,   // ENOENT
    [3] = 4,   // EINTR
    [4] = 5,   // EIO
    [5] = 9,   // EBADF
    [6] = 11,  // EAGAIN
    [7] = 12,  // ENOMEM
    [8] = 13,  // EACCES
    [9] = 14,  // EFAULT
    [10] = 16, // EBUSY
    [11] = 17, // EEXIST
    [12] = 22, // EINVAL
    [13] = 28, // ENOSPC
    [14] = 20, // ENOTDIR
    [15] = 21, // EISDIR
    [16] = 39, // ENOTEMPTY
    [17] = 95, // ENOTSUP
    [18] = 75, // EOVERFLOW
    [19] = 110,
    [20] = 125,
    [21] = 9,  // E_REVOKED -> EBADF
    [22] = 90,
    [23] = 71,
    [24] = 111,
    [25] = 107,
    [26] = 98,
    [27] = 101,
    [28] = 113,
    [29] = 36,
    [30] = 40,
    [31] = 10,
    [32] = 3,
    [33] = 38,
    [34] = 32,
    [35] = 34,
    [36] = 33,
    [37] = 35,
    [38] = 37,
    [39] = 42,
    [40] = 43,
    [41] = 25,
    [42] = 29,
    [43] = 26,
    [44] = 27,
    [45] = 31,
    [46] = 30,
    [47] = 19,
    [48] = 6,
    [49] = 15,
    [50] = 7,
    [51] = 24,
    [52] = 23,
};

extern void _start(void);

static int failures;

static void put(const char *text, unsigned long length) {
    __irix7_syscall3(SYS_WRITE, 1, (long)text, (long)length);
}

static unsigned long text_length(const char *text) {
    unsigned long length = 0;
    while (text[length] != 0) {
        length++;
    }
    return length;
}

static void puts(const char *text) { put(text, text_length(text)); }

static void put_dec(u32 value) {
    char buffer[12];
    u32 index = sizeof(buffer);
    do {
        buffer[--index] = (char)('0' + value % 10);
        value /= 10;
    } while (value != 0);
    put(&buffer[index], sizeof(buffer) - index);
}

static void put_hex(u32 value) {
    static const char digits[] = "0123456789abcdef";
    char buffer[8];
    for (u32 index = 0; index < 8; index++) {
        buffer[7 - index] = digits[value & 0xF];
        value >>= 4;
    }
    put(buffer, sizeof(buffer));
}

static void put_oct(u32 value) {
    char buffer[12];
    u32 index = sizeof(buffer);
    do {
        buffer[--index] = (char)('0' + value % 8);
        value /= 8;
    } while (value != 0);
    put(&buffer[index], sizeof(buffer) - index);
}

static void check(int condition, const char *name) {
    puts(condition ? "C-PROBE: check=ok " : "C-PROBE: check=FAIL ");
    puts(name);
    puts("\n");
    if (!condition) {
        failures++;
    }
}

static void *thread_pointer(void) {
    void *pointer;
    __asm__ volatile("move %0, $28" : "=r"(pointer));
    return pointer;
}

static int *errno_pointer(void) {
    return (int *)((char *)thread_pointer() + 8);
}

static unsigned long aux_value(unsigned long *auxv, unsigned long key, int *found) {
    for (;;) {
        if (auxv[0] == AT_NULL) {
            return 0;
        }
        if (auxv[0] == key) {
            *found = 1;
            return auxv[1];
        }
        auxv += 2;
    }
}

static long sys_write(int fd, const void *buffer, unsigned long length) {
    return __irix7_syscall3(SYS_WRITE, fd, (long)buffer, (long)length);
}

static long sys_stat(const char *path, struct stat *buffer) {
    return __irix7_syscall2(SYS_STAT, (long)path, (long)buffer);
}

static long sys_fstat(int fd, struct stat *buffer) {
    return __irix7_syscall2(SYS_FSTAT, fd, (long)buffer);
}

static long sys_getpid(void) { return __irix7_syscall0(SYS_GETPID); }

void probe_main(unsigned long *stack) {
    check(((unsigned long)stack & 15) == 0, "sp 16-aligned");
    check(stack[0] == 1, "argc == 1");

    unsigned long *argv = &stack[1];
    check(argv[0] != 0, "argv[0] present");

    unsigned long *envp = argv + stack[0] + 1;
    while (*envp != 0) {
        envp++;
    }
    unsigned long *auxv = envp + 1;
    check((((unsigned long)auxv) & 3) == 0, "auxv aligned");

    int found = 0;
    unsigned long pagesz = aux_value(auxv, AT_PAGESZ, &found);
    check(found && pagesz == 4096, "AT_PAGESZ == 4096");

    found = 0;
    unsigned long entry = aux_value(auxv, AT_ENTRY, &found);
    check(found && entry == (unsigned long)&_start, "AT_ENTRY == _start");

    found = 0;
    unsigned long random = aux_value(auxv, AT_RANDOM, &found);
    check(found && random != 0, "AT_RANDOM present");
    if (random != 0) {
        u32 any = 0;
        for (u32 index = 0; index < 16; index++) {
            any |= ((const u8 *)random)[index];
        }
        check(any != 0, "AT_RANDOM non-zero");
    }

    found = 0;
    unsigned long clktck = aux_value(auxv, AT_CLKTCK, &found);
    check(found && clktck == 100, "AT_CLKTCK == 100");

    found = 0;
    unsigned long secure = aux_value(auxv, AT_SECURE, &found);
    check(found && secure == 0, "AT_SECURE == 0");

    found = 0;
    unsigned long execfn = aux_value(auxv, AT_EXECFN, &found);
    check(found && execfn != 0, "AT_EXECFN present");

    found = 0;
    unsigned long platform = aux_value(auxv, AT_PLATFORM, &found);
    check(found && platform != 0, "AT_PLATFORM present");

    check(main_tcb.self == (u32)(unsigned long)&main_tcb, "TCB self");
    check(main_tcb.dtv == 0, "TCB dtv == 0");
    check(thread_pointer() == &main_tcb, "$28 == TCB");
    check(errno_pointer() == &main_tcb.errno_value, "errno at TP+8");

    const char marker[] = "C-PROBE: marker=IRIX7-C-PROBE-1\n";
    long written = sys_write(1, marker, sizeof(marker) - 1);
    check(written == (long)(sizeof(marker) - 1), "write returns length");
    check(thread_pointer() == &main_tcb, "$28 preserved across syscall");
    puts("C-PROBE: write=ok\n");

    check(sys_getpid() == 1, "getpid == 1");

    struct stat st;
    long result = sys_stat("/hello.txt", &st);
    check(result == 0, "stat(/hello.txt) == 0");
    check(S_ISREG(st.st_mode), "stat is regular");
    check(st.st_nlink == 1, "stat st_nlink == 1");
    check(st.st_size == 28, "stat st_size == 28");
    check(st.st_blksize == 4096, "stat st_blksize == 4096");
    check(st.st_blocks == 1, "stat st_blocks == 1");

    puts("C-PROBE: stat size=");
    put_dec((u32)st.st_size);
    puts(" mode=0");
    put_oct(st.st_mode);
    puts(" blksize=");
    put_dec((u32)st.st_blksize);
    puts("\n");

    main_tcb.errno_value = 0;
    main_tcb.last_status = 0;
    result = sys_fstat(999, &st);
    check(result == -1, "fstat(999) == -1");
    check(main_tcb.last_status == CELL_E_BADF, "fstat status == E_BADF");
    check(main_tcb.errno_value == EBADF, "fstat errno == EBADF");
    puts("C-PROBE: badf status=");
    put_dec(main_tcb.last_status);
    puts(" errno=");
    put_dec((u32)main_tcb.errno_value);
    puts("\n");

    main_tcb.errno_value = 0;
    main_tcb.last_status = 0;
    result = sys_stat("/missing", &st);
    check(result == -1, "stat(/missing) == -1");
    check(main_tcb.last_status == CELL_E_NOENT, "stat status == E_NOENT");
    check(main_tcb.errno_value == ENOENT, "stat errno == ENOENT");
    puts("C-PROBE: notfound status=");
    put_dec(main_tcb.last_status);
    puts(" errno=");
    put_dec((u32)main_tcb.errno_value);
    puts("\n");

    puts("C-PROBE: tcb=0x");
    put_hex((u32)(unsigned long)&main_tcb);
    puts(" auxv=0x");
    put_hex((u32)(unsigned long)auxv);
    puts("\n");

    puts("C-PROBE: done rc=");
    put_dec(failures == 0 ? 0 : 1);
    puts("\n");

    __irix7_syscall1(SYS_EXIT, failures == 0 ? 0 : 1);
    for (;;) {
    }
}
