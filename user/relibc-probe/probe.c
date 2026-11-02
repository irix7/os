// SPDX-License-Identifier: GPL-3.0-only
//
// Third C program linked against the relibc IRIX 7 PAL.
//
// The first probe proved that a normal C `main` starts on the native n32
// kernel through the System V stack, uses `$28` as the TCB pointer, and gets a
// real negative errno channel from the `CELL_E_*` status returned by the native
// gate.  The second added the file surface: seek, read and write at an offset,
// a duplicated descriptor, a bounded pipe with EAGAIN and EPIPE, close-on-exec,
// and a directory iterated to the end with cookies.  This one keeps those
// checks and adds descriptor readiness: `poll` over pipes and the console with
// zero and negative timeouts, real `O_NONBLOCK` state through
// `F_GETFL`/`F_SETFL`, `POLLHUP` at end-of-file and `POLLERR` with no reader.
//
// It prints the fixed marker `IRIX7-RELIB-PROBE-3` and exits 0 only if every
// check passes.

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

struct dirent {
    ino_t d_ino;
    off_t d_off;
    unsigned short d_reclen;
    unsigned char d_type;
    char d_name[256];
};

/* POSIX-2024 directory record; the same 280-byte layout as dirent. */
struct posix_dent {
    ino_t d_ino;
    off_t d_off;
    unsigned short d_reclen;
    unsigned char d_type;
    char d_name[256];
};

struct pollfd {
    int fd;
    short events;
    short revents;
};

_Static_assert(sizeof(struct stat) == 136, "relibc n32 struct stat is 136 bytes");
_Static_assert(__builtin_offsetof(struct stat, st_nlink) == 16, "st_nlink offset");
_Static_assert(__builtin_offsetof(struct stat, st_mode) == 20, "st_mode offset");
_Static_assert(__builtin_offsetof(struct stat, st_size) == 40, "st_size offset");
_Static_assert(__builtin_offsetof(struct stat, st_blksize) == 48, "st_blksize offset");
_Static_assert(__builtin_offsetof(struct stat, st_blocks) == 56, "st_blocks offset");
_Static_assert(__builtin_offsetof(struct stat, st_mtim) == 80, "st_mtim offset");
_Static_assert(sizeof(struct dirent) == 280, "n32 dirent is 280 bytes");
_Static_assert(__builtin_offsetof(struct dirent, d_off) == 8, "d_off offset");
_Static_assert(__builtin_offsetof(struct dirent, d_name) == 19, "d_name offset");
_Static_assert(sizeof(struct pollfd) == 8, "n32 pollfd is 8 bytes");
_Static_assert(__builtin_offsetof(struct pollfd, events) == 4, "pollfd events offset");
_Static_assert(__builtin_offsetof(struct pollfd, revents) == 6, "pollfd revents offset");

#define S_IFMT 0170000
#define S_IFREG 0100000
#define S_IFDIR 0040000

#define EBADF 9
#define EAGAIN 11
#define EINVAL 22
#define ESPIPE 29
#define EPIPE 32
#define ENOENT 2

#define AT_PAGESZ 1
#define AT_ENTRY 2
#define AT_CLKTCK 13

#define O_RDONLY 0
#define O_RDWR 2
#define O_NONBLOCK 0x800
#define O_DIRECTORY 0x10000
#define O_CLOEXEC 0x80000

#define F_GETFD 1
#define F_SETFD 2
#define F_GETFL 3
#define F_SETFL 4
#define FD_CLOEXEC 1

#define POLLIN 0x001
#define POLLOUT 0x004
#define POLLERR 0x008
#define POLLHUP 0x010
#define POLLNVAL 0x020

#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#define SEEK_DATA 3
#define SEEK_HOLE 4

extern ssize_t write(int, const void *, size_t);
extern ssize_t read(int, void *, size_t);
extern int open(const char *, int, ...);
extern int close(int);
extern off_t lseek(int, off_t, int);
extern ssize_t pread(int, void *, size_t, off_t);
extern ssize_t pwrite(int, const void *, size_t, off_t);
extern int dup(int);
extern int dup2(int, int);
extern int pipe(int *);
extern int pipe2(int *, int);
extern int fcntl(int, int, ...);
extern int poll(struct pollfd *, unsigned long, int);
extern int stat(const char *, struct stat *);
extern int fstat(int, struct stat *);
extern int getpid(void);
extern int *__errno_location(void);
extern unsigned long getauxval(unsigned long);
extern int printf(const char *, ...);
extern void *opendir(const char *);
extern struct dirent *readdir(void *);
extern int closedir(void *);
extern void seekdir(void *, long);
extern long telldir(void *);
extern ssize_t posix_getdents(int, void *, size_t, int);
extern void _start(void);

#define errno (*__errno_location())

static int failures;
static int checks;

static void check(int condition, const char *name) {
    checks++;
    if (!condition) {
        printf("IRIX7-RELIB-PROBE-3: check=FAIL %s\n", name);
        failures++;
    }
}

static int bytes_are(const char *buffer, const char *text, int length) {
    for (int index = 0; index < length; index++) {
        if (buffer[index] != text[index]) {
            return 0;
        }
    }
    return 1;
}

static int name_is(const char *name, const char *text) {
    int index = 0;
    while (name[index] != 0 && text[index] != 0) {
        if (name[index] != text[index]) {
            return 0;
        }
        index++;
    }
    return name[index] == 0 && text[index] == 0;
}

static unsigned long read_tp(void) {
    unsigned long tp;
    __asm__ volatile("move %0, $28" : "=r"(tp));
    return tp;
}

static _Alignas(8) unsigned char dent_buffer[1024];

int main(int argc, char **argv) {
    static const char marker[] = "IRIX7-RELIB-PROBE-3\n";
    struct stat st;
    int status;
    int err;
    unsigned long tp;
    int fd, other, pipefd[2];
    struct pollfd pfd[2];
    char buffer[64];
    long cookie;

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

    /* Seek, read and write at an offset on a regular file. */
    fd = open("/hello.txt", O_RDWR);
    check(fd >= 0, "open(/hello.txt) succeeds");
    check(lseek(fd, 6, SEEK_SET) == 6, "lseek(6, SEEK_SET) == 6");
    check(read(fd, buffer, 4) == 4 && bytes_are(buffer, "from", 4), "read at 6 is 'from'");
    check(lseek(fd, 0, SEEK_CUR) == 10, "SEEK_CUR tracks the read");
    check(lseek(fd, 0, SEEK_END) == 28, "SEEK_END == 28");
    check(lseek(fd, -5, SEEK_END) == 23, "lseek(-5, SEEK_END) == 23");
    check(read(fd, buffer, 2) == 2 && bytes_are(buffer, "am", 2), "read at 23 is 'am'");
    errno = 0;
    check(lseek(fd, -1, SEEK_SET) == -1 && errno == EINVAL, "negative offset is EINVAL");
    errno = 0;
    check(lseek(fd, 0, 7) == -1 && errno == EINVAL, "bad whence is EINVAL");
    check(lseek(fd, 0, SEEK_DATA) == 0, "SEEK_DATA at 0 == 0");
    check(lseek(fd, 0, SEEK_HOLE) == 28, "SEEK_HOLE at 0 == 28");
    check(lseek(fd, 6, SEEK_SET) == 6, "lseek back to 6");
    check(write(fd, "FROM", 4) == 4, "write at 6 writes four bytes");
    check(lseek(fd, 0, SEEK_SET) == 0, "lseek back to 0");
    check(read(fd, buffer, 10) == 10 && bytes_are(buffer, "hello FROM", 10),
          "sequential read sees the written bytes");
    check(pwrite(fd, "from", 4, 6) == 4, "pwrite restores bytes at 6");
    check(pread(fd, buffer, 4, 6) == 4 && bytes_are(buffer, "from", 4), "pread at 6 is 'from'");
    check(lseek(fd, 0, SEEK_CUR) == 10, "pread and pwrite do not move the offset");

    /* A duplicated descriptor shares the offset. */
    other = dup(fd);
    check(other >= 0, "dup returns a descriptor");
    check(lseek(other, 0, SEEK_CUR) == 10, "dup shares the offset");
    check(lseek(other, 0, SEEK_SET) == 0, "seek through the duplicate");
    check(lseek(fd, 0, SEEK_CUR) == 0, "the original sees the shared seek");
    check(dup2(fd, 9) == 9, "dup2 onto 9");
    check(lseek(9, 0, SEEK_SET) == 0, "seek through dup2 target");
    check(read(9, buffer, 5) == 5 && bytes_are(buffer, "hello", 5), "read through dup2 target");
    check(lseek(other, 0, SEEK_CUR) == 5, "dup2 shares the offset");
    errno = 0;
    check(dup2(999, 10) == -1 && errno == EBADF, "dup2 from a bad descriptor is EBADF");
    check(close(9) == 0, "close dup2 target");
    check(close(other) == 0, "close duplicate");
    check(close(fd) == 0, "close hello.txt");

    /* Close-on-exec is real state and is per descriptor. */
    fd = open("/hello.txt", O_RDONLY | O_CLOEXEC);
    check(fd >= 0, "open with O_CLOEXEC");
    check(fcntl(fd, F_GETFD) == FD_CLOEXEC, "O_CLOEXEC reaches F_GETFD");
    check(fcntl(fd, F_SETFD, 0) == 0, "F_SETFD clears the flag");
    check(fcntl(fd, F_GETFD) == 0, "F_GETFD reports the cleared flag");
    check(fcntl(fd, F_SETFD, FD_CLOEXEC) == 0, "F_SETFD sets the flag");
    check(fcntl(fd, F_GETFD) == FD_CLOEXEC, "F_GETFD reports the set flag");
    other = dup(fd);
    check(other >= 0 && fcntl(other, F_GETFD) == 0, "dup clears close-on-exec");
    check(dup2(other, 8) == 8 && fcntl(8, F_GETFD) == 0, "dup2 clears close-on-exec");
    check(close(8) == 0 && close(other) == 0 && close(fd) == 0, "close the flag descriptors");
    errno = 0;
    check(fcntl(999, F_GETFD) == -1 && errno == EBADF, "fcntl on a bad descriptor is EBADF");

    /* F_GETFL carries the access mode; F_SETFL changes O_NONBLOCK. */
    fd = open("/hello.txt", O_RDWR);
    check(fd >= 0, "open for F_GETFL succeeds");
    check((fcntl(fd, F_GETFL) & 3) == O_RDWR, "F_GETFL reports O_RDWR");
    check((fcntl(fd, F_GETFL) & O_NONBLOCK) == 0, "F_GETFL starts without O_NONBLOCK");
    check(fcntl(fd, F_SETFL, O_NONBLOCK) == 0, "F_SETFL sets O_NONBLOCK on a file");
    check((fcntl(fd, F_GETFL) & O_NONBLOCK) != 0, "F_GETFL reports the file flag");
    check(fcntl(fd, F_SETFL, 0) == 0, "F_SETFL clears O_NONBLOCK on a file");
    check((fcntl(fd, F_GETFL) & O_NONBLOCK) == 0, "F_GETFL reports the cleared flag");
    check(close(fd) == 0, "close the F_GETFL file");

    /* A bounded pipe with real O_NONBLOCK state and poll readiness. */
    check(pipe(pipefd) == 0, "pipe succeeds");
    check(write(pipefd[1], "ping", 4) == 4, "write four bytes into the pipe");
    check(read(pipefd[0], buffer, 8) == 4 && bytes_are(buffer, "ping", 4),
          "read the four bytes back");
    errno = 0;
    check(lseek(pipefd[0], 0, SEEK_SET) == -1 && errno == ESPIPE, "lseek on a pipe is ESPIPE");

    check((fcntl(pipefd[0], F_GETFL) & O_NONBLOCK) == 0, "a fresh pipe is blocking");
    check(fcntl(pipefd[0], F_SETFL, O_NONBLOCK) == 0, "F_SETFL sets O_NONBLOCK on a pipe");
    check((fcntl(pipefd[0], F_GETFL) & O_NONBLOCK) != 0, "F_GETFL reports O_NONBLOCK");
    other = dup(pipefd[0]);
    check(other >= 0 && (fcntl(other, F_GETFL) & O_NONBLOCK) != 0,
          "dup shares O_NONBLOCK");
    errno = 0;
    check(read(pipefd[0], buffer, 8) == -1 && errno == EAGAIN,
          "a non-blocking empty pipe read is EAGAIN");
    check(fcntl(pipefd[0], F_SETFL, 0) == 0, "F_SETFL clears O_NONBLOCK");
    check((fcntl(other, F_GETFL) & O_NONBLOCK) == 0, "the shared description is cleared");
    check(close(other) == 0, "close the duplicate reader");

    /* poll: an empty pipe is not readable, its writer is writable. */
    pfd[0].fd = pipefd[0];
    pfd[0].events = POLLIN;
    pfd[0].revents = -1;
    check(poll(pfd, 1, 0) == 0 && pfd[0].revents == 0,
          "poll on an empty pipe times out at zero");

    pfd[0].fd = pipefd[1];
    pfd[0].events = POLLOUT;
    pfd[0].revents = -1;
    status = poll(pfd, 1, 0);
    check(status == 1 && (pfd[0].revents & POLLOUT) != 0, "poll reports a writable pipe");
    check((pfd[0].revents & (POLLERR | POLLHUP)) == 0, "a live pipe has no error");

    /* Two entries: only the console is ready, and revents are per record. */
    pfd[0].fd = 1;
    pfd[0].events = POLLOUT;
    pfd[0].revents = -1;
    pfd[1].fd = pipefd[0];
    pfd[1].events = POLLIN;
    pfd[1].revents = -1;
    status = poll(pfd, 2, 0);
    check(status == 1, "poll counts only ready descriptors");
    check((pfd[0].revents & POLLOUT) != 0 && pfd[1].revents == 0,
          "poll reports revents per descriptor");

    /* A negative timeout is bounded: an idle pipe returns 0 after the cap. */
    pfd[0].fd = pipefd[0];
    pfd[0].events = POLLIN;
    pfd[0].revents = -1;
    check(poll(pfd, 1, -1) == 0, "a negative timeout is bounded");

    /* Data makes the reader readable. */
    check(write(pipefd[1], "poll", 4) == 4, "write for poll");
    pfd[0].fd = pipefd[0];
    pfd[0].events = POLLIN;
    pfd[0].revents = -1;
    status = poll(pfd, 1, 0);
    check(status == 1 && (pfd[0].revents & POLLIN) != 0, "poll sees pipe data");
    check((pfd[0].revents & POLLHUP) == 0, "a live pipe is not hung up");
    check(read(pipefd[0], buffer, 8) == 4 && bytes_are(buffer, "poll", 4),
          "read the polled bytes");

    /* Every writer closed: POLLHUP and end-of-file. */
    check(close(pipefd[1]) == 0, "close the writer before HUP");
    pfd[0].fd = pipefd[0];
    pfd[0].events = POLLIN;
    pfd[0].revents = -1;
    status = poll(pfd, 1, 0);
    check(status == 1 && (pfd[0].revents & POLLHUP) != 0, "poll reports POLLHUP at EOF");
    check((pfd[0].revents & POLLIN) == 0, "no POLLIN at EOF");
    check(read(pipefd[0], buffer, 8) == 0, "read at end-of-file returns 0");
    check(close(pipefd[0]) == 0, "close the pipe reader");

    /* No reader left: POLLERR on the write end and EPIPE on write. */
    check(pipe(pipefd) == 0, "error pipe succeeds");
    check(close(pipefd[0]) == 0, "close the only reader");
    pfd[0].fd = pipefd[1];
    pfd[0].events = POLLOUT;
    pfd[0].revents = -1;
    status = poll(pfd, 1, 0);
    check(status == 1 && (pfd[0].revents & POLLERR) != 0,
          "poll reports POLLERR with no reader");
    errno = 0;
    check(write(pipefd[1], "x", 1) == -1 && errno == EPIPE, "write with no reader is EPIPE");
    check(close(pipefd[1]) == 0, "close the pipe writer");

    /* The console is always writable; a negative timeout returns at once. */
    pfd[0].fd = 1;
    pfd[0].events = POLLOUT;
    pfd[0].revents = -1;
    status = poll(pfd, 1, -1);
    check(status == 1 && (pfd[0].revents & POLLOUT) != 0,
          "the console is POLLOUT with a negative timeout");

    /* Closed and negative descriptors. */
    pfd[0].fd = 999;
    pfd[0].events = POLLIN;
    pfd[0].revents = -1;
    status = poll(pfd, 1, 0);
    check(status == 1 && (pfd[0].revents & POLLNVAL) != 0,
          "poll reports POLLNVAL for a closed descriptor");
    pfd[0].fd = -1;
    pfd[0].events = POLLIN;
    pfd[0].revents = -1;
    check(poll(pfd, 1, 0) == 0 && pfd[0].revents == 0, "a negative fd is ignored");
    check(poll(pfd, 0, 0) == 0, "poll with no descriptors returns 0");

    check(pipe2(pipefd, O_CLOEXEC) == 0, "pipe2 with O_CLOEXEC");
    check(fcntl(pipefd[0], F_GETFD) == FD_CLOEXEC, "pipe2 marks the reader");
    check(fcntl(pipefd[1], F_GETFD) == FD_CLOEXEC, "pipe2 marks the writer");
    other = dup(pipefd[0]);
    check(other >= 0 && fcntl(other, F_GETFD) == 0, "dup of a pipe clears close-on-exec");
    check(dup2(pipefd[1], 7) == 7 && fcntl(7, F_GETFD) == 0,
          "dup2 of a pipe clears close-on-exec");
    check(close(7) == 0 && close(other) == 0, "close the pipe duplicates");
    check(close(pipefd[0]) == 0 && close(pipefd[1]) == 0, "close the pipe2 pair");
    errno = 0;
    check(pipe2(pipefd, 1) == -1 && errno == EINVAL, "pipe2 rejects unknown flags");

    /* Read a directory in batches and walk it back with cookies. */
    fd = open("/", O_RDONLY | O_DIRECTORY);
    check(fd >= 0, "open(/) succeeds");
    status = (int)posix_getdents(fd, dent_buffer, sizeof(dent_buffer), 0);
    check(status == 3 * 280, "getdents fills three records");
    if (status == 3 * 280) {
        struct posix_dent *dent = (struct posix_dent *)dent_buffer;
        check(dent[0].d_off == 1 && name_is(dent[0].d_name, "."), "cookies: . -> 1");
        check(dent[1].d_off == 2 && name_is(dent[1].d_name, ".."), "cookies: .. -> 2");
        check(dent[2].d_off == 3 && name_is(dent[2].d_name, "hello.txt"),
              "cookies: hello.txt -> 3");
        check(dent[2].d_ino != 0 && dent[2].d_reclen == 280 && dent[2].d_type == 8,
              "dirent fields are filled");
    }
    status = (int)posix_getdents(fd, dent_buffer, sizeof(dent_buffer), 0);
    check(status == 2 * 280, "getdents continues after the cookie");
    if (status == 2 * 280) {
        struct posix_dent *dent = (struct posix_dent *)dent_buffer;
        check(dent[0].d_off == 4 && name_is(dent[0].d_name, "etc"), "cookies: etc -> 4");
        check(dent[1].d_off == 5 && name_is(dent[1].d_name, "dev"), "cookies: dev -> 5");
    }
    check(posix_getdents(fd, dent_buffer, sizeof(dent_buffer), 0) == 0,
          "getdents at end-of-directory returns 0");
    check(lseek(fd, 1, SEEK_SET) == 1, "lseek to the '..' cookie");
    status = (int)posix_getdents(fd, dent_buffer, sizeof(dent_buffer), 0);
    check(status == 3 * 280, "getdents resumes at the cookie");
    if (status == 3 * 280) {
        struct posix_dent *dent = (struct posix_dent *)dent_buffer;
        check(dent[0].d_off == 2 && name_is(dent[0].d_name, ".."), "resume: .. -> 2");
        check(name_is(dent[1].d_name, "hello.txt"), "resume reaches hello.txt second");
        check(dent[2].d_off == 4 && name_is(dent[2].d_name, "etc"), "resume: etc -> 4");
    }
    status = (int)posix_getdents(fd, dent_buffer, sizeof(dent_buffer), 0);
    check(status == 280, "getdents finishes the directory");
    if (status == 280) {
        struct posix_dent *dent = (struct posix_dent *)dent_buffer;
        check(dent[0].d_off == 5 && name_is(dent[0].d_name, "dev"), "finish: dev -> 5");
    }
    check(lseek(fd, 0, SEEK_SET) == 0, "rewind the directory");
    errno = 0;
    check(posix_getdents(fd, dent_buffer, 100, 0) == -1 && errno == EINVAL,
          "a buffer smaller than one record is EINVAL");
    check(lseek(fd, 0, SEEK_END) == 5, "SEEK_END on a directory is the entry count");
    check(close(fd) == 0, "close the directory");

    /* The readdir(3) wrapper reaches the same entries and cookies. */
    {
        static const char *expected[5] = {".", "..", "hello.txt", "etc", "dev"};
        void *dir = opendir("/");
        struct dirent *entry;
        int index = 0;
        check(dir != 0, "opendir(/) succeeds");
        if (dir != 0) {
            while (index < 5) {
                entry = readdir(dir);
                check(entry != 0 && name_is(entry->d_name, expected[index]),
                      "readdir returns the expected entry");
                index++;
            }
            check(readdir(dir) == 0, "readdir reaches end-of-directory");
            seekdir(dir, 0);
            entry = readdir(dir);
            check(entry != 0 && name_is(entry->d_name, "."), "seekdir(0) rewinds");
            cookie = telldir(dir);
            check(cookie == 1, "telldir reports the next cookie");
            check(closedir(dir) == 0, "closedir succeeds");
        }
    }

    printf("IRIX7-RELIB-PROBE-3: stat size=%lld mode=%o blksize=%d blocks=%lld\n",
           st.st_size, (unsigned)st.st_mode, st.st_blksize, st.st_blocks);
    printf("IRIX7-RELIB-PROBE-3: tp=%p auxv pagesz=%lu\n",
           (void *)tp, getauxval(AT_PAGESZ));
    printf("IRIX7-RELIB-PROBE-3: checks=%d failures=%d\n", checks, failures);

    return failures == 0 ? 0 : 1;
}
