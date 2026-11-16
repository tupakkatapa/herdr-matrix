# The chat gate in a VM, against a homeserver of a few lines that answers
# only with the token of `tokenFile`
{ pkgs, module, homeManager }:
let
  homeserver = pkgs.writeText "homeserver.py" ''
    import json
    from http.server import BaseHTTPRequestHandler, HTTPServer
    from urllib.parse import parse_qs, unquote, urlsplit

    ROOMS = {
        "!bridged:x": ["@owner:x", "@signal_1:x"],
        "!herdr:x": ["@owner:x", "@signal_1:x", "@herdr_x:x"],
    }
    EVENT = {
        "type": "m.room.message",
        "sender": "@signal_1:x",
        "origin_server_ts": 1794832200000,
        "content": {"msgtype": "m.text", "body": "hello from signal"},
    }


    def member(user):
        return {
            "type": "m.room.member",
            "state_key": user,
            "content": {"membership": "join"},
        }


    class Homeserver(BaseHTTPRequestHandler):
        def route(self):
            with open("/run/gate-token") as f:
                token = f.read().strip()
            if self.headers.get("Authorization") != f"Bearer {token}":
                return 401, {"errcode": "M_UNKNOWN_TOKEN"}
            url = urlsplit(self.path)
            parts = [unquote(p) for p in url.path.split("/")[4:]]
            match parts:
                case ["account", "whoami"]:
                    return 200, {"user_id": "@owner:x"}
                case ["joined_rooms"]:
                    return 200, {"joined_rooms": list(ROOMS)}
                case ["rooms", room, "members"] if room in ROOMS:
                    return 200, {"chunk": [member(u) for u in ROOMS[room]]}
                case ["rooms", room, "state", "m.room.name", ""]:
                    return 200, {"name": room[1:].split(":")[0]}
                case ["rooms", room, "messages"]:
                    first = "from" not in parse_qs(url.query)
                    return 200, {"chunk": [EVENT] if first else [], "end": "e"}
                case ["rooms", room, "send", "m.room.message", _]:
                    return 200, {"event_id": "$sent"}
            return 404, {"errcode": "M_NOT_FOUND"}

        def do_GET(self):
            status, body = self.route()
            data = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        do_PUT = do_GET


    HTTPServer(("127.0.0.1", 8008), Homeserver).serve_forever()
  '';
in
pkgs.testers.runNixOSTest {
  name = "herdr-matrix-gate";
  nodes.machine = {
    imports = [ module homeManager ];
    users.users.alice.isNormalUser = true;
    users.users.bob.isNormalUser = true;
    home-manager.users.alice.programs.herdr = {
      enable = true;
      matrix = {
        enable = true;
        homeserver = "http://127.0.0.1:8008";
        owner = "@owner:x";
        environmentFile = "/run/env";
        avatar = "/a.png";
        gate = {
          enable = true;
          tokenFile = "/run/gate-token";
        };
      };
    };
    home-manager.users.bob = { };
    systemd.services.homeserver = {
      wantedBy = [ "multi-user.target" ];
      serviceConfig.ExecStart = "${pkgs.python3.interpreter} ${homeserver}";
    };
  };
  testScript = ''
    machine.wait_for_unit("multi-user.target")
    machine.wait_for_open_port(8008)
    machine.succeed("printf %s s3cret-gate-token > /run/gate-token && chmod 0400 /run/gate-token")

    rooms = machine.succeed("sudo -u alice herdr-matrix chats rooms")
    assert "!bridged:x" in rooms and "!herdr:x" not in rooms, rooms
    read = machine.succeed("sudo -u alice herdr-matrix chats read '!bridged:x'")
    assert '"body": "hello from signal"' in read, read
    machine.succeed("sudo -u alice herdr-matrix chats send '!bridged:x' hi")
    refused = machine.fail("sudo -u alice herdr-matrix chats send '!herdr:x' hi 2>&1")
    assert refused.strip() == "not a bridged chat", refused

    machine.fail("systemctl cat herdr-matrix-gate-bob.socket")
    bob = machine.fail("sudo -u bob herdr-matrix chats rooms 2>&1")
    assert bob.strip() == "the gate is not set up for this user", bob
    denied = machine.fail(
        "sudo -u bob env HERDR_MATRIX_GATE_SOCKET=/run/herdr-matrix-gate/alice herdr-matrix chats rooms 2>&1"
    )
    assert "Permission denied" in denied, denied

    machine.fail("sudo -u alice cat /run/gate-token")
    machine.fail("sudo -u bob cat /run/gate-token")
    uid = machine.succeed("id -u herdr-matrix-gate").strip()
    machine.wait_until_succeeds(
        f"journalctl -o verbose -u 'herdr-matrix-gate-alice@*' | grep -E '^ +_UID={uid}$'"
    )
    machine.succeed("! journalctl --no-pager | grep -F s3cret-gate-token")
  '';
}
