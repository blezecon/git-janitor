{
  description = "git-janitor reproducible zero-dependency build";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    crane.url = "github:ipetkov/crane";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, crane, fenix }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      fenixPkgs = fenix.packages.${system};
      rustToolchain = fenixPkgs.stable.toolchain;
      craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

      rustCrossToolchain = fenixPkgs.combine [
        fenixPkgs.stable.toolchain
        fenixPkgs.targets.x86_64-unknown-linux-gnu.stable.rust-std
        fenixPkgs.targets.aarch64-unknown-linux-gnu.stable.rust-std
        fenixPkgs.targets.x86_64-pc-windows-gnu.stable.rust-std
        fenixPkgs.targets.aarch64-pc-windows-gnullvm.stable.rust-std
        fenixPkgs.targets.x86_64-apple-darwin.stable.rust-std
        fenixPkgs.targets.aarch64-apple-darwin.stable.rust-std
        fenixPkgs.targets.x86_64-unknown-freebsd.stable.rust-std
      ];
    in {
      packages.${system}.default = craneLib.buildPackage {
        src = craneLib.cleanCargoSource ./.;
        strictDeps = true;
        doCheck = true;

        # Ensures deterministic binaries by stripping local build paths
        RUSTFLAGS = "--remap-path-prefix=${./.}=/build";
      };

      checks.${system} = {
        git-janitor = self.packages.${system}.default;
      };

      devShells.${system}.default = pkgs.mkShell {
        inputsFrom = [ self.packages.${system}.default ];
        packages = [
          rustCrossToolchain
          pkgs.cargo-zigbuild
          pkgs.zig
          pkgs.zip
          pkgs.gzip
          pkgs.coreutils
        ];
      };
    };
}
