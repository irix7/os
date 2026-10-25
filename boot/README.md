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
mcp.url=https://raw.githubusercontent.com/irix7/os/main/boot/modules/v0.1.1/mcp.module
mcp.sha256=<64 hex digits>
```

The v0.1.0 release used the historical `httpd.module` name.  Subsequent
releases build `mcp.module`.

The release workflow builds the flat module under `boot/modules/<tag>/` and
proposes an update to this file.  Because `main` is protected, that update is a
pull request rather than a direct push; merge it after the release assets are
public.  The kernel verifies the module's SHA-256 before executing it.

The DS1386 NVRAM on a real Indy is a hardware chip, not a distribution
channel.  A local test rig may still set `mcp.url`/`mcp.sha256` in its emulated
NVRAM to override this public configuration for a temporary HTTP fixture, or
set `mcp.local=<target>:<lba>:<sectors>:<sha256>` to load the module from the
local SCSI bus, and `net=off` to skip DHCP and the network stack entirely.
Both are development/recovery overrides.  Release machines should leave them
unset and use this file.
