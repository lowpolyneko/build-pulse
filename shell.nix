with import <nixpkgs> { };

mkShell {
  packages = [
    openssl
    pkg-config
  ];
}

# vim: ts=2:sw=2:expandtab:
