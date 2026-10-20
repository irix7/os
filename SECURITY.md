# Security Policy

## Supported versions

IRIX 7 is pre-1.0. Security fixes are applied to the latest stable release and the `main`
branch. There is no long-term support yet.

| Version | Supported |
|---|---|
| `main` | Yes |
| latest `v0.x` | Yes |
| older `v0.x` | Best effort |

## Reporting a vulnerability

**Do not open a public issue.**

Use GitHub's **private vulnerability reporting** on this repository:

1. Go to the **Security** tab.
2. Choose **Report a vulnerability**.
3. Include the details below.

If private reporting is unavailable, contact a maintainer listed in
[`MAINTAINERS.md`](MAINTAINERS.md) through a private channel.

## What to include

- A description of the issue and its impact.
- The affected component (kernel, compat, driver, display, storage, MCP, CI, release).
- The version/commit and the hardware or emulator configuration.
- Reproduction steps or a proof of concept.
- Whether the issue is already public.
- Your preferred credit and contact details.

## Scope

In scope:

- kernel memory safety, capability/rights bugs, privilege escalation;
- the compatibility container (syscall decode, ELF loader, libc);
- drivers and DMA/cache handling;
- the display server and X11 server;
- storage parsers (EROFS, XFS, I7FS);
- the MCP server (authentication, tool authority, prompt-injection impact);
- supply chain (dependencies, Actions, release artefacts);
- clean-room/provenance issues.

Out of scope:

- physical access to the machine;
- the SGI PROM/firmware itself;
- third-party software not shipped by IRIX 7;
- denial of service from an intentionally unbounded local root shell;
- issues that require the attacker to already have the MCP token (unless the token handling
  itself is flawed).

## Timeline

- Acknowledgement within **7 days**.
- Triage and severity assessment within **14 days**.
- A fix or mitigation target within **90 days**, depending on complexity.
- A security advisory is published after the fix, with credit unless you prefer anonymity.

## Safe harbour

We will not pursue legal action against researchers who:

- act in good faith;
- avoid privacy violations, data destruction and service disruption;
- give us reasonable time to fix the issue before disclosure;
- do not exploit the issue beyond the minimum needed to demonstrate it.

## No bounty

The project is a volunteer, hobby effort. There is no bug bounty, but we will credit
reporters and send thanks.
