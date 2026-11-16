# A Matrix chat with the herdr agents; extends `programs.herdr`, whose
# `package` and state directory it shares
{ lib
, config
, pkgs
, ...
}:
let
  cfg = config.programs.herdr;
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
  agent = lib.types.submodule {
    options = {
      cwd = lib.mkOption {
        type = lib.types.str;
        example = "/home/alice/Workspace/assistant";
        description = "The working directory of the agent; a trailing `/` aside, it must equal the agent's. It takes no systemd specifier such as `%h`.";
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
    };
  };
  agentsFile = pkgs.writeText "herdr-matrix-agents.json" (builtins.toJSON (lib.mapAttrsToList
    (_: a: {
      inherit (a) cwd name prefix;
      avatar = if a.avatar == null then null else "${a.avatar}";
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
        and files from the chat for 24 h in `$XDG_RUNTIME_DIR/herdr-matrix`.
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
        description = "Settings per agent, each for the agent working in its {option}`cwd`. The attribute name is used in unit names only.";
      };
    };
  };

  config = lib.mkIf cfg.matrix.enable (lib.mkMerge [{
    assertions = [{
      assertion = cfg.enable;
      message = "programs.herdr.matrix.enable needs programs.herdr.enable.";
    }];
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
        prompts);
      systemd.user.timers = lib.listToAttrs (map
        (p: lib.nameValuePair p.name {
          Unit.Description = p.description;
          Timer.OnCalendar = p.onCalendar;
          Install.WantedBy = [ "timers.target" ];
        })
        prompts);
    }]);
}
