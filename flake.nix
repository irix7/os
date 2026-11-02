{
  description = "IRIX 7 OS development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Binary distributions of the exact rustup channels named in
    # rust-toolchain.toml, including rust-src.  nixpkgs' rustc is a stable
    # release without rust-src, so it cannot run -Z build-std.
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ (import rust-overlay) ];
      };

      # The single pin: rust-toolchain.toml.  The devshell, a rustup user and
      # CI all read the same channel and component list, so a shell cannot
      # drift from the toolchain CI proves.
      rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

      # Host tools every cross build needs: -Zbuild-std compiles
      # compiler-builtins' build script with the host C compiler, and the
      # repository's image tools are Python.  One list, so a new shell cannot
      # quietly omit one.
      crossTools = [
        pkgs.gcc
        pkgs.binutils
        pkgs.python3
        pkgs.pkg-config
        pkgs.llvmPackages.bintools
      ];
    in {
      devShells.${system} = {
        default = pkgs.mkShell {
          name = "irix7-os-dev";

          nativeBuildInputs = crossTools ++ [ rust ];

          # rust-toolchain.toml names nightly-2026-10-03 with rust-src and
          # llvm-tools-preview, so this shell runs the M0 gate directly:
          #
          #   cargo build -Z build-std=core,alloc -Zjson-target-spec \
          #     --features cross --target targets/mips3-sgi-irix7-n32.json -p kernel
          #
          # rust-lld and llvm-objcopy live beside the toolchain, not in its
          # bin/, so put the host rustlib directory on PATH exactly as CI does.
          shellHook = ''
            sysroot="$(rustc --print sysroot)"
            host="$(rustc -vV | sed -n 's/^host: //p')"
            export PATH="$sysroot/lib/rustlib/$host/bin:$PATH"
            if [ ! -d "$sysroot/lib/rustlib/src/rust/library" ]; then
              echo "irix7: rust-src is missing; -Z build-std will fail" >&2
            fi
            echo "IRIX 7 devshell (${system})"
            echo "rustc: $(rustc --version 2>/dev/null || echo 'not on PATH')"
            echo "Target specs: $PWD/targets"
          '';
        };

        # PowerBook G3 (PowerPC 750) cross shell.  This shell uses rustup rather
        # than the pinned toolchain derivation: its first entry downloads the
        # channel named in rust-toolchain.toml into RUSTUP_HOME.
        powerbook = pkgs.mkShell {
          name = "irix7-powerbook-dev";

          nativeBuildInputs = crossTools ++ [
            pkgs.rustup
            # System emulation for a future boot gate.  qemu-system-ppc is in
            # the full qemu build, not in qemu-host-cpu-only.
            pkgs.qemu
          ];

          # Force the channel even if the caller runs the shell from outside
          # the repository root.  rust-toolchain.toml installs rust-src and
          # llvm-tools-preview with it.
          RUSTUP_TOOLCHAIN = "nightly-2026-10-03";

          shellHook = ''
            rustup component add rust-src llvm-tools-preview \
              --toolchain "$RUSTUP_TOOLCHAIN" >/dev/null 2>&1 ||
              echo "powerbook: run 'rustup component add rust-src llvm-tools-preview' before building"
            echo "IRIX 7 PowerBook devshell (${system})"
            echo "rustc: $(rustc --version 2>/dev/null || echo 'not on PATH')"
            echo "Target: $PWD/targets/powerpc-unknown-irix7-ppc.json"
            echo "Check:  tools/powerpc-check.sh"
            echo "QEMU:   $(qemu-system-ppc --version 2>/dev/null | head -n 1 || echo 'not on PATH')"
          '';
        };
      };
    };
}
