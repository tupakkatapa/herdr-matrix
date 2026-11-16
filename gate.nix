# The chat gate: the processes of each Home Manager user with
# `programs.herdr.matrix.gate.enable` read, search and send in the owner's
# bridged chats through `herdr-matrix chats`, and only this service reads
# the owner's token
{ lib
, config
, ...
}:
let
  # `%` kept from specifier expansion
  escape = lib.replaceStrings [ "%" ] [ "%%" ];
  # Empty without Home Manager, and for users without the home module
  users = lib.filterAttrs (_: home: home.programs.herdr.matrix.gate.enable or false)
    (lib.attrByPath [ "home-manager" "users" ] { } config);
  perUser = f: lib.mapAttrs' (user: home: f user home.programs.herdr.matrix) users;
in
{
  options.herdr-matrix.gate.present = lib.mkOption {
    type = lib.types.bool;
    default = true;
    readOnly = true;
    internal = true;
    description = "This module is imported; `programs.herdr.matrix.gate.enable` asserts it.";
  };

  config = lib.mkIf (users != { }) {
    users.users.herdr-matrix-gate = {
      isSystemUser = true;
      group = "herdr-matrix-gate";
    };
    users.groups.herdr-matrix-gate = { };

    environment.systemPackages = lib.mapAttrsToList (_: home: home.programs.herdr.matrix.package) users;

    systemd.sockets = perUser (user: _: lib.nameValuePair "herdr-matrix-gate-${user}" {
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
    });

    systemd.services = perUser (user: matrix: lib.nameValuePair "herdr-matrix-gate-${user}@" {
      description = "herdr-matrix chat gate request of ${user}";
      environment = {
        HERDR_MATRIX_GATE_HOMESERVER = escape matrix.homeserver;
        HERDR_MATRIX_GATE_BRIDGES = lib.concatStringsSep " " matrix.gate.bridges;
        HERDR_MATRIX_GATE_BOT = matrix.gate.bot;
      };
      serviceConfig = {
        ExecStart = "${lib.getExe matrix.package} gate";
        StandardInput = "socket";
        StandardOutput = "socket";
        StandardError = "journal";
        User = "herdr-matrix-gate";
        Group = "herdr-matrix-gate";
        LoadCredential = "token:${escape matrix.gate.tokenFile}";
        RuntimeMaxSec = "2min";
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
      };
    });
  };
}
