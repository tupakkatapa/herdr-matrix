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
  # The socket of each user, and one of each confined agent with chats
  gates = lib.concatLists (lib.mapAttrsToList
    (user: home:
      let inherit (home.programs.herdr) matrix; in
      [{
        name = "herdr-matrix-gate-${user}";
        of = user;
        path = "/run/herdr-matrix-gate/${user}";
        inherit user matrix;
        environment = { };
      }]
      ++ lib.mapAttrsToList
        (id: a: {
          name = "herdr-matrix-gate-${user}.${id}";
          of = "${user}, agent ${id}";
          # In a directory of its own, which the sandbox binds
          path = "/run/herdr-matrix-gate/${user}.${id}/socket";
          inherit user matrix;
          environment = {
            HERDR_MATRIX_GATE_COMMANDS = lib.concatStringsSep " "
              ([ "rooms" ] ++ lib.optionals a.sandbox.allow.chats.read [ "read" "search" ]
              ++ lib.optional a.sandbox.allow.chats.send "send");
          } // lib.optionalAttrs (a.sandbox.allow.chats.rooms != [ ]) {
            HERDR_MATRIX_GATE_ROOMS = escape (lib.concatStringsSep " " a.sandbox.allow.chats.rooms);
          };
        })
        (lib.filterAttrs (_: a: a.sandbox.enable && (a.sandbox.allow.chats.read || a.sandbox.allow.chats.send)) matrix.agents))
    users);
  perGate = f: lib.listToAttrs (map f gates);
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

    systemd.sockets = perGate (gate: lib.nameValuePair gate.name {
      description = "herdr-matrix chat gate of ${gate.of}";
      wantedBy = [ "sockets.target" ];
      socketConfig = {
        ListenStream = gate.path;
        SocketUser = gate.user;
        SocketMode = "0600";
        Accept = true;
        MaxConnections = 4;
        RemoveOnStop = true;
      };
    });

    systemd.services = perGate (gate: lib.nameValuePair "${gate.name}@" {
      description = "herdr-matrix chat gate request of ${gate.of}";
      environment = {
        HERDR_MATRIX_GATE_HOMESERVER = escape gate.matrix.homeserver;
        HERDR_MATRIX_GATE_BRIDGES = lib.concatStringsSep " " gate.matrix.gate.bridges;
        HERDR_MATRIX_GATE_BOT = gate.matrix.gate.bot;
      } // gate.environment;
      serviceConfig = {
        ExecStart = "${lib.getExe gate.matrix.package} gate";
        StandardInput = "socket";
        StandardOutput = "socket";
        StandardError = "journal";
        User = "herdr-matrix-gate";
        Group = "herdr-matrix-gate";
        LoadCredential = "token:${escape gate.matrix.gate.tokenFile}";
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
