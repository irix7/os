# Boot configuration

`config.conf` is the kernel's repository boot configuration.  The kernel
fetches it at boot from:

```text
https://raw.githubusercontent.com/irix7/os/main/boot/config.conf
```

The file maps the boot-time module names to immutable raw GitHub files:

```text
version=1
release=v0.1.0
mcp.url=https://raw.githubusercontent.com/irix7/os/main/boot/modules/v0.1.0/httpd.module
mcp.sha256=<64 hex digits>
```

The release workflow updates this file and adds the flat module under
`boot/modules/<tag>/` after the GitHub Release is built.  The kernel verifies
the module's SHA-256 before executing it.

The DS1386 NVRAM on a real Indy is a hardware chip, not a distribution
channel.  A local test rig may still set `mcp.url`/`mcp.sha256` in its emulated
NVRAM to override this public configuration for a temporary HTTP fixture; that
is a development-only override.  Release machines should leave it unset and
use this file.
