# herdr-matrix

> Written largely by AI, reviewed and maintained by me

A Matrix chat with the Claude Code agents that [herdr](https://github.com/ogulcancelik/herdr) runs on one machine, for when you are away from the desk.

## Features

- **One chat, one agent at a time**: what you write goes to the attached agent, and what it says comes back as it works
- **Away notices**: after `!away`, every agent that stops or waits on a choice posts here, until `!here`; reply to answer it without switching
- **Choices**: questions and permission prompts come as numbered lists; answer with a number, a key, your own text or a 👍
- **Files and voice**: pictures and files are handed to the agent; voice messages are transcribed by whisper.cpp and count as written
- **Own chats**: `!dm <name>` gives an agent a direct chat of its own, as an appservice user

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
