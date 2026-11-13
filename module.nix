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
    };
  };

  config = lib.mkIf cfg.matrix.enable {
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
        }
        // lib.optionalAttrs transcribes {
          HERDR_MATRIX_WHISPER_MODEL = modelPath cfg.matrix.transcribe.model;
          HERDR_MATRIX_WHISPER_LANGUAGE = cfg.matrix.transcribe.language;
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
  };
}
