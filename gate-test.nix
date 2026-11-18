# The chat gate in a VM, against a homeserver of a few lines that answers
# only with the token of `tokenFile`, and the confined agents of its user
{ pkgs, module, homeManager }:
let
  inherit (pkgs) lib;
  # The program of each confined agent: what it finds in its sandbox, by
  # the agent's name
  probe = pkgs.writeShellScript "probe" ''
    set -euo pipefail
    export PATH=${lib.makeBinPath [ pkgs.coreutils pkgs.gnugrep pkgs.findutils pkgs.curl ]}:$PATH
    home=/home/alice
    me=$1
    fail() { echo "$me: $*"; exit 1; }
    absent() { for p in "$@"; do if [ -e "$p" ] || [ -L "$p" ]; then fail "present: $p"; fi; done; }
    never() { if "$@" 2>/dev/null; then fail "did: $*"; fi; }
    refused() {
      local want=$1 out
      shift
      if out=$("$@" 2>&1); then fail "not refused: $*"; fi
      [ "$out" = "$want" ] || fail "$*: $out"
    }
    online() { (exec 3<>/dev/tcp/127.0.0.1/8008) 2>/dev/null; }

    absent $home/secret $home/.config /run/agenix /nix/var
    [ "$(cat /proc/$PPID/comm)" = bwrap ] || fail "started by $(cat /proc/$PPID/comm)"
    for agent in bare nora mia; do
      [ "$agent" = "$me" ] || absent "$home/$agent"
    done
    [ "$PWD" = "$home/$me" ] || fail "in $PWD"
    touch "$home/$me/made"
    case $me in
      bare) kind=pi state=''${PI_CODING_AGENT_DIR-} other=''${CLAUDE_CONFIG_DIR-} ;;
      *) kind=claude state=''${CLAUDE_CONFIG_DIR-} other=''${PI_CODING_AGENT_DIR-} ;;
    esac
    [ "$state" = "$home/.local/state/herdr-matrix/agents/$me/$kind" ] || fail "state $state"
    [ -z "$other" ] || fail "state of another kind: $other"
    [ "''${HERDR_AGENT-}" = "$kind" ] || fail "HERDR_AGENT ''${HERDR_AGENT-}"
    touch "$state/made"
    [ "$(ls $home/.cache/herdr-matrix/agents)" = "$me" ] || fail "files of others"
    never touch "$home/.cache/herdr-matrix/agents/$me/made"
    absent $home/.cache/herdr-matrix/prompts
    [ -z "''${HERDR_PANE_ID-}''${XDG_RUNTIME_DIR-}" ] || fail "environment of the pane"
    never grep -qsx sleep /proc/[0-9]*/comm
    echo data > /tmp/shot.png

    case $me in
      bare)
        absent /run/herdr-matrix-gate $home/.cache/herdr-matrix/uploads
        refused "herdr-matrix: the gate is not set up for this user" herdr-matrix chats rooms
        refused "herdr-matrix: send works only in a herdr pane: HERDR_PANE_ID is not set" herdr-matrix send /tmp/shot.png
        never online
        ports=/run/user/1000/herdr-matrix/ports
        [ "$(find /run/user -maxdepth 4)" = "$(printf '%s\n' /run/user{,/1000{,/herdr-matrix{,/ports{,/bare}}}})" ] || fail "runtime: $(find /run/user)"
        [ "$(curl -sS --retry 5 --retry-connrefused --retry-delay 1 http://127.0.0.1:11434/answer)" = "from the model" ] || fail "no answer"
        never curl -sS -m 2 http://127.0.0.1:8080/answer
        never touch $ports/bare/made
        [ "$(cat "$state/models.json")" = '{"providers":{}}' ] || fail "models.json"
        never tee -a "$state/models.json" </dev/null
        never rm -f "$state/models.json"
        read -r _ _ _ _ group session _ </proc/$$/stat
        listeners=0
        for p in /proc/[0-9]*; do
          case $(cat "$p/comm") in socat*) ;; *) continue ;; esac
          read -r pid _ _ _ pgrp sid _ <"$p/stat"
          [ "$pgrp" != "$group" ] && [ "$sid" != "$session" ] || fail "socat $pid in the agent's group"
          [ "$(readlink "$p/fd/0")" = /dev/null ] || fail "socat $pid reads $(readlink "$p/fd/0")"
          never grep -qz '^HERDR_AGENT=' "$p/environ"
          [ "$sid" != "$pid" ] || listeners=$((listeners + 1))
        done
        [ "$listeners" = 1 ] || fail "$listeners listeners"
        ;;
      nora)
        absent /run/user
        [ "$(ls /run/herdr-matrix-gate)" = alice.nora ] || fail "gate sockets"
        absent $home/.cache/herdr-matrix/uploads
        rooms=$(herdr-matrix chats rooms)
        grep -qF '!bridged:x' <<<"$rooms" || fail "$rooms"
        never grep -qF '!other:x' <<<"$rooms"
        herdr-matrix chats read '!bridged:x' | grep -qF 'hello from signal'
        herdr-matrix chats search '!bridged:x' hello | grep -qF 'hello from signal'
        refused "herdr-matrix: not a bridged chat" herdr-matrix chats read '!other:x'
        refused "herdr-matrix: not allowed for this agent" herdr-matrix chats send '!bridged:x' hi
        refused "herdr-matrix: send works only in a herdr pane: HERDR_PANE_ID is not set" herdr-matrix send /tmp/shot.png
        never online
        ;;
      mia)
        absent /run/user
        [ "$(ls /run/herdr-matrix-gate)" = alice.mia ] || fail "gate sockets"
        [ "$(ls $home/.cache/herdr-matrix/uploads)" = mia ] || fail "queues of others"
        rooms=$(herdr-matrix chats rooms)
        grep -qF '!other:x' <<<"$rooms" || fail "$rooms"
        herdr-matrix chats send '!other:x' hi
        refused "herdr-matrix: not a bridged chat" herdr-matrix chats send '!herdr:x' hi
        herdr-matrix send /tmp/shot.png Look
        online
        ;;
    esac
    echo "probed $me"
  '';
  homeserver = pkgs.writeText "homeserver.py" ''
    import json
    from http.server import BaseHTTPRequestHandler, HTTPServer
    from urllib.parse import parse_qs, unquote, urlsplit

    ROOMS = {
        "!bridged:x": ["@owner:x", "@signal_1:x"],
        "!herdr:x": ["@owner:x", "@signal_1:x", "@herdr_x:x"],
        "!other:x": ["@owner:x", "@signal_2:x"],
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
  nodes.machine = { config, ... }: {
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
        agents = {
          bare = {
            cwd = "/home/alice/bare";
            kind = "pi";
            sandbox = {
              enable = true;
              program = [ "${probe}" ];
              files."models.json" = pkgs.writeText "models.json" (builtins.toJSON { providers = { }; });
              allow = { network = false; sendFiles = false; ports."11434" = "127.0.0.1:8080"; };
            };
          };
          nora = {
            cwd = "/home/alice/nora";
            sandbox = {
              enable = true;
              program = [ "${probe}" ];
              allow = { network = false; sendFiles = false; chats = { read = true; rooms = [ "!bridged:x" ]; }; };
            };
          };
          mia = {
            cwd = "/home/alice/mia";
            sandbox = {
              enable = true;
              program = [ "${probe}" ];
              allow = { chats = { read = true; send = true; }; sendFiles = true; network = true; };
            };
          };
        };
      };
    };
    # As Home Manager's NixOS module installs `home.packages`, and alice's
    # user units of `allow.ports`, for a user manager that runs from boot
    users.users.alice.packages = config.home-manager.users.alice.home.packages;
    users.users.alice.linger = true;
    systemd.user =
      let
        units = config.home-manager.users.alice.systemd.user;
        ports = lib.filterAttrs (name: _: lib.hasPrefix "herdr-matrix-port-" name);
      in
      {
        sockets = lib.mapAttrs (_: u: { unitConfig = u.Unit; socketConfig = u.Socket; wantedBy = u.Install.WantedBy; }) (ports units.sockets);
        services = lib.mapAttrs (_: u: { unitConfig = u.Unit; serviceConfig = u.Service; }) (ports units.services);
      };
    home-manager.users.bob = { };
    systemd.services.homeserver = {
      wantedBy = [ "multi-user.target" ];
      serviceConfig.ExecStart = "${pkgs.python3.interpreter} ${homeserver}";
    };
    # The model of the agent `bare`, reached only through its port
    systemd.services.model = {
      wantedBy = [ "multi-user.target" ];
      serviceConfig.ExecStart = "${pkgs.python3.interpreter} -m http.server 8080 --bind 127.0.0.1 --directory ${pkgs.writeTextDir "answer" "from the model"}";
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
    assert refused.strip() == "herdr-matrix: not a bridged chat", refused

    machine.fail("systemctl cat herdr-matrix-gate-bob.socket")
    bob = machine.fail("sudo -u bob herdr-matrix chats rooms 2>&1")
    assert bob.strip() == "herdr-matrix: the gate is not set up for this user", bob
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

    # Confined agents: each runs the probe through its command, as alice
    import json

    machine.succeed("mkdir -p /run/agenix /run/user && echo s > /run/agenix/secret && test -e /nix/var/nix")
    machine.succeed(
        "sudo -u alice sh -c 'umask 077 && mkdir -p ~/bare ~/nora ~/mia ~/.config/herdr ~/.cache/herdr-matrix/prompts && echo s > ~/secret'"
    )
    machine.wait_for_open_port(8080)
    machine.wait_for_unit("herdr-matrix-port-bare-11434.socket", "alice")
    launcher = "/etc/profiles/per-user/alice/bin/herdr-matrix-agent-bare bare 2>&1"
    unset = machine.fail(f"sudo -u alice {launcher}")
    assert unset.strip() == "herdr-matrix-agent-bare: XDG_RUNTIME_DIR is not set; allow.ports needs it", unset
    down = machine.fail(f"sudo -u alice env XDG_RUNTIME_DIR=/tmp {launcher}")
    assert down.strip() == "herdr-matrix-agent-bare: the sockets of allow.ports are not up: no /tmp/herdr-matrix/ports/bare", down
    machine.execute("sudo -u alice sleep 600 >/dev/null 2>&1 &")
    machine.wait_until_succeeds("pgrep -u alice -x sleep")
    for agent, kind in [("bare", "pi"), ("nora", "claude"), ("mia", "claude")]:
        probed = machine.succeed(
            "sudo -u alice env HERDR_PANE_ID=w1:p1 XDG_RUNTIME_DIR=/run/user/1000"
            f" /etc/profiles/per-user/alice/bin/herdr-matrix-agent-{agent} {agent} 2>&1"
        )
        assert probed.strip().endswith(f"probed {agent}"), probed
        machine.succeed(f"test -e /home/alice/{agent}/made")
        machine.succeed(f"test -e /home/alice/.local/state/herdr-matrix/agents/{agent}/{kind}/made")
    modes = machine.succeed(
        "cd /home/alice && stat -c %a .cache/herdr-matrix .cache/herdr-matrix/uploads .cache/herdr-matrix/uploads/mia"
        " .cache/herdr-matrix/agents .cache/herdr-matrix/agents/nora .local/state/herdr-matrix/agents/nora/claude"
        " .local/state/herdr-matrix/agents/bare/pi /run/user/1000/herdr-matrix /run/user/1000/herdr-matrix/ports/bare"
    )
    assert set(modes.split()) == {"700"}, modes
    entry = json.loads(machine.succeed("cat /home/alice/.cache/herdr-matrix/uploads/mia/*.json"))
    assert (entry["pane"], entry["name"], entry["caption"]) == ("", "shot.png", "Look"), entry
    machine.succeed("test \"$(cat /home/alice/.cache/herdr-matrix/uploads/mia/*.data)\" = data")
    machine.succeed("test \"$(stat -c %a /run/user/1000/herdr-matrix/ports/bare/11434)\" = 600")
    machine.succeed("test -f /home/alice/.local/state/herdr-matrix/agents/bare/pi/models.json")
    machine.succeed("test ! -s /home/alice/.local/state/herdr-matrix/agents/bare/pi/models.json")
    machine.succeed("systemctl cat herdr-matrix-gate-alice.nora.socket herdr-matrix-gate-alice.mia.socket")
    machine.fail("systemctl cat herdr-matrix-gate-alice.bare.socket")
  '';
}
