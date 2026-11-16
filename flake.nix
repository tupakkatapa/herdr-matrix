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
                  systemd.user.timers = lib.mkOption {
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
              # Prints the arguments the prompt units call it with, one per line
              echo = pkgs.writeShellScriptBin "herdr-matrix" ''printf '%s\n' "$@"'';
              agents = eval {
                programs.herdr = {
                  enable = true;
                  matrix = matrix // {
                    package = echo;
                    agents = {
                      kate = {
                        cwd = "/w/a b";
                        name = "Kate";
                        prefix = false;
                        avatar = "/k.png";
                        prompts.wake = { text = "it's 100% $HOME \"q\""; onCalendar = "hourly"; };
                        prompts.mail = { text = "mail"; onCalendar = "*:0/15"; };
                      };
                      quiet.cwd = "/w/q";
                    };
                  };
                };
              };
              service = config: config.systemd.user.services.herdr-matrix;
              agentsFile = config: lib.findFirst (lib.hasPrefix "\"HERDR_MATRIX_AGENTS=") null (service config).Service.Environment;
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
                && lib.elem "\"HERDR_MATRIX_SPAWN_PREFIX=true\"" (service bare).Service.Environment
                && (service bare).Unit.Wants == [ ]
                && !(bare.systemd.user.services ? herdr-matrix-models)
                && !(lib.hasInfix "whisper" (path bare))
                && (service hears).Unit.Wants == [ "herdr-matrix-models.service" ]
                && lib.hasInfix "whisper-cpp" (path hears)
                && lib.hasInfix "ffmpeg" (path hears)
              );
              # Per-agent settings: none, no file and no prompt units; else the
              # file, and a service and timer per prompt that pass its text whole
              module-agents =
                let
                  units = agents.systemd.user.services;
                  timers = agents.systemd.user.timers;
                  names = [ "herdr-matrix-prompt-kate-mail" "herdr-matrix-prompt-kate-wake" ];
                  file = lib.removeSuffix "\"" (lib.removePrefix "\"HERDR_MATRIX_AGENTS=" (agentsFile agents));
                  wake = pkgs.writeText "wake" "prompt\n--cwd\n/w/a b\nit's 100% $HOME \"q\"\n";
                in
                assert agentsFile bare == null
                && lib.attrNames bare.systemd.user.services == [ "herdr-matrix" ]
                && bare.systemd.user.timers == { }
                && lib.attrNames units == [ "herdr-matrix" ] ++ names
                && lib.attrNames timers == names
                && units.herdr-matrix-prompt-kate-wake.Unit == {
                  Description = "herdr-matrix prompt kate-wake";
                  After = [ "herdr.service" "herdr-matrix.service" ];
                }
                && units.herdr-matrix-prompt-kate-wake.Service.Type == "oneshot"
                && timers.herdr-matrix-prompt-kate-wake == {
                  Unit.Description = "herdr-matrix prompt kate-wake";
                  Timer.OnCalendar = "hourly";
                  Install.WantedBy = [ "timers.target" ];
                }
                && timers.herdr-matrix-prompt-kate-mail.Timer.OnCalendar == "*:0/15";
                pkgs.runCommand "herdr-matrix-module-agents" { nativeBuildInputs = [ pkgs.jq ]; } ''
                  ${units.herdr-matrix-prompt-kate-wake.Service.ExecStart} > wake
                  diff wake ${wake}
                  test "$(${units.herdr-matrix-prompt-kate-mail.Service.ExecStart})" = "$(printf '%s\n' prompt --cwd '/w/a b' mail)"
                  jq -e '. == [
                    {"cwd": "/w/a b", "name": "Kate", "prefix": false, "avatar": "/k.png"},
                    {"cwd": "/w/q", "name": null, "prefix": null, "avatar": null}
                  ]' ${file}
                  touch $out
                '';
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
              # Every option of the gate's module documents
              gate-options =
                (pkgs.nixosOptionsDoc {
                  options =
                    (lib.evalModules {
                      modules = [ self.nixosModules.default { _module.check = false; } ];
                      specialArgs = { inherit pkgs; };
                    }).options.services.herdr-matrix-gate;
                }).optionsJSON;
            }
            // lib.optionalAttrs pkgs.stdenv.isLinux {
              gate = import ./gate-test.nix {
                inherit pkgs;
                module = self.nixosModules.default;
              };
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
        nixosModules = {
          herdr-matrix-gate = import ./gate.nix;
          default = self.nixosModules.herdr-matrix-gate;
        };
      };
    };
}
