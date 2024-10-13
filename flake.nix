{
  inputs = {
    nixpkgs.url = "github:NickCao/nixpkgs";
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
      let
        pkgs = import nixpkgs { inherit system; };
      in
      rec {
        devShells.default = pkgs.mkShell {
          inputsFrom = [ packages.default ];
        };
        packages.default = pkgs.rustPlatform.buildRustPackage {
          name = "bouncer";
          src = self;
          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "ruma-0.10.1" = "sha256-UOC+i2vVqHEdGkLbc/f4tbiuodIWiw1k9UnVHcT9VKU=";
            };
          };
          meta.mainProgram = "bouncer";
        };
      }
    );
}
