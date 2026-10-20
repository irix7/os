{
  description = "IRIX 7 OS development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in {
      devShells.${system}.default = pkgs.mkShell {
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
    };
}
