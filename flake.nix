{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };
  outputs = {
    self,
    nixpkgs,
    flake-utils,
    ...
  }:
    {
      overlay = final: prev: {
        measure-hdr = self.packages.${prev.system}.default;
      };
    }
    // flake-utils.lib.eachDefaultSystem (system:
      # Instantiating a nixpkgs here as it needs to have `config.allowUnfree = true;`.
      # Using https://github.com/numtide/nixpkgs-unfree caused opencv to build from source.
      let
        pkgs = import "${nixpkgs}" {
          config.allowUnfree = true;
          inherit system;
        };
        inherit (pkgs) lib rustPlatform;

        nnedi_weights = pkgs.fetchurl {
          url = "https://github.com/dubhater/vapoursynth-nnedi3/raw/cc6f6065e09c9241553cb51f10002a7314d66bfa/src/nnedi3_weights.bin";
          sha256 = "0hhx4n19qaj3g68f5kqjk23cj063g4y2zidivq9pdfrm0i1q5wr7";
        };

        measure-hdr = {
          rustPlatform,
          lib,
          pkg-config,
          ffmpeg,
        }:
          rustPlatform.buildRustPackage {
            name = "measure-hdr";
            src = lib.cleanSource ./.;

            cargoLock.lockFile = ./Cargo.lock;

            # Point to a nixpkgs ffmpeg rather than using the one on $PATH

            nativeBuildInputs = [
              pkg-config
              rustPlatform.bindgenHook
            ];

            buildInputs = with pkgs; (
              [
                ffmpeg
                fontconfig
              ]
              ++ lib.optionals (stdenv.isDarwin) [
                libiconv
              ]
            );

            inherit ffmpeg;

            meta = with lib; {
              license = licenses.mpl20;
              homepage = "https://github.com/Sciencentistguy/measure-hdr";
              platforms = ffmpeg.meta.platforms;
            };
          };
      in {
        packages.measure-hdr = pkgs.callPackage measure-hdr {
          ffmpeg = pkgs.ffmpeg_7.override {
            ffmpegVariant = "full";
            withFullDeps = true;
            withUnfree = true;
          };
        };

        packages.nnedi_weights = nnedi_weights;

        packages.default = self.packages.${system}.measure-hdr;

        devShells.default = self.packages.${system}.default.overrideAttrs (super: {
          nativeBuildInputs = with pkgs;
            super.nativeBuildInputs
            ++ [
              clippy
              rustfmt
              cargo-edit
            ];
          RUST_SRC_PATH = "${rustPlatform.rustLibSrc}";
        });
      });
}
