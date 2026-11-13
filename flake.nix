{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    flake-parts.url = "github:hercules-ci/flake-parts";
    git-hooks.url = "github:cachix/git-hooks.nix";
    git-hooks.inputs.nixpkgs.follows = "nixpkgs";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";
    # RustSec's advisories, pinned: `checks.audit` reads them offline
    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  outputs = { self, ... } @inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      systems = inputs.nixpkgs.lib.systems.flakeExposed;
      imports = [
        inputs.git-hooks.flakeModule
        inputs.treefmt-nix.flakeModule
      ];

      perSystem =
        { self'
        , pkgs
        , config
        , ...
        }:
        {
          # Code formatter -> 'nix fmt'
          treefmt.config = {
            projectRootFile = "flake.nix";
            flakeFormatter = true;
            flakeCheck = true;
            programs = {
              nixpkgs-fmt.enable = true;
              deadnix.enable = true;
              statix.enable = true;
              rustfmt.enable = true;
              taplo.enable = true;
            };
          };

          # Pre-commit hooks
          pre-commit.check.enable = false;
          pre-commit.settings.hooks = {
            treefmt = {
              enable = true;
              package = config.treefmt.build.wrapper;
            };
            pedantic-clippy = {
              enable = true;
              entry = "cargo clippy --all-targets -- -D clippy::pedantic -D clippy::cognitive_complexity";
              files = "\\.rs$";
              pass_filenames = false;
            };
            cargo-test = {
              enable = true;
              entry = "cargo test";
              files = "\\.rs$";
              pass_filenames = false;
            };
          };

          # Development shell -> 'nix develop' or 'direnv allow'
          devShells.default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              rustc
              rustfmt
              rust-analyzer
              pre-commit
            ];
            shellHook = config.pre-commit.installationScript;
          };

          checks =
            let
              inherit (pkgs) lib;
              # The parts of Home Manager and of `programs.herdr` the module reads
              home = {
                options = {
                  programs.herdr.enable = lib.mkEnableOption "herdr";
                  programs.herdr.package = lib.mkOption {
                    type = lib.types.package;
                    default = pkgs.emptyDirectory;
                  };
                  xdg.configHome = lib.mkOption {
                    type = lib.types.str;
                    default = "/home/u/.config";
                  };
                  systemd.user.services = lib.mkOption {
                    type = lib.types.attrsOf lib.types.anything;
                    default = { };
                  };
                  assertions = lib.mkOption {
                    type = lib.types.listOf lib.types.anything;
                    default = [ ];
                  };
                };
              };
              eval = config:
                (lib.evalModules {
                  modules = [ self.homeModules.default home config ];
                  specialArgs = { inherit pkgs; };
                }).config;
              matrix = {
                enable = true;
                homeserver = "https://m.example.org/a%b\"c";
                owner = "@a:example.org";
                environmentFile = "/run/env";
                avatar = "/a.png";
              };
              bare = eval { programs.herdr = { enable = true; inherit matrix; }; };
              hears = eval {
                programs.herdr = {
                  enable = true;
                  matrix = matrix // {
                    transcribe.model = {
                      url = "https://e.org/ggml.bin";
                      hash = "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=";
                    };
                  };
                };
              };
              service = config: config.systemd.user.services.herdr-matrix;
              path = config: lib.findFirst (lib.hasPrefix "\"PATH=") "" (service config).Service.Environment;
              pass = name: assertion: assert assertion; pkgs.runCommand name { } "touch $out";
            in
            {
              # Known advisories (RustSec) against Cargo.lock, offline: the
              # database is the pinned `advisory-db` input (`nix flake update
              # advisory-db` for news)
              audit = pkgs.runCommand "herdr-matrix-audit"
                {
                  nativeBuildInputs = [ pkgs.cargo-audit ];
                } ''
                HOME=$TMPDIR cargo-audit audit -n -d ${inputs.advisory-db} -f ${./Cargo.lock}
                touch $out
              '';
              # Every option's description and defaultText evaluate
              module-options =
                (pkgs.nixosOptionsDoc {
                  options =
                    (lib.evalModules {
                      modules = [ self.homeModules.default { _module.check = false; } ];
                      specialArgs = { inherit pkgs; };
                    }).options.programs.herdr.matrix;
                }).optionsJSON;
              # The unit: `"`, `\\` and `%` escaped for systemd, tools on PATH and
              # the model service only with a model
              module-config = pass "herdr-matrix-module-config" (
                lib.elem "\"HERDR_MATRIX_HOMESERVER=https://m.example.org/a%%b\\\"c\""
                  (service bare).Service.Environment
                && (service bare).Unit.Wants == [ ]
                && !(bare.systemd.user.services ? herdr-matrix-models)
                && !(lib.hasInfix "whisper" (path bare))
                && (service hears).Unit.Wants == [ "herdr-matrix-models.service" ]
                && lib.hasInfix "whisper-cpp" (path hears)
                && lib.hasInfix "ffmpeg" (path hears)
              );
              module-assertion = pass "herdr-matrix-module-assertion" (
                lib.elem
                  {
                    assertion = false;
                    message = "programs.herdr.matrix.enable needs programs.herdr.enable.";
                  }
                  (eval { programs.herdr.matrix = matrix; }).assertions
              );
              # A model is kept only with its hash, and only a new one restarts the bot
              models-script =
                let
                  weights = pkgs.writeText "model.bin" "weights";
                  systemctl = pkgs.writeShellScriptBin "systemctl" ''echo "$*" >> restarts'';
                  fetch = text: lib.getExe (pkgs.callPackage ./models.nix {
                    systemd = systemctl;
                    dir = "models";
                    files = [{
                      path = "models/m.bin";
                      url = "file://${weights}";
                      sha256 = builtins.hashString "sha256" text;
                    }];
                  });
                in
                pkgs.runCommand "herdr-matrix-models-script" { } ''
                  if ${fetch "other"}; then exit 1; fi
                  test ! -e models/m.bin
                  test ! -e models/m.bin.part
                  test ! -e restarts
                  ${fetch "weights"}
                  test "$(cat models/m.bin)" = weights
                  test "$(cat restarts)" = "--user try-restart --no-block herdr-matrix.service"
                  ${fetch "weights"}
                  test "$(wc -l < restarts)" = 1
                  touch $out
                '';
            };

          # Packages -> 'nix build' or 'nix run'
          packages = {
            herdr-matrix = pkgs.callPackage ./package.nix { };
            default = self'.packages.herdr-matrix;
          };
        };

      flake = {
        homeModules = {
          herdr-matrix = import ./module.nix;
          default = self.homeModules.herdr-matrix;
        };
      };
    };
}
