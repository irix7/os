{
  description = "IRIX 7 OS development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};

      # Host tools every cross build needs: -Zbuild-std compiles
      # compiler-builtins' build script with the host C compiler, and the
      # repository's image tools are Python.  One list, so a new shell cannot
      # quietly omit one.
      crossTools = [
        pkgs.gcc
        pkgs.binutils
        pkgs.python3
        pkgs.llvmPackages.bintools
      ];
    in {
      devShells.${system} = {
        default = pkgs.mkShell {
          name = "irix7-os-dev";

          nativeBuildInputs = [
            # Host toolchain pinned by rust-toolchain.toml (nightly + rust-src).
            pkgs.rustc
            pkgs.cargo
            pkgs.rustfmt
            pkgs.clippy
            # Host C compiler/linker needed by build-std (compiler-rt build scripts).
            pkgs.gcc
            pkgs.binutils
          ];

          # The custom MIPS/IRIX target specs live in targets/ and are pinned in
          # this repo. Build with:
          #   cargo build -Z build-std=core,alloc -Zjson-target-spec \
          #     --target targets/mips3-sgi-irix7-n32.json
          #
          # rust-toolchain.toml pins the nightly channel; rust-src is required for
          # -Z build-std and is pulled in automatically by rustup/toolchain file.
          shellHook = ''
            echo "IRIX 7 devshell (${system})"
            echo "rustc: $(rustc --version 2>/dev/null || echo 'not on PATH')"
            echo "Target specs: $PWD/targets"
          '';
        };

        # PowerBook G3 (PowerPC 750) cross shell.  This shell uses rustup rather
        # than nixpkgs' rustc: -Zbuild-std needs the rust-src component and the
        # PowerPC backend of the nightly pinned in rust-toolchain.toml, and
        # nixpkgs ships a stable toolchain without rust-src.  The first entry
        # downloads that nightly into RUSTUP_HOME.
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
