{
  description = "build-pulse";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      with import nixpkgs {
        inherit system;
      }; {
        devShell = mkShell {
          packages = [
            openssl
            pkg-config
            rustup
            sqlite
          ];
        };
      }
    );
}

# vim: ts=2:sw=2:expandtab:
