# relibc fork/exec child probe

`probe.c` is the second image for the fork/exec gate.  The kernel stages it
from SCSI target 3 as `/bin/child`; the parent execs that path.

After `execve` it verifies:

- `getpid() == 2` and `getppid() == 1`;
- fd 3 still reads the inherited `/hello.txt` fixture;
- fd 4, marked `FD_CLOEXEC` by the parent, is closed;
- it prints `CHILD-OK` and exits 7, or exits 9 if a check failed.

The parent checks the wait status through `waitpid`.  See
[`../relibc-forkexec-probe/README.md`](../relibc-forkexec-probe/README.md).
