# The chat gate: the processes of `users` read, search and send in the
# owner's bridged chats through `herdr-matrix chats`, and only this service
# reads the owner's token
{ lib
, config
, pkgs
, ...
}:
let
  cfg = config.services.herdr-matrix-gate;
  # `%` kept from specifier expansion
  escape = lib.replaceStrings [ "%" ] [ "%%" ];
in
{
  options.services.herdr-matrix-gate = {
    enable = lib.mkEnableOption ''
      the chat gate: the agents of {option}`users` read, search and send in the owner's bridged chats with `herdr-matrix chats`.
      Each user gets the socket `/run/herdr-matrix-gate/<user>`, which only that user can open, and the units `herdr-matrix-gate-<user>.socket` and `herdr-matrix-gate-<user>@.service`.
      The gate runs as the system user `herdr-matrix-gate`, one process per request, and refuses every room that holds the bot or one of its users.
      `journalctl -u 'herdr-matrix-gate-*'` shows one line per request: the command and the outcome, never the token, a message or a room name
    '';
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
      description = "The herdr-matrix package to use. It goes on the system path.";
    };
    homeserver = lib.mkOption {
      type = lib.types.str;
      example = "https://matrix.example.org";
      description = "Client-server API base URL: `https://`, or `http://` on this host only.";
    };
    tokenFile = lib.mkOption {
      type = lib.types.externalPath;
      example = "/run/agenix/matrix-gate-token";
      description = "File with the owner's access token, outside the Nix store. systemd passes it to the gate as a credential, so it may stay `root:root`, mode `0400`.";
    };
    users = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      example = [ "alice" ];
      description = "Unix users whose processes may use the gate.";
    };
    bridges = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ "whatsapp" "signal" "telegram" ];
      description = "Localpart prefixes of bridged contacts: a room is a bridged chat when a user `@<prefix>_…` on the owner's server joined it.";
    };
    bot = lib.mkOption {
      type = lib.types.str;
      default = "herdr";
      description = "Localpart of the bot: a room with `@<bot>` or `@<bot>_…` on any server, joined, invited or knocking, is refused.";
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.herdr-matrix-gate = {
      isSystemUser = true;
      group = "herdr-matrix-gate";
    };
    users.groups.herdr-matrix-gate = { };

    environment.systemPackages = [ cfg.package ];

    systemd.sockets = lib.listToAttrs (map
      (user: lib.nameValuePair "herdr-matrix-gate-${user}" {
        description = "herdr-matrix chat gate of ${user}";
        wantedBy = [ "sockets.target" ];
        socketConfig = {
          ListenStream = "/run/herdr-matrix-gate/${user}";
          SocketUser = user;
          SocketMode = "0600";
          Accept = true;
          MaxConnections = 4;
          RemoveOnStop = true;
        };
      })
      cfg.users);

    systemd.services = lib.listToAttrs (map
      (user: lib.nameValuePair "herdr-matrix-gate-${user}@" {
        description = "herdr-matrix chat gate request of ${user}";
        environment = {
          HERDR_MATRIX_GATE_HOMESERVER = escape cfg.homeserver;
          HERDR_MATRIX_GATE_BRIDGES = lib.concatStringsSep " " cfg.bridges;
          HERDR_MATRIX_GATE_BOT = cfg.bot;
        };
        serviceConfig = {
          ExecStart = "${lib.getExe cfg.package} gate";
          StandardInput = "socket";
          StandardOutput = "socket";
          StandardError = "journal";
          User = "herdr-matrix-gate";
          Group = "herdr-matrix-gate";
          LoadCredential = "token:${escape cfg.tokenFile}";
          RuntimeMaxSec = "2min";
          NoNewPrivileges = true;
          ProtectSystem = "strict";
          ProtectHome = true;
          PrivateTmp = true;
          PrivateDevices = true;
          RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
        };
      })
      cfg.users);
  };
}
