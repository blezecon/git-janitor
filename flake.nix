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
      rustToolchain = fenix.packages.${system}.stable.toolchain;
      craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
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
        packages = [ rustToolchain ];
      };
    };
}
