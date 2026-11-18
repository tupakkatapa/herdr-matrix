# herdr-matrix

> Written largely by AI, reviewed and maintained by me

A Matrix chat with the coding agents that [herdr](https://github.com/ogulcancelik/herdr) runs on one machine, for when you are away from the desk. Claude Code and pi answer with their own text; other agents show their screen.

## Features

- **One chat, one agent at a time**: what you write goes to the attached agent, and what it says comes back as it works
- **Away notices**: after `!away`, every agent that stops or waits on a choice posts here, until `!here`; reply to answer it without switching
- **Choices**: questions and permission prompts of Claude Code come as numbered lists; answer with a number, a key, your own text or a 👍
- **Files and voice**: pictures and files are handed to the agent; files that come within 5 s of each other go as one prompt. Voice messages are transcribed by whisper.cpp and count as written
- **Files from the agent**: `herdr-matrix send` posts a file in the agent's chat, a picture inline
- **Own chats**: `!dm <name>` gives an agent a direct chat of its own, as an appservice user named after the agent
- **Silent turns**: a turn whose last message is `NOOP` posts nothing, so a routine check can stay quiet
- **Prompts from the machine**: `herdr-matrix prompt` hands a prompt to an agent, for example on a schedule from `agents.<id>.prompts`, and its turn posts in the chat
- **Settings per agent**: `agents.<id>` gives the own chat of the agent working in a directory its name, prefix and picture
- **Chats for the agents**: `herdr-matrix chats` reads, searches and sends in your bridged chats (WhatsApp, Signal, Telegram), through a gate that alone holds your token
- **Confined agents**: `agents.<id>.sandbox` runs an agent in a bubblewrap sandbox with its folder and only the chats, files and network you grant
- **Agent kinds**: Claude Code and pi, also confined; `agents.<id>.kind` sets which one `!new` starts

## Getting Started

You need Home Manager, herdr with a module that defines `programs.herdr.enable` and `programs.herdr.package` and runs `herdr server` as the user service `herdr.service`, and a homeserver with application services.

Register an application service on the homeserver (tested on tuwunel; for Synapse, list the file in `app_service_config_files`):

```yaml
id: herdr
url: null
as_token: <secret>
hs_token: <secret>
sender_localpart: herdr
rate_limited: false
namespaces:
  users:
    - exclusive: true
      regex: '@herdr(_.*)?:example\.org'
```

Add this repository as a flake input and enable the module:

```nix
{
  inputs.herdr-matrix.url = "github:tupakkatapa/herdr-matrix";
}
```

```nix
# Home Manager
{
  imports = [ inputs.herdr-matrix.homeModules.default ];
  programs.herdr.enable = true;
  programs.herdr.matrix = {
    enable = true;
    homeserver = "https://matrix.example.org";
    owner = "@alice:example.org";
    environmentFile = "/run/agenix/herdr-matrix-env"; # HERDR_MATRIX_TOKEN=<as_token>
  };
}
```

At its first start, the bot invites `owner` to a direct chat. See [`module.nix`](module.nix) for every option.

## Commands

A command acts on the agent of the message you reply to, else on the attached one.

| Command | Action |
|---|---|
| `!agents` | Agents and what they do |
| `!attach <name>` | Attach the chat to an agent; part of a name is enough |
| `!new [folder]` | Start an agent in a folder under `~/Workspace`, or list them: Claude Code, or the `kind` of the folder's settings |
| `!dm <name>` | Give an agent its own chat; `!leave` ends it, the agent keeps running |
| `!last`, `!screen [n]` | What it said last; its terminal, 20 lines or `n` up to 200 |
| `!stop` | Interrupt it while it works |
| `!exit` | Asks first; `!exit now` ends it and closes its pane |
| `!away`, `!here` | Away notices on, off |
| `!help` | Help text |

A key (`1`-`9`, `y`, `n`, `enter`, `esc`, `tab`, `space`, arrows) answers a choice on the screen. A voice message of a key or a command word ("yes", "two", "screen 50") becomes that key or command; `!dm`, `!leave` and `!exit` cannot be spoken.

## Prompts from the machine

A program on the same machine, such as a systemd timer, prompts an agent through the running bot:

```sh
herdr-matrix prompt <agent> <text>...
herdr-matrix prompt --cwd <dir> <text>...
```

The command queues the prompt and exits. It names the agent like `!attach`, or by its working directory. The bot hands the text to the agent as written, without `[Element]`, and posts the last message of the turn in the agent's own chat, else in the main chat. An agent that works or waits on a choice does not get the prompt. Home Manager puts the binary on your `PATH`; a systemd unit, whose `PATH` is not yours, uses `${config.programs.herdr.matrix.package}/bin/herdr-matrix`.

Settings per agent, each for the agent working in `cwd`. `name`, `prefix` and `avatar` set its own chat user (`!dm`); each `prompts.<name>` is a user service and timer `herdr-matrix-prompt-<id>-<name>`:

```nix
programs.herdr.matrix.agents.assistant = {
  cwd = "${config.home.homeDirectory}/Workspace/assistant";
  name = "Kate"; # default: the agent's name
  prefix = false; # default: spawnPrefix
  avatar = ./kate.png; # default: spawnAvatar
  prompts.wake = {
    text = "Check the mail. When nothing is new, answer NOOP only.";
    onCalendar = "hourly";
  };
};
```

## Files from the agent

An agent posts a file in its chat with you:

```sh
herdr-matrix send <file> [caption]...
```

- The file posts in the agent's own chat, else in the main chat, within one poll (3 s). The caption is the text of the same message. A reply to the file goes to the agent.
- PNG, JPEG, GIF and WebP show as pictures, audio and video as players, all else as a file.
- The command works only in a herdr pane: the bot finds the agent by `HERDR_PANE_ID`. It refuses a path that is not a regular file, and an empty file, with one line of error and exit code 1.
- The command copies the file to the bot's queue and exits 0. The bot never opens the path the agent names. When the upload fails, a notice in the chat says why.
- The bot drops a file when the agent is gone, or when the bot is down for 10 minutes.

## Chats for the agents

The chat gate lets the agents read, search and send in your bridged chats with your own Matrix account. It runs as a system service, set up in the same block as the bot:

```nix
programs.herdr.matrix = {
  enable = true;
  homeserver = "https://matrix.example.org";
  gate = {
    enable = true;
    tokenFile = "/run/agenix/matrix-gate-token"; # your own access token
    bridges = [ "whatsapp" "signal" "telegram" ]; # the default
    bot = "herdr"; # the default: the sender_localpart of the bot's registration
  };
};
```

The NixOS configuration must import `inputs.herdr-matrix.nixosModules.default`, which makes the system user, the socket and the service. Standalone Home Manager cannot run the gate.

```sh
herdr-matrix chats rooms
herdr-matrix chats read <room> [count] [--since 7d] [--until 2026-11-01] [--from <end>]
herdr-matrix chats search <room> <words>... [--since 2w] [--until <time>] [--context 2]
herdr-matrix chats send <room> <text>...
```

Each command prints JSON, or one line of error and exit code 1.

- Your token is your account: with it, a process can answer the bot's permission prompts. The bot and the agents run as your user, so the token must stay out of that user's reach. Only the gate reads it, as a systemd credential, under its own system user.
- Each user with `gate.enable` gets a socket that only that user can open.
- The gate options are system configuration: systemd reads `tokenFile` as root and sends the token to `homeserver`. Do not let an untrusted user write `home-manager.users.<user>`.
- The gate serves only rooms with a bridged contact on your server, and refuses every room that holds the bot or one of its users.
- Messages from other people are untrusted input for the agent; say so in its instructions.

`read` gives the last 20 messages, or a whole range with `--since`, up to 5000; `end` in its answer continues with `--from`. Times are in UTC. The gate cannot read end-to-end encrypted rooms.

## Agent kinds

The bot works with every agent that herdr knows: it sends prompts and keys, and it shows the status and the screen. It reads the answers of Claude Code and pi from their own session files. An agent of another kind posts its screen at the end of a turn.

herdr names the kind of each agent. `agents.<id>.kind` (`"claude"`, the default, or `"pi"`) sets what `!new` starts in the folder, and the program and state directory of a confined agent.

What carries over to pi:

| Feature | pi | Note |
|---|---|---|
| Messages in, with `[Element]` | Yes | A text that starts with `/` goes bare, as for Claude; pi runs it as its own command |
| Progress and final answer | Yes | pi writes each message when it ends, so progress comes per message |
| Silent `NOOP` turns | Yes | |
| Files from the owner | Yes | The path is in the prompt; pi reads it with its `read` tool |
| Files to the owner | Yes | `herdr-matrix send`, as for any pane |
| Voice | Yes | |
| `!away`, `!here`, notices | Yes | |
| Prompts on a schedule | Yes | A prompt that starts with `!` runs as a shell command in pi; do not write one |
| `!stop`, `!screen`, `!last`, `!attach`, `!dm` | Yes | |
| Choices and permission prompts | No | pi has none: it runs its tools without asking. A dialog of a pi extension shows on `!screen` only |
| Blocked state | No | herdr learns it only from pi's herdr extension, which needs herdr's socket |
| A branch made with `/tree` | Partly | The bot reads the file in order; after a jump to an older point it can show a message of the other branch |

The bot reads the pi session that herdr reports, else the newest one for the agent's folder in `~/.pi/agent/sessions`. It does not read sessions that `PI_CODING_AGENT_DIR` or `--session-dir` puts elsewhere.

A confined pi agent with a local model:

```nix
programs.herdr.matrix.agents.nora = {
  cwd = "${config.home.homeDirectory}/Workspace/nora";
  kind = "pi"; # default: "claude"
  sandbox = {
    enable = true;
    program = [ "pi" "--provider" "ollama" "--model" "qwen3:8b" ]; # default: [ "pi" ]
    files."models.json" = pkgs.writeText "models.json" (builtins.toJSON {
      providers.ollama = {
        baseUrl = "http://127.0.0.1:11434/v1";
        api = "openai-completions";
        apiKey = "ollama";
        models = [{ id = "qwen3:8b"; }];
      };
    });
  };
};
```

pi in the sandbox reads its settings from its state directory, `$XDG_STATE_HOME/herdr-matrix/agents/nora/pi`. `files` puts `models.json` there, read-only.

## Confined agents

An agent without `sandbox.enable` has full trust: it runs as your user and can do what your user can.

A confined agent runs in a bubblewrap sandbox and can use only what its options grant:

```nix
programs.herdr.matrix.agents.nora = {
  cwd = "${config.home.homeDirectory}/Workspace/nora";
  sandbox = {
    enable = true;
    program = [ "claude" ]; # the default
    environment.ANTHROPIC_BASE_URL = "http://127.0.0.1:11434";
    files."CLAUDE.md" = ./CLAUDE.md; # default: { }
    allow = {
      readPaths = [ ]; # more paths, read-only
      network = false; # default: true
      ports."11434" = "10.23.0.18:11444"; # default: { }
      chats.read = true; # default: false
      chats.send = false; # default: false
      chats.rooms = [ ]; # default: every bridged chat
      sendFiles = true; # the default
    };
  };
};
```

Home Manager puts the command `herdr-matrix-agent-nora` on your `PATH`. Start the agent in a pane with `exec herdr-matrix-agent-nora`; `!new` in the agent's folder types the same. With `exec`, the pane closes when the agent exits, and no shell outside the sandbox gets your next message.

Inside the sandbox, the agent has:

- its `cwd`, read and write;
- its own state directory, `$XDG_STATE_HOME/herdr-matrix/agents/nora/<kind>`, read and write, as `CLAUDE_CONFIG_DIR` for Claude Code or `PI_CODING_AGENT_DIR` for pi;
- files you send to it, in `$XDG_CACHE_HOME/herdr-matrix/agents/nora`, read-only;
- the Nix store, `/etc`, `/run/current-system` and `allow.readPaths`, read-only;
- an empty home and `/tmp`, its own `/proc` and `/dev`, and its terminal;
- only the environment variables `HOME`, `PATH`, `TERM`, `LANG`, the variable of its state directory, `HERDR_AGENT`, `environment`, and those of its grants. `HERDR_AGENT` names the kind, so herdr knows the agent.

It does not see the rest of your home, `/run/user`, `/run/agenix`, herdr's socket, the Nix daemon, the bot's files and queues, other agents' folders, or the processes of the host.

Each grant is one path that only this sandbox holds:

- `network`, on by default, shares the host's network, which an agent needs to reach its model. Off, the agent has no network, not even `127.0.0.1`, but `ports`.
- `ports` gives an agent without `network` a fixed set of TCP addresses: inside, `127.0.0.1:11434` reaches `10.23.0.18:11444`, and no other address exists. Each port is a user socket `herdr-matrix-port-nora-11434.socket` on `$XDG_RUNTIME_DIR/herdr-matrix/ports/nora/11434`, served by `systemd-socket-proxyd`. The command binds that directory read-only and starts a `socat` per port in the sandbox before the agent; while the sockets are not up, it stops with an error.
- `files` puts files in the state directory, read-only: the key is the path in the state directory, the value the file. Each start shows the file of the current generation. The mount point stays there as an empty file after the agent exits; the bot does not take it for a session file.
- `chats.read` gives `herdr-matrix chats rooms`, `read` and `search`; `chats.send` gives `rooms` and `send`. They go to the socket `/run/herdr-matrix-gate/<user>.nora/socket`, whose gate refuses other commands with `not allowed for this agent`. `chats.rooms` limits the agent to these room ids; to the agent, other rooms do not exist. It needs `gate.enable`; the rules of the [chat gate](#chats-for-the-agents) apply first.
- `sendFiles`, on by default, gives `herdr-matrix send`; you already get every text of the agent. The file goes to the queue `$XDG_CACHE_HOME/herdr-matrix/uploads/nora`, and the bot posts it as the agent working in `cwd`. The queue names the agent, never what the agent writes in it.

The bot reads the agent's answers from the newest session file in its state directory. It reads what the agent writes there and in its queue as untrusted: it follows no symlink and opens only regular files.

What is not enforced:

- An agent started without `herdr-matrix-agent-<id>`, for example `claude` or `pi` typed by hand in the folder, is not confined.
- An agent with the network reaches every service the host reaches: local services, and the abstract unix sockets of the host.
- Your other, unconfined agents run as your user, and can use the sockets and queues of a confined agent.
- The sandbox runs as your user. The namespaces are the boundary: no other mount, process or IPC view.
- herdr takes a pane's directory from what the pane's terminal reports. A confined agent can report the folder of another agent: its chat user then takes that agent's name and picture, and a prompt for that folder is dropped or reaches the wrong agent. It gains no file, socket or chat.
- What a confined agent writes in its folder is untrusted: do not run its Git hooks or scripts, and tell agents that read the folder so.

The agent in the sandbox starts with a state directory of its own that holds only `files`.

## Limitations

- Answers in text come only from Claude Code and pi. An agent of another kind posts its screen at the end of a turn. pi has no choices and no blocked state in the chat (see [Agent kinds](#agent-kinds)).
- The owner's Matrix login is the only guard: whoever holds it has a shell as the bot's user, and so does whoever holds the appservice token. The agents run as that user and can read the token.
- The bot uses only rooms of the owner and itself, joined by invite; when anyone else joins the chat or is invited, it leaves and makes a new one. It does not read encrypted rooms or files.
- herdr takes prompts as command-line arguments: on a host whose `/proc` lacks `hidepid`, other local users can read them.
- A one-word voice message such as "yes" answers a choice at once, a permission prompt too, and whisper may hear such a word in noise.
- Messages older than 10 minutes when they arrive, for example after downtime, are skipped.
- Files go in both directions up to the homeserver's upload limit (`m.upload.size`). Files of the chat stay in `$XDG_CACHE_HOME/herdr-matrix`, else `~/.cache/herdr-matrix`, for 30 days, readable by your user; the homeserver keeps its own copy. An agent that needs a file for longer copies it.
- A large file blocks the bot while it moves, up to 1 minute. Pictures from the agent post without a thumbnail; the homeserver makes one.
- A voice message blocks the bot while it is transcribed, up to 2 minutes per step. An outage shows only in `journalctl --user -u herdr-matrix`.

## License

GPL-3.0-or-later
