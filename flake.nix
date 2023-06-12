{
  inputs = {
    nixpkgs.url = "github:NickCao/nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem
      (system:
        let pkgs = import nixpkgs { inherit system; }; in rec {
          devShells.default = pkgs.mkShell {
            inputsFrom = [ packages.default ];
          };
          packages.default = pkgs.rustPlatform.buildRustPackage {
            name = "bouncer";
            src = self;
            cargoLock = {
              lockFile = ./Cargo.lock;
              outputHashes = {
                "matrix-sdk-0.6.2" = "sha256-ngUhpzrqDhZ6cYhY0K5x9ZqOWru1uwGTIY4m8iax2m8=";
                "ruma-0.8.2" = "sha256-hyz5TpdCJCs3ewLUvlfH0sHDgdbJ9u1atnjAkF7i44U=";
              };
            };
          };
        }
      );
}
