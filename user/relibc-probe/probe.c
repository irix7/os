// SPDX-License-Identifier: GPL-3.0-only
//
// First C program linked against the relibc IRIX 7 PAL.
//
// The point of this probe is not to be a libc test suite.  It proves that a
// normal C `main`, linked against `librelibc.a` and relibc's crt0, starts on
// the native n32 kernel through the System V stack, uses `$28` as the TCB
// pointer, and gets a real negative errno channel from the `CELL_E_*` status
// returned by the native gate.
//
// It prints the fixed marker `IRIX7-RELIB-PROBE-1`, performs `write`, `stat`
// and two deterministic failing calls, and exits 0 only if every check passes.

typedef unsigned int size_t;
typedef int ssize_t;
typedef long long off_t;
typedef long long time_t;
typedef unsigned long long dev_t;
typedef unsigned long long ino_t;
typedef unsigned int nlink_t;
typedef int mode_t;
typedef int uid_t;
typedef int gid_t;
typedef int blksize_t;
typedef long long blkcnt_t;

struct timespec {
    time_t tv_sec;
    int tv_nsec;
    int _pad;
};

struct stat {
    dev_t st_dev;
    ino_t st_ino;
    nlink_t st_nlink;
    mode_t st_mode;
    uid_t st_uid;
    gid_t st_gid;
    dev_t st_rdev;
    off_t st_size;
    blksize_t st_blksize;
    int _pad0;
    blkcnt_t st_blocks;
    struct timespec st_atim;
    struct timespec st_mtim;
    struct timespec st_ctim;
    char _pad[24];
};

_Static_assert(sizeof(struct stat) == 136, "relibc n32 struct stat is 136 bytes");
_Static_assert(__builtin_offsetof(struct stat, st_nlink) == 16, "st_nlink offset");
_Static_assert(__builtin_offsetof(struct stat, st_mode) == 20, "st_mode offset");
_Static_assert(__builtin_offsetof(struct stat, st_size) == 40, "st_size offset");
_Static_assert(__builtin_offsetof(struct stat, st_blksize) == 48, "st_blksize offset");
_Static_assert(__builtin_offsetof(struct stat, st_blocks) == 56, "st_blocks offset");
_Static_assert(__builtin_offsetof(struct stat, st_mtim) == 80, "st_mtim offset");

#define S_IFMT 0170000
#define S_IFREG 0100000

#define EBADF 9
#define ENOENT 2

#define AT_PAGESZ 1
#define AT_ENTRY 2
#define AT_CLKTCK 13

extern ssize_t write(int, const void *, size_t);
extern int stat(const char *, struct stat *);
extern int fstat(int, struct stat *);
extern int getpid(void);
extern int *__errno_location(void);
extern unsigned long getauxval(unsigned long);
extern int printf(const char *, ...);
extern void _start(void);

#define errno (*__errno_location())

static int failures;

static void check(int condition, const char *name) {
    if (!condition) {
        printf("IRIX7-RELIB-PROBE-1: check=FAIL %s\n", name);
        failures++;
    }
}

static unsigned long read_tp(void) {
    unsigned long tp;
    __asm__ volatile("move %0, $28" : "=r"(tp));
    return tp;
}

int main(int argc, char **argv) {
    static const char marker[] = "IRIX7-RELIB-PROBE-1\n";
    struct stat st;
    int status;
    int err;
    unsigned long tp;

    status = (int)write(1, marker, sizeof(marker) - 1);
    check(status == (int)(sizeof(marker) - 1), "write returns the length");

    check(argc == 1, "argc == 1");
    check(argv != 0 && argv[0] != 0 && argv[0][0] != 0, "argv[0] present");
    check(getpid() == 1, "getpid == 1");

    /* 1 = AT_PAGESZ, 2 = AT_ENTRY, 13 = AT_CLKTCK in the native vector. */
    check(getauxval(AT_PAGESZ) == 4096, "AT_PAGESZ == 4096");
    check(getauxval(AT_ENTRY) == (unsigned long)&_start, "AT_ENTRY == _start");
    check(getauxval(AT_CLKTCK) == 100, "AT_CLKTCK == 100");

    tp = read_tp();
    check(tp != 0, "$28 is a TCB");
    check((unsigned long)__errno_location() == tp + 8, "__errno_location == TP+8");

    status = stat("/hello.txt", &st);
    check(status == 0, "stat(/hello.txt) == 0");
    check((st.st_mode & S_IFMT) == S_IFREG, "stat is regular");
    check(st.st_nlink == 1, "stat st_nlink == 1");
    check(st.st_size == 28, "stat st_size == 28");
    check(st.st_blksize == 4096, "stat st_blksize == 4096");
    check(st.st_blocks == 1, "stat st_blocks == 1");

    errno = 0;
    status = fstat(999, &st);
    err = errno;
    check(status == -1, "fstat(999) == -1");
    check(err == EBADF, "fstat(999) errno == EBADF");

    errno = 0;
    status = stat("/missing", &st);
    err = errno;
    check(status == -1, "stat(/missing) == -1");
    check(err == ENOENT, "stat(/missing) errno == ENOENT");

    printf("IRIX7-RELIB-PROBE-1: stat size=%lld mode=%o blksize=%d blocks=%lld\n",
           st.st_size, (unsigned)st.st_mode, st.st_blksize, st.st_blocks);
    printf("IRIX7-RELIB-PROBE-1: tp=%p auxv pagesz=%lu failures=%d\n",
           (void *)tp, getauxval(AT_PAGESZ), failures);

    return failures == 0 ? 0 : 1;
}
