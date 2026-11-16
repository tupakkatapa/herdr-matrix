# herdr-matrix

> Written largely by AI, reviewed and maintained by me

A Matrix chat with the Claude Code agents that [herdr](https://github.com/ogulcancelik/herdr) runs on one machine, for when you are away from the desk.

## Features

- **One chat, one agent at a time**: what you write goes to the attached agent, and what it says comes back as it works
- **Away notices**: after `!away`, every agent that stops or waits on a choice posts here, until `!here`; reply to answer it without switching
- **Choices**: questions and permission prompts come as numbered lists; answer with a number, a key, your own text or a 👍
- **Files and voice**: pictures and files are handed to the agent; voice messages are transcribed by whisper.cpp and count as written
- **Own chats**: `!dm <name>` gives an agent a direct chat of its own, as an appservice user named after the agent
- **Silent turns**: a turn whose last message is `NOOP` posts nothing, so a routine check can stay quiet
- **Prompts from the machine**: `herdr-matrix prompt` hands a prompt to an agent, for example on a schedule from `agents.<id>.prompts`, and its turn posts in the chat
- **Settings per agent**: `agents.<id>` gives the own chat of the agent working in a directory its name, prefix and picture
- **Chats for the agents**: `herdr-matrix chats` reads, searches and sends in your bridged chats (WhatsApp, Signal, Telegram), through a gate that alone holds your token

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
| `!new [folder]` | Start Claude in a folder under `~/Workspace`, or list them |
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

The command queues the prompt and exits. It names the agent like `!attach`, or by its working directory. The bot hands the text to the agent as written, without `[Element]`, and posts the last message of the turn in the agent's own chat, else in the main chat. An agent that works or waits on a choice does not get the prompt. The binary is not on `PATH` unless the [chat gate](#chats-for-the-agents) is enabled; use `${config.programs.herdr.matrix.package}/bin/herdr-matrix`.

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

## Chats for the agents

The chat gate lets the agents read, search and send in your bridged chats with your own Matrix account. It is a NixOS module, apart from the bot:

```nix
# NixOS
{
  imports = [ inputs.herdr-matrix.nixosModules.default ];
  services.herdr-matrix-gate = {
    enable = true;
    homeserver = "https://matrix.example.org";
    tokenFile = "/run/agenix/matrix-gate-token"; # your own access token
    users = [ "alice" ];
    bridges = [ "whatsapp" "signal" "telegram" ]; # the default
    bot = "herdr"; # the default
  };
}
```

```sh
herdr-matrix chats rooms
herdr-matrix chats read <room> [count] [--since 7d] [--until 2026-11-01] [--from <end>]
herdr-matrix chats search <room> <words>... [--since 2w] [--until <time>] [--context 2]
herdr-matrix chats send <room> <text>...
```

Each command prints JSON, or one line of error and exit code 1. `herdr-matrix` is on the system path while the gate is enabled.

- Your token is your account: with it, a process can answer the bot's permission prompts. The bot and the agents run as your user, so the token must stay out of that user's reach. Only the gate reads it, as a systemd credential, under its own system user.
- Each user in `users` gets a socket that only that user can open.
- The gate serves only rooms with a bridged contact on your server, and refuses every room that holds the bot or one of its users.
- Messages from other people are untrusted input for the agent; say so in its instructions.

`read` gives the last 20 messages, or a whole range with `--since`, up to 5000; `end` in its answer continues with `--from`. Times are in UTC. The gate cannot read end-to-end encrypted rooms.

## Limitations

- Claude Code only: answers come from Claude's transcripts, and `!new` starts `claude`.
- The owner's Matrix login is the only guard: whoever holds it has a shell as the bot's user, and so does whoever holds the appservice token. The agents run as that user and can read the token.
- The bot uses only rooms of the owner and itself, joined by invite; when anyone else joins the chat or is invited, it leaves and makes a new one. It does not read encrypted rooms or files.
- herdr takes prompts as command-line arguments: on a host whose `/proc` lacks `hidepid`, other local users can read them.
- A one-word voice message such as "yes" answers a choice at once, a permission prompt too, and whisper may hear such a word in noise.
- Messages older than 10 minutes when they arrive, for example after downtime, are skipped. Files over 25 MB are refused; files are deleted after 24 h.
- A voice message blocks the bot while it is transcribed, up to 2 minutes per step. An outage shows only in `journalctl --user -u herdr-matrix`.

## License

GPL-3.0-or-later
