# A Matrix chat with the herdr agents; extends `programs.herdr`, whose
# `package` and state directory it shares
{ lib
, config
, options
, pkgs
, ...
}@args:
let
  cfg = config.programs.herdr;
  # This module defines `programs.herdr.matrix` only; herdr's own module the rest
  herdrDefined = options.programs.herdr ? enable && options.programs.herdr ? package;
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
        description = "The prompt, as the agent gets it. A turn whose last message is exactly `NOOP` posts nothing. For a `pi` agent, a text that starts with `!` runs as a shell command.";
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
      enable = lib.mkEnableOption "the sandbox: the command `herdr-matrix-agent-<name>` runs {option}`programs.herdr.matrix.agents.<name>.sandbox.program` in bubblewrap";
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
        description = "Environment variables inside the sandbox. The agent gets only these, `HOME`, `PATH`, `TERM`, `LANG`, `HERDR_AGENT`, the variable of its state directory, and the variables of what {option}`programs.herdr.matrix.agents.<name>.sandbox.allow` grants. The values are in the Nix store: each user and each confined agent on the host can read them. Do not put a secret here.";
      };
      files = lib.mkOption {
        type = lib.types.attrsOf lib.types.path;
        default = { };
        example = lib.literalExpression ''{ "models.json" = pkgs.writeText "models.json" (builtins.toJSON { /* … */ }); }'';
        description = ''
          Files in the agent's state directory. The key is a path in the state directory; the value is the file.
          The command binds each file read-only at its path: the agent reads it and cannot change it, and each start shows the file of the current generation. A missing file stops the start.
          The mount point stays in the state directory as an empty file after the agent exits.
          The files are in the Nix store: each user and each confined agent on the host can read them. Do not put a secret here.
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
          description = "Share the host's network with the agent, which it needs to reach its model. On, the agent shares the host's network namespace: it reaches each service on the host's loopback addresses and each abstract unix socket. A service that trusts a local caller or this user's uid trusts the agent, as an X server that accepts local users by uid (Xwayland with `SI:localuser`) does: the agent can read the clipboard and send input to X11 programs. On, `/run/systemd/resolve` is shown read-only when it exists, for name lookups. Off, the agent has no network. For an agent that needs only local services, set this to false and name the ports in {option}`programs.herdr.matrix.agents.<name>.sandbox.allow.ports`.";
        };
        ports = lib.mkOption {
          type = lib.types.attrsOf lib.types.str;
          default = { };
          example = { "11434" = "10.23.0.18:11444"; };
          description = ''
            Give an agent without the network fixed TCP addresses. It needs {option}`programs.herdr.matrix.agents.<name>.sandbox.allow.network` off. The key is a port inside the sandbox, from 1024 to 65535; the value is the address outside, `host:port`. Inside, `127.0.0.1:<key>` reaches the value, and no other address exists.
            Each port is a user socket `herdr-matrix-port-<name>-<port>.socket` on `$XDG_RUNTIME_DIR/herdr-matrix/ports/<name>/<port>`, only for this user; its service, `systemd-socket-proxyd`, connects to the value.
            The command binds that directory read-only, starts a `socat` per port on `127.0.0.1` in the sandbox, then the agent. While the directory is missing, the command stops with an error.
          '';
        };
        chats = {
          read = lib.mkOption {
            type = lib.types.bool;
            default = false;
            description = ''
              `herdr-matrix chats rooms`, `read` and `search`, through a socket of the chat gate for this agent only.
              The socket is `/run/herdr-matrix-gate/<user>.<name>/socket`, served by the system units `herdr-matrix-gate-<user>.<name>.socket` and `herdr-matrix-gate-<user>.<name>@.service`. It needs {option}`programs.herdr.matrix.gate.enable`.
            '';
          };
          send = lib.mkOption {
            type = lib.types.bool;
            default = false;
            description = "`herdr-matrix chats rooms` and `send`, through the same socket. Without {option}`programs.herdr.matrix.agents.<name>.sandbox.allow.chats.read`, the agent sends and sees the names of the chats, not their messages.";
          };
          rooms = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [ ];
            example = [ "!abc:example.org" ];
            description = "Empty, {option}`programs.herdr.matrix.agents.<name>.sandbox.allow.chats.read` and {option}`programs.herdr.matrix.agents.<name>.sandbox.allow.chats.send` reach every bridged chat. Else, they reach only the bridged chats with these room ids.";
          };
        };
        sendFiles = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = "`herdr-matrix send`: the agent posts files in its chat with the owner, who already gets every text of the agent. The bot takes the agent from the directory of the queue, `$XDG_CACHE_HOME/herdr-matrix/uploads/<name>`.";
        };
      };
    };
  };
  agent = lib.types.submodule ({ config, ... }: {
    options = {
      cwd = lib.mkOption {
        type = lib.types.str;
        example = "/home/alice/Workspace/assistant";
        description = ''
          Absolute path of the folder the agent works in. It must equal the working directory herdr reports for the agent; a trailing `/` is ignored.
          Each agent has its own folder. `!new` lists the folders of {option}`programs.herdr.matrix.workspace`; `!new <part of a name>` looks one or two levels deep.
          It takes no systemd specifier such as `%h`.
        '';
      };
      kind = lib.mkOption {
        type = lib.types.enum [ "claude" "pi" ];
        default = "claude";
        description = "The agent program: Claude Code or pi. `!new` starts it in {option}`programs.herdr.matrix.agents.<name>.cwd`. It sets the default of {option}`programs.herdr.matrix.agents.<name>.sandbox.program` and the state directory in the sandbox. For an agent without a sandbox, the bot takes the kind from herdr.";
      };
      displayName = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = config.name;
        defaultText = lib.literalExpression "null";
        example = "Kate";
        description = "The display name of the user of this agent's own chat (`!dm`). Unset, it is herdr's name for the agent. The agent's name in `!attach`, `!agents` and `herdr-matrix prompt` stays herdr's.";
      };
      name = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        visible = false;
        description = "The old name of {option}`programs.herdr.matrix.agents.<name>.displayName`.";
      };
      prefix = lib.mkOption {
        type = lib.types.nullOr lib.types.bool;
        default = null;
        description = "Put the bot's {option}`programs.herdr.matrix.displayName` before the name of the user of this agent's own chat (`!dm`). Unset, {option}`programs.herdr.matrix.ownChat.prefix` decides.";
      };
      avatar = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = "The picture of the user of this agent's own chat (`!dm`), a PNG. Unset, it is {option}`programs.herdr.matrix.ownChat.avatar`.";
      };
      chat = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          The bot opens the agent's own chat when it starts the agent (`!new`, {option}`programs.herdr.matrix.agents.<name>.autostart`). Without this, `!dm` opens one.
          A pane reports its own folder, so the bot opens a chat without a command only for an agent it started.
          The chat stays while the agent is in the configuration, also while the agent is not running. It follows the agent to a new pane when the bot starts the agent. After a start by hand, `!dm <agent>` continues the same chat.
          After `!leave`, the bot opens no chat for the agent until `!dm`, also after a restart.
        '';
      };
      autostart = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          The bot starts the agent once when it comes up and the agent is not running. It starts it as `!new` does: a new herdr workspace in {option}`programs.herdr.matrix.agents.<name>.cwd`, with the command `herdr-matrix-agent-<name>` for a confined agent, else the program of {option}`programs.herdr.matrix.agents.<name>.kind`. The main chat stays as it is. With {option}`programs.herdr.matrix.agents.<name>.chat`, the bot opens the agent's own chat.
          The bot does not start the agent again in the same run: not after the agent exits, and not after a start that fails. When herdr fails and lists again, the bot starts the agents that it did not start yet. When {option}`programs.herdr.matrix.agents.<name>.cwd` is missing, the bot logs it and does not start the agent.
        '';
      };
      prompts = lib.mkOption {
        type = lib.types.attrsOf prompt;
        default = { };
        example = { wake = { text = "wake"; onCalendar = "hourly"; }; };
        description = ''
          Prompts the agent gets on a schedule. Each one is a user service and timer `herdr-matrix-prompt-<agent>-<prompt>`, where `<agent>` and `<prompt>` are the attribute names of the agent and the prompt.
          The bot drops a prompt when no agent or more than one works in {option}`programs.herdr.matrix.agents.<name>.cwd`, when the agent works or waits on a choice, or when the bot is down for 10 minutes; `journalctl --user -u herdr-matrix` says why.
        '';
      };
      sandbox = lib.mkOption {
        type = sandbox config.kind;
        default = { };
        description = ''
          The confinement of the agent. Without {option}`programs.herdr.matrix.agents.<name>.sandbox.enable`, the agent has full trust: it runs as this user and can do what this user can.
          With it, the command `herdr-matrix-agent-<name>` runs {option}`programs.herdr.matrix.agents.<name>.sandbox.program` in bubblewrap, and `!new` starts it for {option}`programs.herdr.matrix.agents.<name>.cwd`. Start it in a pane with `exec herdr-matrix-agent-<name>`, or let {option}`programs.herdr.matrix.agents.<name>.autostart` start it. An agent started without the command is not confined.
          Inside, the agent has {option}`programs.herdr.matrix.agents.<name>.cwd` and its own state directory, read and write; the Nix store, `/etc`, `/run/current-system` and {option}`programs.herdr.matrix.agents.<name>.sandbox.allow.readPaths`, read-only; an empty `/tmp` and home; no other process of the host; and only what {option}`programs.herdr.matrix.agents.<name>.sandbox.allow` grants.
          Its state directory is `$XDG_STATE_HOME/herdr-matrix/agents/<name>/<kind>`, in `CLAUDE_CONFIG_DIR` for Claude Code and `PI_CODING_AGENT_DIR` for pi. Files from the owner are in `$XDG_CACHE_HOME/herdr-matrix/agents/<name>`, read-only.
          `HERDR_AGENT` names the kind, so herdr knows the agent.
          `!new` and a start by hand run this user's shell in {option}`programs.herdr.matrix.agents.<name>.cwd` before the command starts. A shell hook that runs code from the folder, such as direnv with an allowed `.envrc`, runs it outside the sandbox.
        '';
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
    let s = a.sandbox; in
    lib.optional (s.program != [ a.kind ]) "sandbox.program"
    ++ lib.optional (s.environment != { }) "sandbox.environment"
    ++ lib.optional (s.files != { }) "sandbox.files"
    ++ lib.optional (s.allow.readPaths != [ ]) "sandbox.allow.readPaths"
    ++ lib.optional (!s.allow.network) "sandbox.allow.network"
    ++ lib.optional (s.allow.ports != { }) "sandbox.allow.ports"
    ++ lib.optional s.allow.chats.read "sandbox.allow.chats.read"
    ++ lib.optional s.allow.chats.send "sandbox.allow.chats.send"
    ++ lib.optional (s.allow.chats.rooms != [ ]) "sandbox.allow.chats.rooms"
    ++ lib.optional (!s.allow.sendFiles) "sandbox.allow.sendFiles";
  # For the messages of the assertions
  show = lib.generators.toPretty { multiline = false; };
  agentOption = id: "programs.herdr.matrix.agents.${lib.strings.escapeNixIdentifier id}";
  sentence = xs: if lib.length xs < 2 then lib.concatStrings xs else "${lib.concatStringsSep ", " (lib.init xs)} and ${lib.last xs}";
  # A name with each character outside `[A-Za-z0-9_-]` replaced by `-`
  rename = name: lib.concatMapStrings (c: if builtins.match "[A-Za-z0-9_-]" c != null then c else "-") (lib.stringToCharacters name);
  # As the bot's `safe_url`: the token goes in clear text over plain HTTP
  safeUrl = url: lib.hasPrefix "https://" url
    || builtins.match "http://((localhost|127(\\.[0-9]+){3})(:[^/]*)?|\\[[0:.]*1(][^/]*)?)(/.*)?" url != null;
  # The characters of a systemd unit name
  unitName = name: builtins.match "[A-Za-z0-9:_.\\\\@-]*" name != null;
  unitChars = "letters, digits, `-`, `_`, `.`, `:`, `@` and `\\`";
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
      inherit (a) cwd prefix;
      name = a.displayName;
      avatar = if a.avatar == null then null else "${a.avatar}";
    } // lib.optionalAttrs a.chat {
      inherit (a) chat;
    } // lib.optionalAttrs a.autostart {
      inherit (a) autostart;
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
  imports = [
    (lib.mkRenamedOptionModule [ "programs" "herdr" "matrix" "spawnAvatar" ] [ "programs" "herdr" "matrix" "ownChat" "avatar" ])
    (lib.mkRenamedOptionModule [ "programs" "herdr" "matrix" "spawnPrefix" ] [ "programs" "herdr" "matrix" "ownChat" "prefix" ])
  ];

  options.programs.herdr = {
    matrix = {
      enable = lib.mkEnableOption "a Matrix chat with the herdr agents, the user service `herdr-matrix`";
      package = lib.mkOption {
        type = lib.types.package;
        default = pkgs.callPackage ./package.nix { };
        defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
        description = "The herdr-matrix package. It is in `home.packages`, so `herdr-matrix` works in an agent's pane.";
      };
      homeserver = lib.mkOption {
        type = lib.types.str;
        example = "https://matrix.example.org";
        description = "Client-server API base URL: `https://`, or `http://` to `localhost`, `127.0.0.1` or `[::1]`. The bot sends its token with each request.";
      };
      owner = lib.mkOption {
        type = lib.types.str;
        example = "@alice:example.org";
        description = ''
          The only Matrix user the bot listens to and invites. The bot takes the owner's prompts, files, voice messages and answers. While away is on (`!away`), it posts each agent that stops or waits.
          To replace a lost chat, invite the bot to a new direct chat with only this user.
        '';
      };
      tokenFile = lib.mkOption {
        type = lib.types.nullOr lib.types.externalPath;
        default = null;
        example = "/run/agenix/herdr-matrix-token";
        description = ''
          File with the `as_token` of the appservice registration and nothing else, outside the Nix store. Whitespace around it is ignored. Set this or {option}`programs.herdr.matrix.environmentFile`.
          systemd passes it to the bot as the credential `token`. The user manager reads it, so this user must be able to read it.
          While it is missing, the service fails and retries every 10 s. After a change, restart `herdr-matrix`.
        '';
      };
      environmentFile = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/agenix/herdr-matrix-env";
        description = "File with the line `HERDR_MATRIX_TOKEN=<as_token>`, where `<as_token>` is the `as_token` of the appservice registration. Set this or {option}`programs.herdr.matrix.tokenFile`. While it is missing, the service fails and retries every 10 s. After a change, restart `herdr-matrix`.";
      };
      workspace = lib.mkOption {
        type = lib.types.str;
        default = "${config.home.homeDirectory}/Workspace";
        defaultText = lib.literalExpression ''"''${config.home.homeDirectory}/Workspace"'';
        example = "/srv/code";
        description = "The folder of the projects. `!new` lists its folders; `!new <part of a name>` looks one or two levels deep. It starts an agent in the folder the owner picks.";
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
      ownChat.avatar = lib.mkOption {
        type = lib.types.path;
        default = cfg.matrix.avatar;
        defaultText = lib.literalExpression "config.programs.herdr.matrix.avatar";
        description = "The picture of the user of an agent's own chat (`!dm`), a PNG. An agent's {option}`programs.herdr.matrix.agents.<name>.avatar` replaces it.";
      };
      ownChat.prefix = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Put {option}`programs.herdr.matrix.displayName` before the agent's name in the name of the user of an agent's own chat (`!dm`), as `herdr · repo`. Off, the name is the agent's alone. The name follows the agent when it changes. An agent's {option}`programs.herdr.matrix.agents.<name>.prefix` replaces it.";
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
              displayName = "Kate";
              prefix = false;
              chat = true;
              avatar = ./kate.png;
              prompts.wake = { text = "wake"; onCalendar = "hourly"; };
            };
          }
        '';
        description = "Settings per agent, each for the agent working in its {option}`programs.herdr.matrix.agents.<name>.cwd`. The attribute name is used in unit names and, for a confined agent, in the names of its command, directories and socket. `<name>` in the descriptions of these options is this attribute name.";
      };
      gate = {
        enable = lib.mkEnableOption "the chat gate: this user's agents read, search and send in the owner's bridged chats with `herdr-matrix chats`";
        tokenFile = lib.mkOption {
          type = lib.types.externalPath;
          example = "/run/agenix/matrix-gate-token";
          description = ''
            File with the owner's access token, outside the Nix store. systemd passes it to the gate as a credential, so it may stay `root:root`, mode `0400`, out of this user's reach.
            The gate needs {option}`programs.herdr.matrix.enable` and the NixOS module `herdr-matrix.nixosModules.default`, which runs it; standalone Home Manager cannot.
            The user gets the socket `/run/herdr-matrix-gate/<user>`, which only that user can open, and the system units `herdr-matrix-gate-<user>.socket` and `herdr-matrix-gate-<user>@.service`.
            The gate runs as the system user `herdr-matrix-gate`, one process per request, and refuses every room that holds the bot or one of its users.
            The gate options are system configuration: systemd reads this file as root and sends the token to {option}`programs.herdr.matrix.homeserver`. Do not let an untrusted user write this Home Manager configuration.
            `journalctl -u 'herdr-matrix-gate-*'` shows one line per request: the command and the outcome, never the token, a message or a room name.
          '';
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
          message = "programs.herdr.matrix.enable = false: programs.herdr.matrix.gate.enable needs the chat. Set programs.herdr.matrix.enable = true.";
        }
        {
          assertion = osConfig != null && (osConfig.herdr-matrix.gate.present or false);
          message = "programs.herdr.matrix.gate.enable = true: the gate runs as a system service, which only the NixOS module makes; standalone Home Manager cannot run it. Import herdr-matrix.nixosModules.default in the NixOS configuration, and run Home Manager as its NixOS module.";
        }
      ];
    })
    (lib.mkIf cfg.matrix.enable (lib.mkMerge [{
      assertions = [
        {
          assertion = herdrDefined;
          message = "programs.herdr.matrix needs a module that defines programs.herdr.enable and programs.herdr.package.";
        }
        {
          assertion = !herdrDefined || cfg.enable;
          message = "programs.herdr.enable = false: programs.herdr.matrix needs herdr. Set programs.herdr.enable = true.";
        }
        {
          assertion = safeUrl cfg.matrix.homeserver;
          message = "programs.herdr.matrix.homeserver = ${show cfg.matrix.homeserver}: the bot sends its token with each request, so the URL must be https://, or http:// to localhost, 127.0.0.1 or [::1]. Use the https:// URL of the homeserver.";
        }
        {
          assertion = builtins.match "@[^:]+:.+" cfg.matrix.owner != null;
          message = "programs.herdr.matrix.owner = ${show cfg.matrix.owner}: a Matrix user id has the form @local:server. Use the full id, such as \"@alice:example.org\".";
        }
        {
          assertion = cfg.matrix.tokenFile != null || cfg.matrix.environmentFile != null;
          message = "programs.herdr.matrix.tokenFile = null: the bot needs its as_token from tokenFile or environmentFile. Set programs.herdr.matrix.tokenFile.";
        }
        {
          assertion = cfg.matrix.tokenFile == null || cfg.matrix.environmentFile == null;
          message = "programs.herdr.matrix.tokenFile = ${show cfg.matrix.tokenFile}: programs.herdr.matrix.environmentFile is set too, and the bot takes its as_token from one file. Remove one of them.";
        }
      ]
      ++ lib.concatLists (lib.mapAttrsToList
        (id: a:
          let
            s = a.sandbox;
            on = s.enable;
            agent = agentOption id;
            cwd = "${agent}.cwd = ${show a.cwd}";
            cwdParts = lib.filter (part: part != "") (lib.splitString "/" a.cwd);
            set = customised a;
          in
          [
            {
              assertion = lib.hasPrefix "/" a.cwd;
              message = "${cwd}: the folder must be an absolute path, as herdr reports it. Use a path such as \"\${config.home.homeDirectory}/Workspace/${id}\".";
            }
            {
              assertion = a.prompts == { } || unitName id;
              message = "${agent}: the attribute name of an agent with prompts goes into systemd unit names, so it takes only ${unitChars}. Rename it, such as ${show (rename id)}.";
            }
            {
              assertion = on || set == [ ];
              message = "${agent}: ${sentence set} ${if lib.length set > 1 then "are" else "is"} set, but sandbox.enable is false. Set sandbox.enable = true, or remove ${if lib.length set > 1 then "them" else "it"}.";
            }
            {
              assertion = !on || builtins.match "[A-Za-z0-9_-]+" id != null;
              message = "${agent}: the attribute name of a confined agent names its command, directories and socket, so it takes only letters, digits, `_` and `-`. Rename it, such as ${show (rename id)}.";
            }
            {
              assertion = !on || !lib.any (part: lib.elem part [ "." ".." ]) cwdParts;
              message = "${cwd}: the folder of a confined agent must not hold a `.` or `..` part. Write the path without them.";
            }
            {
              assertion = !on || !(lib.hasPrefix "/${lib.concatMapStrings (part: "${part}/") cwdParts}" "${config.home.homeDirectory}/");
              message = "${cwd}: the folder of a confined agent must not hold the home directory. Use a folder inside it, such as ${show "${config.home.homeDirectory}/Workspace/${id}"}.";
            }
            {
              assertion = !on || !chats s || cfg.matrix.gate.enable;
              message = "${agent}.sandbox.allow.chats.${if s.allow.chats.read then "read" else "send"} = true: the agent's chats go through the chat gate. Set programs.herdr.matrix.gate.enable = true.";
            }
            {
              assertion = !on || s.allow.chats.rooms == [ ] || chats s;
              message = "${agent}.sandbox.allow.chats.rooms = ${show s.allow.chats.rooms}: the rooms limit sandbox.allow.chats.read and send, and neither is on. Set sandbox.allow.chats.read or sandbox.allow.chats.send to true, or remove the rooms.";
            }
            {
              assertion = !on || s.allow.ports == { } || !s.allow.network;
              message = "${agent}.sandbox.allow.network = true: sandbox.allow.ports is the way to reach a service without the network, so it needs the network off. Set sandbox.allow.network = false, or remove sandbox.allow.ports.";
            }
          ]
          ++ map
            (name: {
              assertion = unitName name;
              message = "${agent}.prompts.${lib.strings.escapeNixIdentifier name}: the name of a prompt goes into systemd unit names, so it takes only ${unitChars}. Rename it, such as ${show (rename name)}.";
            })
            (lib.attrNames a.prompts)
          ++ map
            (key: {
              assertion = !on || lib.all (part: !lib.elem part [ "" "." ".." ]) (lib.splitString "/" key);
              message = "${agent}.sandbox.files.${lib.strings.escapeNixIdentifier key}: the key must be a path in the state directory, without an empty, `.` or `..` part. Use a relative path, such as \"models.json\".";
            })
            (lib.attrNames s.files)
          ++ lib.concatLists (lib.mapAttrsToList
            (port: address: [
              {
                assertion = !on || builtins.match "[1-9][0-9]{3,4}" port != null && lib.toInt port >= 1024 && lib.toInt port <= 65535;
                message = "${agent}.sandbox.allow.ports.${lib.strings.escapeNixIdentifier port}: the key is the port inside the sandbox, a number from 1024 to 65535. Use a port in that range.";
              }
              {
                assertion = !on || builtins.match "([A-Za-z0-9.-]+|\\[[0-9A-Fa-f:.]+]):[0-9]{1,5}" address != null;
                message = "${agent}.sandbox.allow.ports.${lib.strings.escapeNixIdentifier port} = ${show address}: the address outside must be `host:port`. Write it as host:port, such as \"10.23.0.18:11444\" or \"[::1]:80\".";
              }
            ])
            s.allow.ports))
        cfg.matrix.agents);
      warnings = lib.concatLists
        (lib.mapAttrsToList
          (_: ids: lib.optional (lib.length ids > 1)
            "${agentOption (lib.head ids)}.cwd = ${show cfg.matrix.agents.${lib.head ids}.cwd}: the agents ${sentence ids} work in the same folder, and the bot applies the settings of one agent per folder. Give each agent its own cwd.")
          (lib.groupBy (id: lib.removeSuffix "/" cfg.matrix.agents.${id}.cwd) (lib.attrNames cfg.matrix.agents)))
      ++ lib.concatLists (lib.mapAttrsToList
        (id: a: lib.optional (a.name != null)
          "${agentOption id}.name = ${show a.name}: the option is named displayName. Set ${agentOption id}.displayName."
        ++ lib.optional (a.sandbox.enable && !a.sandbox.allow.network && a.sandbox.allow.ports == { })
          "${agentOption id}.sandbox.allow.network = false: the agent has no network and no sandbox.allow.ports, so it cannot reach a model. Name the model's address in sandbox.allow.ports, or set sandbox.allow.network = true.")
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
          Environment = lib.mapAttrsToList env ({
            PATH = lib.makeBinPath ([ cfg.package ]
              ++ lib.optionals transcribes [ pkgs.ffmpeg-headless pkgs.whisper-cpp ]);
            HERDR_MATRIX_STATE = stateDir;
            HERDR_MATRIX_HOMESERVER = cfg.matrix.homeserver;
            HERDR_MATRIX_OWNER = cfg.matrix.owner;
            HERDR_MATRIX_NAME = cfg.matrix.displayName;
            HERDR_MATRIX_AVATAR = "${cfg.matrix.avatar}";
            HERDR_MATRIX_SPAWN_AVATAR = "${cfg.matrix.ownChat.avatar}";
            HERDR_MATRIX_SPAWN_PREFIX = lib.boolToString cfg.matrix.ownChat.prefix;
            HERDR_MATRIX_WORKSPACE = cfg.matrix.workspace;
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
        }
        // lib.optionalAttrs (cfg.matrix.environmentFile != null) {
          EnvironmentFile = cfg.matrix.environmentFile;
        }
        // lib.optionalAttrs (cfg.matrix.tokenFile != null) {
          LoadCredential = "token:${lib.replaceStrings [ "%" ] [ "%%" ] cfg.matrix.tokenFile}";
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
