# relibc fork/exec parent probe

`probe.c` is the parent image for the local cell fork/exec/wait gate.  It is
linked against the relibc `irix7-pal` branch and runs from SCSI target 2; the
child image is [`../relibc-forkexec-child/probe.c`](../relibc-forkexec-child/probe.c)
on SCSI target 3 and is staged as `/bin/child` at boot.

The parent:

- confirms its local pid is 1 and it has no parent;
- opens `/hello.txt` twice, marks fd 4 `FD_CLOEXEC`, and forks;
- verifies `waitpid(pid, &status, WNOHANG)` reports 0 because the cooperative
  child cannot have run;
- waits for the child, which execs `/bin/child`, prints `CHILD-OK` and exits 7;
- checks the wait status, the inherited fd 3 and the shared file offset;
- prints `FORK-EXEC-OK` and exits 0.

The child checks its pid is 2, its parent is 1, fd 3 is still the fixture file
and fd 4 was closed by `FD_CLOEXEC`.

```sh
tools/relibc-pal/build.sh /tmp/opencode/relibc-pal-work/out
```

Attach `forkexec-probe.raw` to SCSI target 2 and `forkexec-child.raw` to target
3.  See [`tools/relibc-pal/README.md`](../../tools/relibc-pal/README.md).
