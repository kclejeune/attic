# Development shells

toplevel @ { lib, flake-parts-lib, ... }:
let
  inherit (lib)
    mkOption
    types
    ;
  inherit (flake-parts-lib)
    mkPerSystemOption
    ;
in
{
  options = {
    perSystem = mkPerSystemOption {
      options.attic.devshell = {
        packageSets = mkOption {
          type = types.attrsOf (types.listOf types.package);
          default = {};
        };
        extraPackages = mkOption {
          type = types.listOf types.package;
          default = [];
        };
        extraArgs = mkOption {
          type = types.attrsOf types.unspecified;
          default = {};
        };
      };
    };
  };

  config = {
    perSystem = { self', pkgs, config, ... }: let
      cfg = config.attic.devshell;
    in {
      attic.devshell.packageSets = with pkgs; {
        rustc = lib.optionals (config.attic.toolchain == null) [
          rustc
        ];

        rust = [
          cargo-audit
          cargo-expand
          cargo-outdated
          cargo-edit
          cargo-udeps
          tokio-console
        ];

        linters = [
          clippy
          rustfmt

          editorconfig-checker
        ];

        utils = [
          jq
          just
        ];

        ops = [
          postgresql
          sqlite-interactive

          skopeo
          manifest-tool
        ];

        bench = [
          wrk
        ] ++ lib.optionals pkgs.stdenv.isLinux [
          linuxPackages.perf
        ];

        wasm = [
          llvmPackages_latest.bintools
          worker-build wasm-pack wasm-bindgen-cli
        ];

        # nix 2.34's split .pc files transitively require these C libraries. The
        # pure `nix build` derivation gets them via propagation, but an
        # interactive `nix develop` does not — without them `pkg-config nix-main`
        # (in attic's build.rs) fails to resolve, and on macOS the too-old system
        # libcurl leaks in. Adding them puts their `.pc` files on PKG_CONFIG_PATH.
        nixDeps = with pkgs; [
          curl
          libgit2
          libsodium
          libblake3
          brotli
        ];
      };

      devShells.default = pkgs.mkShell (lib.recursiveUpdate {
        inputsFrom = [
          self'.packages.attic
          self'.packages.book
        ];

        packages = lib.flatten (lib.attrValues cfg.packageSets);

        env = {
          ATTIC_DISTRIBUTOR = toplevel.config.attic.distributor;

          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;

          NIX_PATH = "nixpkgs=${pkgs.path}";

          # Used by `just with-nix` to build/test with alternative Nix versions.
          NIX_VERSIONS = config.attic.nix-versions.manifestFile;

          # A leaked CPATH/LIBRARY_PATH (e.g. Homebrew's `/opt/homebrew/include`
          # or a manual export of the CommandLineTools SDK) is searched ahead of
          # nix's headers, shadowing libc++ and breaking the cxx / aws-lc-sys
          # C/C++ builds on macOS. Clear them so `cargo build` matches the pure
          # `nix build` derivation, which strips them.
          CPATH = "";
          LIBRARY_PATH = "";
        };
      } cfg.extraArgs);

      devShells.demo = pkgs.mkShell {
        packages = [ self'.packages.default ];

        shellHook = ''
          >&2 echo
          >&2 echo '🚀 Run `atticd` to get started!'
          >&2 echo
        '';
      };
    };
  };
}
