# A Matrix chat with the herdr agents; extends `programs.herdr`, whose
# `package` and state directory it shares
{ lib
, config
, pkgs
, ...
}@args:
let
  cfg = config.programs.herdr;
  # Only Home Manager's NixOS module passes it; the module system ignores a
  # `? null` default on a module argument
  osConfig = args.osConfig or null;
  stateDir = "${config.xdg.configHome}/herdr";

  # Models stay out of the closure: the service downloads them to the state directory
  download = lib.types.submodule {
    options = {
      url = lib.mkOption {
        type = lib.types.str;
        description = "URL of the file.";
      };
      hash = lib.mkOption {
        type = lib.types.str;
        description = "SRI sha256 of the file.";
      };
    };
  };
  prompt = lib.types.submodule {
    options = {
      text = lib.mkOption {
        type = lib.types.str;
        example = "wake";
        description = "The prompt.";
      };
      onCalendar = lib.mkOption {
        type = lib.types.str;
        example = "hourly";
        description = "When to send it, as a systemd calendar expression (`man systemd.time`).";
      };
    };
  };
  # The variable that names the state directory of each kind
  stateVariable = { claude = "CLAUDE_CONFIG_DIR"; pi = "PI_CODING_AGENT_DIR"; };
  sandbox = kind: lib.types.submodule {
    options = {
      enable = lib.mkEnableOption ''
        the sandbox: the command `herdr-matrix-agent-<id>` runs {option}`program` in bubblewrap, and `!new` starts it for {option}`cwd`.
        Inside, the agent has {option}`cwd` and its own state directory, read and write; the Nix store, `/etc`, `/run/current-system` and {option}`allow.readPaths`, read-only; an empty `/tmp` and home; no other process of the host; and only what {option}`allow` grants.
        Its state directory is `$XDG_STATE_HOME/herdr-matrix/agents/<id>/<kind>`, in `CLAUDE_CONFIG_DIR` for Claude Code and `PI_CODING_AGENT_DIR` for pi. Files from the owner are in `$XDG_CACHE_HOME/herdr-matrix/agents/<id>`, read-only.
        `HERDR_AGENT` names the kind, so herdr knows the agent.
        Start it in a pane with `exec herdr-matrix-agent-<id>`. An agent started without the command is not confined. Without this option, the agent has full trust: it runs as this user and can do what this user can'';
      program = lib.mkOption {
        type = lib.types.nonEmptyListOf lib.types.str;
        default = [ kind ];
        defaultText = lib.literalExpression ''[ kind ]'';
        example = [ "pi" "--provider" "ollama" "--model" "qwen3:8b" ];
        description = "The agent program and its arguments. A name is looked up in `PATH` inside the sandbox; a store path always works.";
      };
      environment = lib.mkOption {
        type = lib.types.attrsOf lib.types.str;
        default = { };
        example = { ANTHROPIC_BASE_URL = "http://127.0.0.1:11434"; };
        description = "Environment variables inside the sandbox. The agent gets only these, `HOME`, `PATH`, `TERM`, `LANG`, `HERDR_AGENT`, the variable of its state directory, and the variables of what {option}`allow` grants.";
      };
      files = lib.mkOption {
        type = lib.types.attrsOf lib.types.path;
        default = { };
        example = lib.literalExpression ''{ "models.json" = pkgs.writeText "models.json" (builtins.toJSON { /* … */ }); }'';
        description = ''
          Files in the agent's state directory. The key is a path in the state directory; the value is the file.
          The command binds each file read-only at its path: the agent reads it and cannot change it, and each start shows the file of the current generation. A missing file stops the start.
          The mount point stays in the state directory as an empty file after the agent exits.
        '';
      };
      allow = {
        readPaths = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ ];
          example = [ "/run/systemd/resolve" ];
          description = "Paths the agent reads and cannot write, inside the sandbox at the same path. A missing path stops the start.";
        };
        network = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = "The host's network, which an agent needs to reach its model. On, the agent reaches every service the host reaches, local ones and the host's abstract unix sockets too. Off, the agent has no network. On, `/run/systemd/resolve` is shown read-only when it exists, for name lookups.";
        };
        ports = lib.mkOption {
          type = lib.types.attrsOf lib.types.str;
          default = { };
          example = { "11434" = "10.23.0.18:11444"; };
          description = ''
            TCP addresses for an agent without {option}`network`. The key is a port inside the sandbox, from 1024 to 65535; the value is the address outside, `host:port`. Inside, `127.0.0.1:<key>` reaches the value, and no other address exists.
            Each port is a user socket `herdr-matrix-port-<id>-<port>.socket` on `$XDG_RUNTIME_DIR/herdr-matrix/ports/<id>/<port>`, only for this user; its service, `systemd-socket-proxyd`, connects to the value.
            The command binds that directory read-only, starts a `socat` per port on `127.0.0.1` in the sandbox, then the agent. While the directory is missing, the command stops with an error.
          '';
        };
        chats = {
          read = lib.mkOption {
            type = lib.types.bool;
            default = false;
            description = ''
              `herdr-matrix chats rooms`, `read` and `search`, through a socket of the chat gate for this agent only.
              The socket is `/run/herdr-matrix-gate/<user>.<id>/socket`, served by the system units `herdr-matrix-gate-<user>.<id>.socket` and `herdr-matrix-gate-<user>.<id>@.service`. It needs {option}`programs.herdr.matrix.gate.enable`.
            '';
          };
          send = lib.mkOption {
            type = lib.types.bool;
            default = false;
            description = "`herdr-matrix chats rooms` and `send`, through the same socket. Without {option}`read`, the agent sends and sees the names of the chats, not their messages.";
          };
          rooms = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [ ];
            example = [ "!abc:example.org" ];
            description = "The room ids of the bridged chats that {option}`read` and {option}`send` reach. Empty, they reach every bridged chat.";
          };
        };
        sendFiles = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = "`herdr-matrix send`: the agent posts files in its chat with the owner, who already gets every text of the agent. The bot takes the agent from the directory of the queue, `$XDG_CACHE_HOME/herdr-matrix/uploads/<id>`.";
        };
      };
    };
  };
  agent = lib.types.submodule ({ config, ... }: {
    options = {
      cwd = lib.mkOption {
        type = lib.types.str;
        example = "/home/alice/Workspace/assistant";
        description = "The working directory of the agent; a trailing `/` aside, it must equal the agent's. It takes no systemd specifier such as `%h`.";
      };
      kind = lib.mkOption {
        type = lib.types.enum [ "claude" "pi" ];
        default = "claude";
        description = "The agent program: Claude Code or pi. `!new` starts it in {option}`cwd`. It sets the default of {option}`sandbox.program` and the state directory in the sandbox. For an agent without a sandbox, the bot takes the kind from herdr.";
      };
      name = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "Kate";
        description = "The name of the agent's own chat user, in place of the agent's name. The agent keeps its herdr name for `!attach`, `!agents` and `herdr-matrix prompt`.";
      };
      prefix = lib.mkOption {
        type = lib.types.nullOr lib.types.bool;
        default = null;
        description = "Put {option}`displayName` before the name of the agent's own chat user. Unset, {option}`spawnPrefix` decides.";
      };
      avatar = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = "The picture of the agent's own chat user, a PNG. Unset, it is {option}`spawnAvatar`.";
      };
      prompts = lib.mkOption {
        type = lib.types.attrsOf prompt;
        default = { };
        example = { wake = { text = "wake"; onCalendar = "hourly"; }; };
        description = ''
          Prompts the agent gets on a schedule. Each one is a user service and timer `herdr-matrix-prompt-<id>-<name>`.
          The bot drops a prompt when no agent or more than one works in {option}`cwd`, when the agent works or waits on a choice, or when the bot is down for 10 minutes; `journalctl --user -u herdr-matrix` says why.
        '';
      };
      sandbox = lib.mkOption {
        type = sandbox config.kind;
        default = { };
        description = "The confinement of the agent.";
      };
    };
  });
  sandboxed = lib.filterAttrs (_: a: a.sandbox.enable) cfg.matrix.agents;
  # Paths of a confined agent, the same inside the sandbox and outside
  confined = id: kind: {
    state = "${config.xdg.stateHome}/herdr-matrix/agents/${id}/${kind}";
    uploads = "${config.xdg.cacheHome}/herdr-matrix/uploads/${id}";
    files = "${config.xdg.cacheHome}/herdr-matrix/agents/${id}";
    gate = "/run/herdr-matrix-gate/${config.home.username}.${id}";
  };
  # The ports of the confined agents: a user socket and service each
  ports = lib.concatLists (lib.mapAttrsToList
    (id: a: lib.mapAttrsToList
      (port: address: {
        name = "herdr-matrix-port-${id}-${port}";
        description = "herdr-matrix port ${port} of ${id}";
        inherit id port address;
      })
      a.sandbox.allow.ports)
    sandboxed);
  # Starts the listeners of `s.allow.ports`, in the directory of its first
  # argument, then the agent. herdr takes the agent from the processes of the
  # pane's foreground group: the listeners leave it, without `HERDR_AGENT`.
  listeners = id: s: pkgs.writeShellScript "herdr-matrix-agent-${id}-ports" (''
    dir=$1
    shift
  '' + lib.concatMapStrings
    (port: ''
      (unset HERDR_AGENT; exec ${lib.getExe' pkgs.util-linux "setsid"} ${lib.getExe pkgs.socat} TCP-LISTEN:${port},bind=127.0.0.1,fork,reuseaddr UNIX-CONNECT:"$dir"/${port}) </dev/null >/dev/null 2>&1 &
    '')
    (lib.attrNames s.allow.ports) + ''
    exec "$@"
  '');
  # Set apart from the defaults, so a sandbox option without `enable` is refused
  chats = s: s.allow.chats.read || s.allow.chats.send;
  customised = a:
    let s = a.sandbox; in s.program != [ a.kind ] || s.environment != { } || s.files != { } || s.allow.readPaths != [ ]
      || !s.allow.network || s.allow.ports != { } || chats s || s.allow.chats.rooms != [ ] || !s.allow.sendFiles;
  # No `--new-session`: herdr finds the agent in the pane's foreground
  # process group. bwrap applies `--clearenv` and `--setenv` in order.
  wrapper = id: a:
    let
      s = a.sandbox;
      p = confined id a.kind;
      ported = s.allow.ports != { };
      home = config.home.homeDirectory;
      bind = flag: path: [ flag path path ];
      setenv = name: value: [ "--setenv" name value ];
      args = [ "--unshare-all" ]
        ++ lib.optional s.allow.network "--share-net"
        ++ [ "--die-with-parent" "--clearenv" ]
        ++ lib.concatMap (bind "--ro-bind") [ "/nix/store" "/etc" "/run/current-system" ]
        ++ [ "--proc" "/proc" "--dev" "/dev" "--tmpfs" "/tmp" "--tmpfs" home ]
        # Where `/etc/resolv.conf` points with systemd-resolved
        ++ lib.optionals s.allow.network [ "--ro-bind-try" "/run/systemd/resolve" "/run/systemd/resolve" ]
        ++ bind "--bind" a.cwd
        ++ bind "--bind" p.state
        ++ lib.concatLists (lib.mapAttrsToList (key: file: [ "--ro-bind" "${file}" "${p.state}/${key}" ]) s.files)
        ++ bind "--ro-bind" p.files
        ++ lib.optionals s.allow.sendFiles (bind "--bind" p.uploads)
        ++ lib.optionals (chats s) (bind "--bind" p.gate)
        ++ lib.concatMap (bind "--ro-bind") s.allow.readPaths
        ++ [ "--chdir" a.cwd ]
        ++ setenv "HOME" home
        ++ setenv stateVariable.${a.kind} p.state
        ++ setenv "HERDR_AGENT" a.kind
        ++ lib.optionals (chats s) (setenv "HERDR_MATRIX_GATE_SOCKET" "${p.gate}/socket")
        ++ lib.optionals s.allow.sendFiles (setenv "HERDR_MATRIX_UPLOADS" p.uploads);
      run = lib.optionalString ported ''--ro-bind "$ports" "$ports" ''
        + lib.escapeShellArgs (lib.concatLists (lib.mapAttrsToList setenv s.environment) ++ [ "--" ] ++ lib.optional ported "${listeners id s}")
        + lib.optionalString ported " \"$ports\""
        + " ${lib.escapeShellArgs s.program}";
    in
    pkgs.writeShellScriptBin "herdr-matrix-agent-${id}" (''
      (umask 077 && mkdir -p -- ${lib.escapeShellArgs ([ p.state p.files ] ++ lib.optional s.allow.sendFiles p.uploads)})
    '' + lib.optionalString ported ''
      if [ -z "''${XDG_RUNTIME_DIR-}" ]; then
        echo "herdr-matrix-agent-${id}: XDG_RUNTIME_DIR is not set; allow.ports needs it" >&2
        exit 1
      fi
      ports=$XDG_RUNTIME_DIR/herdr-matrix/ports/${id}
      if [ ! -d "$ports" ]; then
        echo "herdr-matrix-agent-${id}: the sockets of allow.ports are not up: no $ports" >&2
        exit 1
      fi
    '' + ''
      exec ${lib.getExe' pkgs.bubblewrap "bwrap"} \
        ${lib.escapeShellArgs args} \
        --setenv PATH ${lib.escapeShellArg "${lib.getBin cfg.matrix.package}/bin"}:"$PATH" \
        --setenv TERM "''${TERM:-dumb}" \
        --setenv LANG "''${LANG:-C.UTF-8}" \
        ${run} "$@"
    '');
  agentsFile = pkgs.writeText "herdr-matrix-agents.json" (builtins.toJSON (lib.mapAttrsToList
    (id: a: {
      inherit (a) cwd name prefix;
      avatar = if a.avatar == null then null else "${a.avatar}";
    } // lib.optionalAttrs (a.kind != "claude") {
      inherit (a) kind;
    } // lib.optionalAttrs a.sandbox.enable {
      sandbox = { inherit id; inherit (confined id a.kind) state uploads files; };
    })
    cfg.matrix.agents));
  prompts = lib.concatLists (lib.mapAttrsToList
    (id: a: lib.mapAttrsToList
      (name: p: {
        name = "herdr-matrix-prompt-${id}-${name}";
        description = "herdr-matrix prompt ${id}-${name}";
        args = [ (lib.getExe cfg.matrix.package) "prompt" "--cwd" a.cwd p.text ];
        inherit (p) onCalendar;
      })
      a.prompts)
    cfg.matrix.agents);
  models = "${stateDir}/models";
  transcribes = cfg.matrix.transcribe.model != null;
  # One `Environment=` entry, quoted, with `%` kept from specifier expansion
  env = name: value: "\"${name}=${lib.replaceStrings [ "\\" "\"" "%" ] [ "\\\\" "\\\"" "%%" ] value}\"";
  sha256 = hash: builtins.convertHash { inherit hash; toHashFormat = "base16"; hashAlgo = "sha256"; };
  modelPath = file: "${models}/${sha256 file.hash}-${baseNameOf file.url}";
  modelFiles = lib.optional transcribes (cfg.matrix.transcribe.model // { path = modelPath cfg.matrix.transcribe.model; });
  fetchModels = pkgs.callPackage ./models.nix {
    dir = models;
    files = map (file: { inherit (file) path url; sha256 = sha256 file.hash; }) modelFiles;
  };
in
{
  options.programs.herdr = {
    matrix = {
      enable = lib.mkEnableOption ''
        a Matrix chat with the herdr agents: it takes prompts, files, voice messages and answers, and while away is on, posts every agent that stops or waits.
        It needs {option}`programs.herdr.enable`. Its access token comes from `HERDR_MATRIX_TOKEN` in {option}`environmentFile`.
        It keeps its state in `matrix-state.json` in herdr's state directory (a broken file is moved to `matrix-state.json.broken`),
        and files of the chat for 30 days in `$XDG_CACHE_HOME/herdr-matrix`, else `~/.cache/herdr-matrix`.
        `herdr-matrix send <file> [caption]...` in an agent's pane posts a file in the agent's chat; the package is in `home.packages`.
        `!help` in the chat lists the commands; `journalctl --user -u herdr-matrix` shows errors and one line per delivered message, never its text
      '';
      package = lib.mkOption {
        type = lib.types.package;
        default = pkgs.callPackage ./package.nix { };
        defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
        description = "The herdr-matrix package to use.";
      };
      homeserver = lib.mkOption {
        type = lib.types.str;
        example = "https://matrix.example.org";
        description = "Client-server API base URL: `https://`, or `http://` on this host only.";
      };
      owner = lib.mkOption {
        type = lib.types.str;
        example = "@alice:example.org";
        description = "The only Matrix user the bot listens to and invites. To replace a lost chat, invite the bot to a new direct chat with only this user.";
      };
      environmentFile = lib.mkOption {
        type = lib.types.str;
        example = "/run/agenix/herdr-matrix-env";
        description = "File defining `HERDR_MATRIX_TOKEN`, the `as_token` of the appservice registration. While it is missing, the service fails and retries every 10 s; after a change, restart `herdr-matrix`.";
      };
      displayName = lib.mkOption {
        type = lib.types.str;
        default = "herdr";
        description = "The bot's display name.";
      };
      avatar = lib.mkOption {
        type = lib.types.path;
        default = "${cfg.package.src}/assets/logo.png";
        defaultText = lib.literalExpression ''"''${config.programs.herdr.package.src}/assets/logo.png"'';
        description = "The bot's picture, a PNG. The default is the herdr logo from the source of {option}`programs.herdr.package`.";
      };
      spawnAvatar = lib.mkOption {
        type = lib.types.path;
        default = cfg.matrix.avatar;
        defaultText = lib.literalExpression "config.programs.herdr.matrix.avatar";
        description = "The picture of the user that speaks in an agent's own chat (`!dm`), a PNG.";
      };
      spawnPrefix = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Put {option}`displayName` before the agent's name in the name of an agent's own chat user, as `herdr · repo`. Off, the name is the agent's alone. The name follows the agent when it changes.";
      };
      transcribe.model = lib.mkOption {
        type = lib.types.nullOr download;
        default = null;
        example = {
          url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin";
          hash = "sha256-G+OpsgY4Z7k35k4ux0gzZKeZF+FX+pjF2UtcH//qmHs=";
        };
        description = ''
          A whisper.cpp ggml model that transcribes the owner's voice messages, downloaded by the `herdr-matrix-models` service.
          Without one, voice messages are refused; while it downloads, they are refused with a note saying so.
        '';
      };
      transcribe.language = lib.mkOption {
        type = lib.types.str;
        default = "auto";
        example = "en";
        description = "The language of the voice messages, as `whisper-cli -l` takes it; `auto` detects it.";
      };
      agents = lib.mkOption {
        type = lib.types.attrsOf agent;
        default = { };
        example = lib.literalExpression ''
          {
            assistant = {
              cwd = "''${config.home.homeDirectory}/Workspace/assistant";
              name = "Kate";
              prefix = false;
              avatar = ./kate.png;
              prompts.wake = { text = "wake"; onCalendar = "hourly"; };
            };
          }
        '';
        description = "Settings per agent, each for the agent working in its {option}`cwd`. The attribute name is used in unit names and, for a confined agent, in the names of its command, directories and socket.";
      };
      gate = {
        enable = lib.mkEnableOption ''
          the chat gate: this user's agents read, search and send in the owner's bridged chats with `herdr-matrix chats`.
          It needs {option}`programs.herdr.matrix.enable` and the NixOS module `herdr-matrix.nixosModules.default`, which runs it; standalone Home Manager cannot.
          The user gets the socket `/run/herdr-matrix-gate/<user>`, which only that user can open, and the system units `herdr-matrix-gate-<user>.socket` and `herdr-matrix-gate-<user>@.service`.
          The gate runs as the system user `herdr-matrix-gate`, one process per request, and refuses every room that holds the bot or one of its users.
          These options are system configuration: systemd reads {option}`tokenFile` as root and sends the token to {option}`programs.herdr.matrix.homeserver`. Do not let an untrusted user write this Home Manager configuration.
          `journalctl -u 'herdr-matrix-gate-*'` shows one line per request: the command and the outcome, never the token, a message or a room name
        '';
        tokenFile = lib.mkOption {
          type = lib.types.externalPath;
          example = "/run/agenix/matrix-gate-token";
          description = "File with the owner's access token, outside the Nix store. systemd passes it to the gate as a credential, so it may stay `root:root`, mode `0400`, out of this user's reach.";
        };
        bridges = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ "whatsapp" "signal" "telegram" ];
          description = "Localpart prefixes of bridged contacts: a room is a bridged chat when a user `@<prefix>_…` on the owner's server joined it.";
        };
        bot = lib.mkOption {
          type = lib.types.str;
          default = "herdr";
          description = "Localpart of the bot, the `sender_localpart` of its appservice registration: a room with `@<bot>` or `@<bot>_…` on any server, joined, invited or knocking, is refused. A wrong value lifts that guard, and the owner's token reaches the bot's chats.";
        };
      };
    };
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.matrix.gate.enable {
      assertions = [
        {
          assertion = cfg.matrix.enable;
          message = "programs.herdr.matrix.gate.enable needs programs.herdr.matrix.enable.";
        }
        {
          assertion = osConfig != null && (osConfig.herdr-matrix.gate.present or false);
          message = "programs.herdr.matrix.gate.enable needs the NixOS module: import herdr-matrix.nixosModules.default in the NixOS configuration. Standalone Home Manager cannot run the gate.";
        }
      ];
    })
    (lib.mkIf cfg.matrix.enable (lib.mkMerge [{
      assertions = [{
        assertion = cfg.enable;
        message = "programs.herdr.matrix.enable needs programs.herdr.enable.";
      }] ++ lib.concatLists (lib.mapAttrsToList
        (id: a:
          let
            s = a.sandbox;
            on = s.enable;
            agent = "programs.herdr.matrix.agents.${id}";
          in
          [
            {
              assertion = on || !customised a;
              message = "${agent}: the sandbox options need sandbox.enable.";
            }
            {
              assertion = !on || builtins.match "[A-Za-z0-9_-]+" id != null;
              message = "${agent}: the name of a confined agent takes only letters, digits, `_` and `-`.";
            }
            {
              assertion = !on || !(lib.hasPrefix "${lib.removeSuffix "/" a.cwd}/" "${config.home.homeDirectory}/");
              message = "${agent}: the cwd of a confined agent must not hold the home directory.";
            }
            {
              assertion = !on || !chats s || cfg.matrix.gate.enable;
              message = "${agent}: sandbox.allow.chats needs programs.herdr.matrix.gate.enable.";
            }
            {
              assertion = !on || s.allow.chats.rooms == [ ] || chats s;
              message = "${agent}: sandbox.allow.chats.rooms needs sandbox.allow.chats.read or send.";
            }
            {
              assertion = !on || s.allow.ports == { } || !s.allow.network;
              message = "${agent}: sandbox.allow.ports needs sandbox.allow.network = false.";
            }
          ]
          ++ map
            (key: {
              assertion = !on || lib.all (part: !lib.elem part [ "" "." ".." ]) (lib.splitString "/" key);
              message = "${agent}: sandbox.files.\"${key}\" must be a path in the state directory, without an empty, `.` or `..` part.";
            })
            (lib.attrNames s.files)
          ++ lib.concatLists (lib.mapAttrsToList
            (port: address: [
              {
                assertion = !on || builtins.match "[1-9][0-9]{3,4}" port != null && lib.toInt port >= 1024 && lib.toInt port <= 65535;
                message = "${agent}: sandbox.allow.ports.\"${port}\": the port inside must be a number from 1024 to 65535.";
              }
              {
                assertion = !on || builtins.match "([A-Za-z0-9.-]+|\\[[0-9A-Fa-f:.]+]):[0-9]{1,5}" address != null;
                message = "${agent}: sandbox.allow.ports.\"${port}\": the address outside must be `host:port`.";
              }
            ])
            s.allow.ports))
        cfg.matrix.agents);
      home.packages = [ cfg.matrix.package ] ++ lib.mapAttrsToList wrapper sandboxed;
      systemd.user.sockets = lib.listToAttrs (map
        (p: lib.nameValuePair p.name {
          Unit.Description = p.description;
          Socket = {
            ListenStream = "%t/herdr-matrix/ports/${p.id}/${p.port}";
            SocketMode = "0600";
            DirectoryMode = "0700";
          };
          Install.WantedBy = [ "sockets.target" ];
        })
        ports);
      systemd.user.services.herdr-matrix = {
        Unit = {
          Description = "herdr Matrix bot";
          After = [ "herdr.service" ];
          Wants = lib.optional transcribes "herdr-matrix-models.service";
        };
        Service = {
          ExecStart = lib.getExe cfg.matrix.package;
          EnvironmentFile = cfg.matrix.environmentFile;
          Environment = lib.mapAttrsToList env ({
            PATH = lib.makeBinPath ([ cfg.package ]
              ++ lib.optionals transcribes [ pkgs.ffmpeg-headless pkgs.whisper-cpp ]);
            HERDR_MATRIX_STATE = stateDir;
            HERDR_MATRIX_HOMESERVER = cfg.matrix.homeserver;
            HERDR_MATRIX_OWNER = cfg.matrix.owner;
            HERDR_MATRIX_NAME = cfg.matrix.displayName;
            HERDR_MATRIX_AVATAR = "${cfg.matrix.avatar}";
            HERDR_MATRIX_SPAWN_AVATAR = "${cfg.matrix.spawnAvatar}";
            HERDR_MATRIX_SPAWN_PREFIX = lib.boolToString cfg.matrix.spawnPrefix;
          }
          // lib.optionalAttrs transcribes {
            HERDR_MATRIX_WHISPER_MODEL = modelPath cfg.matrix.transcribe.model;
            HERDR_MATRIX_WHISPER_LANGUAGE = cfg.matrix.transcribe.language;
          }
          // lib.optionalAttrs (cfg.matrix.agents != { }) {
            HERDR_MATRIX_AGENTS = "${agentsFile}";
          });
          Restart = "on-failure";
          RestartSec = 10;
        };
        Install.WantedBy = [ "default.target" ];
      };
      # Not ordered before the bot, which starts without the models it lacks
      systemd.user.services.herdr-matrix-models = lib.mkIf transcribes {
        Unit.Description = "herdr Matrix bot whisper model";
        Service = {
          Type = "oneshot";
          ExecStart = lib.getExe fetchModels;
        };
      };
    }
      {
        systemd.user.services = lib.listToAttrs (map
          (p: lib.nameValuePair p.name {
            Unit = {
              Description = p.description;
              After = [ "herdr.service" "herdr-matrix.service" ];
            };
            Service = {
              Type = "oneshot";
              ExecStart = "${pkgs.writeShellScript p.name "exec ${lib.escapeShellArgs p.args}"}";
            };
          })
          prompts
        ++ map
          (p: lib.nameValuePair p.name {
            Unit = {
              Description = p.description;
              Requires = [ "${p.name}.socket" ];
              After = [ "${p.name}.socket" ];
            };
            Service.ExecStart = "${pkgs.systemd}/lib/systemd/systemd-socket-proxyd --exit-idle-time=5min ${p.address}";
          })
          ports);
        systemd.user.timers = lib.listToAttrs (map
          (p: lib.nameValuePair p.name {
            Unit.Description = p.description;
            Timer.OnCalendar = p.onCalendar;
            Install.WantedBy = [ "timers.target" ];
          })
          prompts);
      }]))
  ];
}
