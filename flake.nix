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
              # The parts of `programs.herdr` the module reads
              herdr = {
                options = {
                  programs.herdr.enable = lib.mkEnableOption "herdr";
                  programs.herdr.package = lib.mkOption {
                    type = lib.types.package;
                    default = pkgs.emptyDirectory;
                  };
                };
              };
              # The parts of Home Manager the module reads
              home = { config, ... }: {
                options = {
                  home.username = lib.mkOption {
                    type = lib.types.str;
                    default = "u";
                  };
                  home.homeDirectory = lib.mkOption {
                    type = lib.types.str;
                    default = "/home/u";
                  };
                  xdg.configHome = lib.mkOption {
                    type = lib.types.str;
                    default = "/home/u/.config";
                  };
                  xdg.cacheHome = lib.mkOption {
                    type = lib.types.str;
                    default = "${config.home.homeDirectory}/.cache";
                  };
                  xdg.stateHome = lib.mkOption {
                    type = lib.types.str;
                    default = "${config.home.homeDirectory}/.local/state";
                  };
                  home.packages = lib.mkOption {
                    type = lib.types.listOf lib.types.package;
                    default = [ ];
                  };
                  systemd.user.services = lib.mkOption {
                    type = lib.types.attrsOf lib.types.anything;
                    default = { };
                  };
                  systemd.user.timers = lib.mkOption {
                    type = lib.types.attrsOf lib.types.anything;
                    default = { };
                  };
                  systemd.user.sockets = lib.mkOption {
                    type = lib.types.attrsOf lib.types.anything;
                    default = { };
                  };
                  assertions = lib.mkOption {
                    type = lib.types.listOf lib.types.anything;
                    default = [ ];
                  };
                  warnings = lib.mkOption {
                    type = lib.types.listOf lib.types.str;
                    default = [ ];
                  };
                };
              };
              evalWith = args: config:
                (lib.evalModules {
                  modules = [ self.homeModules.default home herdr config ];
                  specialArgs = { inherit pkgs; } // args;
                }).config;
              eval = evalWith { };
              # The parts of NixOS the gate's module reads and writes
              system = {
                options = {
                  users.users = lib.mkOption {
                    type = lib.types.attrsOf lib.types.anything;
                    default = { };
                  };
                  users.groups = lib.mkOption {
                    type = lib.types.attrsOf lib.types.anything;
                    default = { };
                  };
                  environment.systemPackages = lib.mkOption {
                    type = lib.types.listOf lib.types.anything;
                    default = [ ];
                  };
                  systemd.sockets = lib.mkOption {
                    type = lib.types.attrsOf lib.types.anything;
                    default = { };
                  };
                  systemd.services = lib.mkOption {
                    type = lib.types.attrsOf lib.types.anything;
                    default = { };
                  };
                };
              };
              # `home-manager.users` of Home Manager's NixOS module, with the home module
              homeManager = { config, ... }: {
                options.home-manager.users = lib.mkOption {
                  type = lib.types.attrsOf (lib.types.submoduleWith {
                    modules = [
                      self.homeModules.default
                      home
                      herdr
                      ({ name, ... }: {
                        home.username = lib.mkDefault name;
                        home.homeDirectory = lib.mkDefault "/home/${name}";
                      })
                    ];
                    specialArgs = { inherit pkgs; osConfig = config; };
                  });
                  default = { };
                };
              };
              nixos = modules:
                (lib.evalModules {
                  modules = [ self.nixosModules.default system ] ++ modules;
                }).config;
              matrix = {
                enable = true;
                homeserver = "https://m.example.org/a%b\"c";
                owner = "@a:example.org";
                environmentFile = "/run/env";
                avatar = "/a.png";
              };
              bare = eval { programs.herdr = { enable = true; inherit matrix; }; };
              gate = { enable = true; tokenFile = "/run/gate-token"; };
              os = { herdr-matrix.gate.present = true; };
              gated = args: evalWith args { programs.herdr = { enable = true; matrix = matrix // { inherit gate; }; }; };
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
              # The agents of `agents` with `displayName` for the old `name`, and
              # with both, where `displayName` wins
              renamed = name: eval {
                programs.herdr = {
                  enable = true;
                  matrix = matrix // {
                    package = echo;
                    agents = {
                      kate = {
                        cwd = "/w/a b";
                        displayName = "Kate";
                        prefix = false;
                        avatar = "/k.png";
                        prompts.wake = { text = "it's 100% $HOME \"q\""; onCalendar = "hourly"; };
                        prompts.mail = { text = "mail"; onCalendar = "*:0/15"; };
                      } // lib.optionalAttrs (name != null) { inherit name; };
                      quiet.cwd = "/w/q";
                    };
                  };
                };
              };
              service = config: config.systemd.user.services.herdr-matrix;
              agentsFile = config: lib.findFirst (lib.hasPrefix "\"HERDR_MATRIX_AGENTS=") null (service config).Service.Environment;
              path = config: lib.findFirst (lib.hasPrefix "\"PATH=") "" (service config).Service.Environment;
              pass = name: assertion: assert assertion; pkgs.runCommand name { } "touch $out";
              fails = message: config: lib.elem { assertion = false; inherit message; } config.assertions;
              ok = config: lib.all (a: a.assertion) config.assertions;
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
              # the model service only with a model; the command on the user's PATH;
              # the token from either file; the old names of `ownChat` with a warning
              module-config =
                let
                  withSettings = settings: eval { programs.herdr = { enable = true; matrix = matrix // settings; }; };
                  tokened = eval {
                    programs.herdr = {
                      enable = true;
                      matrix = removeAttrs matrix [ "environmentFile" ] // { tokenFile = "/run/tok%en"; workspace = "/w/%x"; };
                    };
                  };
                  own = withSettings { ownChat = { avatar = "/o.png"; prefix = false; }; };
                  spawn = withSettings { spawnAvatar = "/o.png"; spawnPrefix = false; };
                  renamedTo = option: lib.any (lib.hasInfix "has been renamed to `programs.herdr.matrix.ownChat.${option}'") spawn.warnings;
                in
                pass "herdr-matrix-module-config" (
                  lib.elem "\"HERDR_MATRIX_HOMESERVER=https://m.example.org/a%%b\\\"c\""
                    (service bare).Service.Environment
                  && lib.elem "\"HERDR_MATRIX_SPAWN_PREFIX=true\"" (service bare).Service.Environment
                  && lib.elem "\"HERDR_MATRIX_WORKSPACE=/home/u/Workspace\"" (service bare).Service.Environment
                  && (service bare).Service.EnvironmentFile == "/run/env"
                  && !((service bare).Service ? LoadCredential)
                  && (service tokened).Service.LoadCredential == "token:/run/tok%%en"
                  && !((service tokened).Service ? EnvironmentFile)
                  && lib.elem "\"HERDR_MATRIX_WORKSPACE=/w/%%x\"" (service tokened).Service.Environment
                  && lib.elem "\"HERDR_MATRIX_SPAWN_AVATAR=/o.png\"" (service own).Service.Environment
                  && lib.elem "\"HERDR_MATRIX_SPAWN_PREFIX=false\"" (service own).Service.Environment
                  && own.warnings == [ ]
                  && service spawn == service own
                  && lib.length spawn.warnings == 2
                  && renamedTo "avatar"
                  && renamedTo "prefix"
                  && (service bare).Unit.Wants == [ ]
                  && !(bare.systemd.user.services ? herdr-matrix-models)
                  && !(lib.hasInfix "whisper" (path bare))
                  && (service hears).Unit.Wants == [ "herdr-matrix-models.service" ]
                  && lib.hasInfix "whisper-cpp" (path hears)
                  && lib.hasInfix "ffmpeg" (path hears)
                  && agents.home.packages == [ echo ]
                );
              # Per-agent settings: none, no file and no prompt units; else the
              # file, and a service and timer per prompt that pass its text whole.
              # The old `name` gives the file of `displayName`, with a warning.
              module-agents =
                let
                  named = renamed null;
                  both = renamed "Old";
                  warned = value: [ "programs.herdr.matrix.agents.kate.name = \"${value}\": the option is named displayName. Set programs.herdr.matrix.agents.kate.displayName." ];
                  started = eval {
                    programs.herdr = {
                      enable = true;
                      matrix = matrix // { agents.sam = { cwd = "/w/s"; displayName = "Sam"; chat = true; autostart = true; }; };
                    };
                  };
                  units = agents.systemd.user.services;
                  timers = agents.systemd.user.timers;
                  names = [ "herdr-matrix-prompt-kate-mail" "herdr-matrix-prompt-kate-wake" ];
                  file = lib.removeSuffix "\"" (lib.removePrefix "\"HERDR_MATRIX_AGENTS=" (agentsFile agents));
                  wake = pkgs.writeText "wake" "prompt\n--cwd\n/w/a b\nit's 100% $HOME \"q\"\n";
                in
                assert agentsFile bare == null
                && agentsFile named == agentsFile agents
                && agentsFile both == agentsFile agents
                && named.warnings == [ ]
                && agents.warnings == warned "Kate"
                && both.warnings == warned "Old"
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
                  jq -e '. == [
                    {"cwd": "/w/s", "name": "Sam", "prefix": null, "avatar": null, "chat": true, "autostart": true}
                  ]' ${lib.removeSuffix "\"" (lib.removePrefix "\"HERDR_MATRIX_AGENTS=" (agentsFile started))}
                  touch $out
                '';
              # Settings that cannot work fail evaluation with the option, its value and
              # the fix; settings the bot applies in part give a warning
              module-assertion =
                let
                  withSettings = settings: eval { programs.herdr = { enable = true; matrix = matrix // settings; }; };
                  withAgents = agents: withSettings { inherit agents; };
                  # Without a module that defines `programs.herdr.enable`
                  alone = (lib.evalModules {
                    modules = [ self.homeModules.default home { programs.herdr.matrix = matrix; } ];
                    specialArgs = { inherit pkgs; };
                  }).config;
                  homeserver = url: withSettings { homeserver = url; };
                  badHomeserver = url: fails "programs.herdr.matrix.homeserver = ${builtins.toJSON url}: the bot sends its token with each request, so the URL must be https://, or http:// to localhost, 127.0.0.1 or [::1]. Use the https:// URL of the homeserver."
                    (homeserver url);
                  badOwner = owner: fails "programs.herdr.matrix.owner = ${builtins.toJSON owner}: a Matrix user id has the form @local:server. Use the full id, such as \"@alice:example.org\"."
                    (withSettings { inherit owner; });
                  prompted = id: withAgents { ${id} = { cwd = "/w/a"; prompts.wake = { text = "wake"; onCalendar = "hourly"; }; }; };
                  shared = withAgents { a.cwd = "/w/x"; b.cwd = "/w/x/"; c.cwd = "/w/x"; d.cwd = "/w/x/y"; };
                in
                pass "herdr-matrix-module-assertion" (
                  fails "programs.herdr.enable = false: programs.herdr.matrix needs herdr. Set programs.herdr.enable = true."
                    (eval { programs.herdr.matrix = matrix; })
                  && fails "programs.herdr.matrix needs a module that defines programs.herdr.enable and programs.herdr.package." alone
                  && ok bare
                  && lib.all badHomeserver [ "http://m.example.org" "http://localhost.example.org" "http://127.0.0.1.example.org" "http://[::2]" "ftp://localhost" "m.example.org" "" ]
                  && lib.all (url: ok (homeserver url)) [ "https://m.example.org" "http://localhost" "http://localhost:8008/x" "http://127.0.0.1:8008" "http://127.1.2.3" "http://[::1]:8008" "http://[0:0:0:0:0:0:0:1]/" ]
                  && lib.all badOwner [ "alice" "@alice" "alice:example.org" "@:example.org" ]
                  && fails "programs.herdr.matrix.agents.nora.cwd = \"Workspace/nora\": the folder must be an absolute path, as herdr reports it. Use a path such as \"\${config.home.homeDirectory}/Workspace/nora\"."
                    (withAgents { nora.cwd = "Workspace/nora"; })
                  && shared.warnings == [ "programs.herdr.matrix.agents.a.cwd = \"/w/x\": the agents a, b and c work in the same folder, and the bot applies the settings of one agent per folder. Give each agent its own cwd." ]
                  && ok shared
                  && ok (withAgents { a.cwd = "/w/x"; b.cwd = "/w/x/y"; c.cwd = "/w/xy"; })
                  && (withAgents { a.cwd = "/w/x"; b.cwd = "/w/x/y"; c.cwd = "/w/xy"; }).warnings == [ ]
                  && fails "programs.herdr.matrix.tokenFile = null: the bot needs its as_token from tokenFile or environmentFile. Set programs.herdr.matrix.tokenFile."
                    (eval { programs.herdr = { enable = true; matrix = removeAttrs matrix [ "environmentFile" ]; }; })
                  && fails "programs.herdr.matrix.tokenFile = \"/run/tok\": programs.herdr.matrix.environmentFile is set too, and the bot takes its as_token from one file. Remove one of them."
                    (withSettings { tokenFile = "/run/tok"; })
                  && ok (eval { programs.herdr = { enable = true; matrix = removeAttrs matrix [ "environmentFile" ] // { tokenFile = "/run/tok"; }; }; })
                  && fails "programs.herdr.matrix.agents.\"a b\": the attribute name of an agent with prompts goes into systemd unit names, so it takes only letters, digits, `-`, `_`, `.`, `:`, `@` and `\\`. Rename it, such as \"a-b\"."
                    (prompted "a b")
                  && fails "programs.herdr.matrix.agents.a.prompts.\"x/y\": the name of a prompt goes into systemd unit names, so it takes only letters, digits, `-`, `_`, `.`, `:`, `@` and `\\`. Rename it, such as \"x-y\"."
                    (withAgents { a = { cwd = "/w/a"; prompts."x/y" = { text = "t"; onCalendar = "hourly"; }; }; })
                  && lib.all (id: ok (prompted id)) [ "a.b" "a_b-c" "a:b" "a@b" ]
                  && ok (withAgents { "a b".cwd = "/w/a"; })
                );
              # A confined agent: its command on the user's PATH, its places in the
              # agents file, and bwrap with only what it is granted; an agent without
              # a sandbox stays as it is, and a bad sandbox fails evaluation
              module-sandbox =
                let
                  confined = base: agents: evalWith { osConfig = os; } {
                    programs.herdr = {
                      enable = true;
                      matrix = matrix // { package = echo; inherit agents; } // base;
                    };
                  };
                  nora = sandbox: extra: confined { inherit gate; } {
                    nora = { cwd = "/w/n o"; inherit sandbox; } // extra;
                    quiet.cwd = "/w/q";
                  };
                  full = nora
                    {
                      enable = true;
                      program = [ "/bin/probe" "a b" ];
                      environment.K = "it's $x";
                      allow = { readPaths = [ "/r o" ]; network = true; chats = { read = true; send = true; rooms = [ "!a:x" ]; }; sendFiles = true; };
                    }
                    { };
                  bare = nora { enable = true; allow = { network = false; sendFiles = false; }; } { };
                  plain = nora { enable = true; } { };
                  pi = nora { enable = true; } { kind = "pi"; };
                  ported = nora
                    {
                      enable = true;
                      files = { "models.json" = "/m.json"; "a b/s.json" = "/s o.json"; };
                      allow = { network = false; ports = { "11434" = "10.23.0.18:11444"; "2000" = "[::1]:80"; }; };
                    }
                    { kind = "pi"; };
                  port = "herdr-matrix-port-nora-11434";
                  at = "programs.herdr.matrix.agents.nora";
                  badFile = key: fails "${at}.sandbox.files.${lib.strings.escapeNixIdentifier key}: the key must be a path in the state directory, without an empty, `.` or `..` part. Use a relative path, such as \"models.json\"."
                    (nora { enable = true; files.${key} = "/m.json"; } { });
                  badPort = key: fails "${at}.sandbox.allow.ports.${lib.strings.escapeNixIdentifier key}: the key is the port inside the sandbox, a number from 1024 to 65535. Use a port in that range."
                    (nora { enable = true; allow = { network = false; ports.${key} = "h:1"; }; } { });
                  badAddress = address: fails "${at}.sandbox.allow.ports.\"2000\" = ${builtins.toJSON address}: the address outside must be `host:port`. Write it as host:port, such as \"10.23.0.18:11444\" or \"[::1]:80\"."
                    (nora { enable = true; allow = { network = false; ports."2000" = address; }; } { });
                  unconfined = set: fails "${at}: ${set}, but sandbox.enable is false. Set sandbox.enable = true, or remove ${if lib.hasSuffix "are set" set then "them" else "it"}.";
                  inHome = cwd: confined { } { nora = { inherit cwd; sandbox.enable = true; }; };
                  wrapper = config: "${lib.findFirst (p: p.name == "herdr-matrix-agent-nora") null config.home.packages}/bin/herdr-matrix-agent-nora";
                  file = config: lib.removeSuffix "\"" (lib.removePrefix "\"HERDR_MATRIX_AGENTS=" (agentsFile config));
                  places = {
                    id = "nora";
                    state = "/home/u/.local/state/herdr-matrix/agents/nora/claude";
                    uploads = "/home/u/.cache/herdr-matrix/uploads/nora";
                    files = "/home/u/.cache/herdr-matrix/agents/nora";
                  };
                  piState = "/home/u/.local/state/herdr-matrix/agents/nora/pi";
                in
                assert ok full
                && ok bare
                && map lib.getName full.home.packages == [ "herdr-matrix" "herdr-matrix-agent-nora" ]
                && unconfined "sandbox.allow.sendFiles is set" (nora { allow.sendFiles = false; } { })
                && unconfined "sandbox.program is set" (nora { program = [ "x" ]; } { })
                && unconfined "sandbox.program is set" (nora { program = [ "claude" ]; } { kind = "pi"; })
                && unconfined "sandbox.program, sandbox.allow.network and sandbox.allow.chats.rooms are set"
                  (nora { program = [ "x" ]; allow = { network = false; chats.rooms = [ "!a:x" ]; }; } { })
                && ok (nora { program = [ "pi" ]; } { kind = "pi"; })
                && ok pi
                && fails "${at}.sandbox.allow.chats.read = true: the agent's chats go through the chat gate. Set programs.herdr.matrix.gate.enable = true." (confined { } {
                  nora = { cwd = "/w/n"; sandbox = { enable = true; allow.chats.read = true; }; };
                })
                && fails "${at}.sandbox.allow.chats.send = true: the agent's chats go through the chat gate. Set programs.herdr.matrix.gate.enable = true." (confined { } {
                  nora = { cwd = "/w/n"; sandbox = { enable = true; allow.chats.send = true; }; };
                })
                && fails "${at}.sandbox.allow.chats.rooms = [ \"!a:x\" ]: the rooms limit sandbox.allow.chats.read and send, and neither is on. Set sandbox.allow.chats.read or sandbox.allow.chats.send to true, or remove the rooms."
                  (nora { enable = true; allow.chats.rooms = [ "!a:x" ]; } { })
                && fails "programs.herdr.matrix.agents.\"no.ra\": the attribute name of a confined agent names its command, directories and socket, so it takes only letters, digits, `_` and `-`. Rename it, such as \"no-ra\"." (confined { } {
                  "no.ra" = { cwd = "/w/n"; sandbox.enable = true; };
                })
                && lib.all (cwd: fails "${at}.cwd = ${builtins.toJSON cwd}: the folder of a confined agent must not hold the home directory. Use a folder inside it, such as \"/home/u/Workspace/nora\"." (inHome cwd)) [ "/home/u" "/home/u/" "/home" "/" "/home//u" ]
                && lib.all (cwd: fails "${at}.cwd = ${builtins.toJSON cwd}: the folder of a confined agent must not hold a `.` or `..` part. Write the path without them." (inHome cwd)) [ "/home/u/." "/home/u/x/.." "/home/u/./x" ]
                && ok (inHome "/home/u/w")
                && ok (inHome "/home/u/x/")
                && ok (inHome "/home/uw")
                && ok ported
                && unconfined "sandbox.files is set" (nora { files."m" = "/m"; } { })
                && unconfined "sandbox.allow.network and sandbox.allow.ports are set" (nora { allow = { network = false; ports."2000" = "h:1"; }; } { })
                && fails "${at}.sandbox.allow.network = true: sandbox.allow.ports is the way to reach a service without the network, so it needs the network off. Set sandbox.allow.network = false, or remove sandbox.allow.ports."
                  (nora { enable = true; allow.ports."2000" = "h:1"; } { })
                && lib.all badFile [ "/abs" "../x" "a/../b" "a/" "a//b" "." "" ]
                && ok (nora { enable = true; files."a/.b/c" = "/m"; } { })
                && lib.all badPort [ "80" "1023" "65536" "01024" "0x400" "" "999999999999999999999" ]
                && lib.all badAddress [ "h" ":1" "h:" "h:p" "a b:1" "h:1 x" "::1:80" "h:1%2" ]
                && ok (nora { enable = true; allow = { network = false; ports = { "1024" = "my-host.lan:1"; "65535" = "10.0.0.1:65535"; }; }; } { })
                && bare.warnings == [ "${at}.sandbox.allow.network = false: the agent has no network and no sandbox.allow.ports, so it cannot reach a model. Name the model's address in sandbox.allow.ports, or set sandbox.allow.network = true." ]
                && ported.warnings == [ ]
                && plain.warnings == [ ]
                && plain.systemd.user.sockets == { }
                && lib.attrNames ported.systemd.user.sockets == [ port "herdr-matrix-port-nora-2000" ]
                && ported.systemd.user.sockets.${port} == {
                  Unit.Description = "herdr-matrix port 11434 of nora";
                  Socket = { ListenStream = "%t/herdr-matrix/ports/nora/11434"; SocketMode = "0600"; DirectoryMode = "0700"; };
                  Install.WantedBy = [ "sockets.target" ];
                }
                && ported.systemd.user.services.${port} == {
                  Unit = { Description = "herdr-matrix port 11434 of nora"; Requires = [ "${port}.socket" ]; After = [ "${port}.socket" ]; };
                  Service.ExecStart = "${pkgs.systemd}/lib/systemd/systemd-socket-proxyd --exit-idle-time=5min 10.23.0.18:11444";
                }
                && ported.systemd.user.services."herdr-matrix-port-nora-2000".Service.ExecStart
                == "${pkgs.systemd}/lib/systemd/systemd-socket-proxyd --exit-idle-time=5min [::1]:80";
                pkgs.runCommand "herdr-matrix-module-sandbox" { nativeBuildInputs = [ pkgs.jq ]; } ''
                  jq -e '. == [
                    {"cwd": "/w/n o", "name": null, "prefix": null, "avatar": null, "sandbox": ${builtins.toJSON places}},
                    {"cwd": "/w/q", "name": null, "prefix": null, "avatar": null}
                  ]' ${file full}
                  jq -e '.[0] == {"cwd": "/w/n o", "name": null, "prefix": null, "avatar": null, "kind": "pi",
                    "sandbox": ${builtins.toJSON (places // { state = piState; })}}' ${file pi}
                  full=${wrapper full}
                  bare=${wrapper bare}
                  for want in \
                    "--unshare-all --share-net --die-with-parent --clearenv" \
                    "--ro-bind /nix/store /nix/store --ro-bind /etc /etc --ro-bind /run/current-system /run/current-system" \
                    "--proc /proc --dev /dev --tmpfs /tmp --tmpfs /home/u" \
                    "--bind '/w/n o' '/w/n o' --bind ${places.state} ${places.state} --ro-bind ${places.files} ${places.files}" \
                    "--bind ${places.uploads} ${places.uploads}" \
                    "--bind /run/herdr-matrix-gate/u.nora /run/herdr-matrix-gate/u.nora" \
                    "--ro-bind '/r o' '/r o' --chdir '/w/n o'" \
                    "--setenv HOME /home/u --setenv CLAUDE_CONFIG_DIR ${places.state} --setenv HERDR_AGENT claude" \
                    "--setenv HERDR_MATRIX_GATE_SOCKET /run/herdr-matrix-gate/u.nora/socket" \
                    "--setenv HERDR_MATRIX_UPLOADS ${places.uploads}" \
                    "--setenv K 'it'\'''s \$x'" \
                    "-- /bin/probe 'a b' \"\$@\""
                  do
                    grep -qF -- "$want" "$full" || { echo "missing: $want"; exit 1; }
                  done
                  grep -qF -- "--unshare-all --die-with-parent --clearenv" "$bare"
                  grep -qF -- "-- claude \"\$@\"" "$bare"
                  for absent in --share-net --new-session HERDR_MATRIX_GATE_SOCKET HERDR_MATRIX_UPLOADS /run/herdr-matrix-gate /uploads/; do
                    if grep -qF -- "$absent" "$bare"; then echo "present: $absent"; exit 1; fi
                  done
                  if grep -qF -- --new-session "$full"; then exit 1; fi
                  plain=${wrapper plain}
                  grep -qF -- "--unshare-all --share-net --die-with-parent" "$plain"
                  grep -qF -- "--ro-bind-try /run/systemd/resolve /run/systemd/resolve" "$plain"
                  grep -qF -- "--setenv HERDR_MATRIX_UPLOADS" "$plain"
                  if grep -qF -- /run/herdr-matrix-gate "$plain"; then exit 1; fi
                  pi=${wrapper pi}
                  for want in \
                    "mkdir -p -- ${piState} ${places.files} ${places.uploads}" \
                    "--bind '/w/n o' '/w/n o' --bind ${piState} ${piState} --ro-bind ${places.files} ${places.files}" \
                    "--setenv HOME /home/u --setenv PI_CODING_AGENT_DIR ${piState} --setenv HERDR_AGENT pi --setenv HERDR_MATRIX_UPLOADS" \
                    "-- pi \"\$@\""
                  do
                    grep -qF -- "$want" "$pi" || { echo "missing: $want"; exit 1; }
                  done
                  if grep -qF -- CLAUDE_CONFIG_DIR "$pi"; then exit 1; fi
                  for launcher in "$full" "$bare" "$plain" "$pi"; do
                    if grep -qF -- herdr-matrix/ports "$launcher"; then echo "ports: $launcher"; exit 1; fi
                  done
                  ported=${wrapper ported}
                  inner=$(grep -o '/nix/store/[^ ]*-herdr-matrix-agent-nora-ports' "$ported")
                  for want in \
                    "--bind ${piState} ${piState} --ro-bind '/s o.json' '${piState}/a b/s.json' --ro-bind /m.json ${piState}/models.json --ro-bind ${places.files} ${places.files}" \
                    'ports=$XDG_RUNTIME_DIR/herdr-matrix/ports/nora' \
                    "--ro-bind \"\$ports\" \"\$ports\" -- $inner \"\$ports\" pi \"\$@\""
                  do
                    grep -qF -- "$want" "$ported" || { echo "missing: $want"; exit 1; }
                  done
                  for want in \
                    "TCP-LISTEN:2000,bind=127.0.0.1,fork,reuseaddr UNIX-CONNECT:\"\$dir\"/2000" \
                    "TCP-LISTEN:11434,bind=127.0.0.1,fork,reuseaddr UNIX-CONNECT:\"\$dir\"/11434" \
                    "${pkgs.socat}/bin/socat" \
                    "unset HERDR_AGENT" \
                    '</dev/null >/dev/null 2>&1 &'
                  do
                    grep -qF -- "$want" "$inner" || { echo "missing in $inner: $want"; exit 1; }
                  done
                  test "$(grep -v '^$' "$inner" | tail -n 1)" = 'exec "$@"'
                  touch $out
                '';
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
              # The gate needs the bot and the NixOS module, and leaves the bot's units alone
              module-gate =
                let
                  needsOs = fails "programs.herdr.matrix.gate.enable = true: the gate runs as a system service, which only the NixOS module makes; standalone Home Manager cannot run it. Import herdr-matrix.nixosModules.default in the NixOS configuration, and run Home Manager as its NixOS module.";
                in
                pass "herdr-matrix-module-gate" (
                  needsOs (gated { })
                  && needsOs (gated { osConfig = null; })
                  && needsOs (gated { osConfig = { }; })
                  && ok (gated { osConfig = os; })
                  && (gated { osConfig = os; }).systemd.user == bare.systemd.user
                  && fails "programs.herdr.matrix.enable = false: programs.herdr.matrix.gate.enable needs the chat. Set programs.herdr.matrix.enable = true."
                    (evalWith { osConfig = os; } { programs.herdr.matrix.gate = gate; })
                );
              # Without a user with the gate on, the system holds no gate unit, user or group
              gate-module =
                let
                  empty = config:
                    config.herdr-matrix.gate.present
                    && config.systemd.sockets == { }
                    && config.systemd.services == { }
                    && config.users.users == { }
                    && config.users.groups == { }
                    && config.environment.systemPackages == [ ];
                in
                pass "herdr-matrix-gate-module" (
                  empty (nixos [ ])
                  && empty (nixos [
                    homeManager
                    {
                      home-manager.users.alice.programs.herdr = { enable = true; inherit matrix; };
                      home-manager.users.bob = { };
                    }
                  ])
                );
              # A confined agent with chats gets a socket and service of its own, with
              # its commands and rooms; the user's own units stay as they are
              gate-agents =
                let
                  withAgents = agents: nixos [
                    homeManager
                    {
                      home-manager.users.alice.programs.herdr = {
                        enable = true;
                        matrix = matrix // { inherit gate agents; };
                      };
                    }
                  ];
                  plain = withAgents { quiet.cwd = "/w/q"; };
                  confined = withAgents {
                    quiet.cwd = "/w/q";
                    nora = { cwd = "/w/n"; sandbox = { enable = true; allow.chats = { read = true; rooms = [ "!a:x" "!b%:x" ]; }; }; };
                    mia = { cwd = "/w/m"; sandbox = { enable = true; allow.chats = { read = true; send = true; }; }; };
                    bare = { cwd = "/w/b"; sandbox.enable = true; };
                  };
                  service = config: name: config.systemd.services."herdr-matrix-gate-${name}@";
                  env = name: (service confined "alice.${name}").environment;
                in
                pass "herdr-matrix-gate-agents" (
                  lib.attrNames plain.systemd.sockets == [ "herdr-matrix-gate-alice" ]
                  && lib.attrNames confined.systemd.sockets
                  == [ "herdr-matrix-gate-alice" "herdr-matrix-gate-alice.mia" "herdr-matrix-gate-alice.nora" ]
                  && confined.systemd.sockets.herdr-matrix-gate-alice == plain.systemd.sockets.herdr-matrix-gate-alice
                  && service confined "alice" == service plain "alice"
                  && !((service plain "alice").environment ? HERDR_MATRIX_GATE_COMMANDS)
                  && confined.systemd.sockets."herdr-matrix-gate-alice.nora".socketConfig
                  == plain.systemd.sockets.herdr-matrix-gate-alice.socketConfig // {
                    ListenStream = "/run/herdr-matrix-gate/alice.nora/socket";
                  }
                  && (service confined "alice.nora").serviceConfig == (service plain "alice").serviceConfig
                  && env "nora" == (service plain "alice").environment // {
                    HERDR_MATRIX_GATE_COMMANDS = "rooms read search";
                    HERDR_MATRIX_GATE_ROOMS = "!a:x !b%%:x";
                  }
                  && env "mia" == (service plain "alice").environment // {
                    HERDR_MATRIX_GATE_COMMANDS = "rooms read search send";
                  }
                );
            }
            // lib.optionalAttrs pkgs.stdenv.isLinux {
              gate = import ./gate-test.nix {
                inherit pkgs homeManager;
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
