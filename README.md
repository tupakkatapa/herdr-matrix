# herdr-matrix

> Written largely by AI, reviewed and maintained by me

A Matrix chat with the coding agents that [herdr](https://github.com/ogulcancelik/herdr) runs on one machine, for when you are away from the desk.

## Features

- **One chat, one agent at a time**: what you write goes to the attached agent, and what it says comes back as it works
- **Away notices**: after `!away`, every agent that stops or waits on a choice posts here, until `!here`; reply to answer it without switching
- **Choices**: questions and permission prompts of Claude Code come as numbered lists; answer with a number, a key, `yes` or `no`, your own text or a 👍
- **Files and voice**: pictures and files are handed to the agent; files that come within 5 s of each other go as one prompt. Voice messages are transcribed by whisper.cpp and count as written
- **Files from the agent**: `herdr-matrix send` posts a file in the agent's chat, a picture inline
- **Own chats**: `!dm <name>` gives an agent a direct chat of its own, as an appservice user named after the agent
- **Silent turns**: a turn whose last message is `NOOP` posts nothing, so a routine check can stay quiet
- **Prompts from the machine**: `herdr-matrix prompt` hands a prompt to an agent, for example on a schedule from `agents.<id>.prompts`, and its turn posts in the chat
- **Settings per agent**: `agents.<id>` sets the name, prefix and picture of the own chat of the agent that works in a directory. It can start the agent when the bot comes up, and open its chat then
- **Chats for the agents**: `herdr-matrix chats` reads, searches and sends in your bridged chats (WhatsApp, Signal, Telegram), through a gate that alone holds your token
- **Confined agents**: `agents.<id>.sandbox` runs an agent in a bubblewrap sandbox with its folder and only the chats, files and network you grant
- **Agent kinds**: Claude Code and pi, also confined; `agents.<id>.kind` sets which one `!new` starts

## Getting Started

1. **Have the parts.**
   - A Matrix homeserver that you administer: you add an application service to it.
   - Home Manager.
   - herdr with a Home Manager module that defines `programs.herdr.enable` and `programs.herdr.package`, and runs `herdr server` as the user unit `herdr.service`. This flake does not ship that module.
   - The herdr package must have `src` with `assets/logo.png`, or set `programs.herdr.matrix.avatar`.

2. **Register the application service.** Make two secrets:

   ```sh
   openssl rand -hex 32 # as_token
   openssl rand -hex 32 # hs_token
   ```

   Write the registration file, with your server in the regex:

   ```yaml
   id: herdr
   url: null
   as_token: <as_token>
   hs_token: <hs_token>
   sender_localpart: herdr
   rate_limited: false
   namespaces:
     users:
       - exclusive: true
         regex: '@herdr(_.*)?:example\.org'
   ```

   - `sender_localpart` is the bot's user, `@herdr:example.org`. Each agent's own chat user is `@herdr_<name>:example.org`. The regex must hold both.
   - The bot does not use `hs_token`: with `url: null`, the homeserver sends nothing to the bot. The file needs it all the same.
   - Synapse: list the file in `app_service_config_files` in `homeserver.yaml`, then restart Synapse. Other servers (the bot is tested on tuwunel): see your homeserver's appservice documentation.

3. **Write the secret file.** It holds the `as_token` of step 2 and nothing else:

   ```sh
   <as_token>
   ```

   Keep it out of the Nix store and readable by your user only, for example with agenix or sops-nix. As an alternative, `environmentFile` takes a file with the line `HERDR_MATRIX_TOKEN=<as_token>`. Set one of them, not both.

4. **Enable the module.** Add the flake input. Pass `inputs` to Home Manager (`extraSpecialArgs = { inherit inputs; };`). Import the Home Manager module of herdr beside this one.

   ```nix
   {
     inputs.herdr-matrix.url = "github:tupakkatapa/herdr-matrix";
   }
   ```

   ```nix
   # Home Manager
   { inputs, ... }:
   {
     imports = [
       inputs.herdr.homeModules.default # your module that defines programs.herdr
       inputs.herdr-matrix.homeModules.default
     ];
     programs.herdr.enable = true;
     programs.herdr.matrix = {
       enable = true;
       homeserver = "https://matrix.example.org";
       owner = "@alice:example.org"; # your own Matrix user
       tokenFile = "/run/agenix/herdr-matrix-token"; # the file of step 3
     };
   }
   ```

   See [`module.nix`](module.nix) for every option.

5. **Switch, then check the service.**

   Run `home-manager switch`, or `nixos-rebuild switch` when Home Manager runs as a NixOS module. Then check the user service `herdr-matrix`:

   ```sh
   systemctl --user status herdr-matrix
   journalctl --user -u herdr-matrix -f
   ```

6. **Accept the invite.** At its first start, the bot invites `owner` to a direct chat. Accept it, then send `!help`.

## Using the chat

- The main chat is attached to one agent at a time. What you write goes to that agent. When only one agent runs, it counts as attached.
- A reply to a message of an agent goes to that agent, without switching.
- `!away` and `!here` switch the notices of all agents; see [Away](#away).
- `!dm` gives an agent a chat of its own, fixed to that agent; see [Own chats](#own-chats).

### Commands

`!help` lists them:

| Command | Action |
|---|---|
| `!agents` | List the agents |
| `!attach <name>` | Talk to that agent |
| `!new [folder]` | Start an agent there, or list folders |
| `!dm <name>` | Give it its own chat |
| `!leave <name>` | Close its own chat; it keeps running |
| `!last` | What it said last |
| `!screen [n]` | Its screen, n <= 200 lines |
| `!stop` | Interrupt what it is doing |
| `!exit` | Quit the agent and close its pane (asks first) |
| `!away` | Every agent posts here when it stops or asks |
| `!here` | Only answers to your messages come here |
| `!help` | This text |

- `!exit` asks first. `!exit now` works for that agent for 2 minutes. For another agent, or later, it asks again.
- `!last`, `!screen`, `!stop` and `!exit` act on the agent of the message you reply to, else on the attached one.
- `!attach`, `!dm` and `!leave` take a name (part of it is enough), a number from the last list the bot showed, or the message you reply to. With none of these, the bot lists the agents.
- A number that is not in the last list, as in `!attach 2`, answers `No agent 2 in the last list.` and shows the list.
- `!agents` numbers the agents, marks the attached one with `>` and an agent with its own chat with `(own chat)`.
- `!screen` shows 20 lines unless you give `n`. `!stop` works while the agent works.
- Small slips are understood: `!stop.`, `! stop` and `!Stop` all work. For an unknown command, the bot names the nearest one.
- Other names: `!list` for `!agents`, `!spawn` and `!chat` for `!dm`, `!kill` and `!unchat` for `!leave`, `!close` for `!exit`, `!afk` for `!away`, `!back` and `!auto` for `!here`. `!kill` closes the chat; it does not quit the agent.

### Messages and answers

- The bot puts `[Element]` before each message, so that the agent answers for a phone. Tell the agent in its instructions what the prefix means. A message that starts with `/` goes without it, as a Claude Code slash command.
- A read receipt on your message means that the agent has it. While the agent works, the chat shows it typing.
- Progress comes as quiet notices. The last message of a turn comes as a normal message, which notifies. When that message already went out as progress, the bot sends it again to notify; when it is longer than 500 characters, the bot says `<name> is done; its answer is above.`
- A turn without text ends with `Done.`. A turn that came from `herdr-matrix prompt` posts nothing instead.
- A turn whose last message is `NOOP` posts nothing; `!last` still shows it.
- With no agent attached, the bot keeps what you send for 2 minutes after the last item. Files and texts go together as one bundle.
  - The bot posts the list of agents once. It answers later items quietly: `Kept with the rest; !attach an agent and I send it.`
  - It sends the bundle after `!attach`, or when a `!new` agent is ready, and says `Sent your message to <name>.`
  - An agent started with `!new` that waits on a choice gets the bundle when it is ready. If the 2 minutes pass first, the bot says `**<name>** still waits on a choice; your message was not sent.`
- When the attached agent ends, the bot says so after about a minute: `<name> ended. Nothing is attached now: !attach another agent, or !new to start one.` A message to it meanwhile is not sent, and the bot says so.

### Choices

- A question of the agent comes as a numbered list. Answer with a number, or write your own answer. A question with several parts, or with options to tick, takes one key at a time (`up`, `down`, `space`, `enter`); `!screen` shows where you are.
- A permission prompt or another choice on the screen comes as a numbered list, with the lines above it (the command and its description).
- The keys are `1`-`9`, `y`, `n`, `enter`, `esc`, `tab`, `space`, `up`, `down`, `left` and `right`.
- For a choice on the screen, write a key or one of these words for it:

  | Key | Words |
  |---|---|
  | `y` | `yes`, `yeah` |
  | `n` | `no` |
  | `1` to `9` | `one` to `nine` |
  | `enter` | `enter` |
  | `esc` | `escape`, `cancel` |

  Capitals, spaces around the word and a closing punctuation mark do not matter: `Yes!` is `y`.
- A 👍 on the message that shows the choice picks its first option. Other reactions do nothing.
- An answer goes only to the choice the chat showed. When the choice changed, the bot sends nothing, says so and shows the new choice. When the chat has not shown the choice yet, the bot shows it first.

### Spoken messages

- The bot transcribes only a voice message, a recording that the client marks as one. An audio file is a file: the agent gets its path. A client that does not mark its voice messages sends files.
- The bot posts what it heard as a quiet notice, `🎤 <text>`, with `→ <key or command>` when the text became one. Then it acts on it.
- One word that is a key or a word for a key (see [Choices](#choices)) becomes that key.
- A command word becomes the command when the rest is what the command takes: nothing for `agents`, `last`, `stop`, `away`, `here` and `help`; one word, or nothing, for `attach` and `new`; a number, or nothing, for `screen`. The other names work too, except `auto`.
- `!dm`, `!leave` and `!exit` cannot be spoken. "Exit now" goes to the agent as heard.
- Anything longer goes to the agent as heard: "Stop, wait" and "new tests, please" are prompts.

#### Voice model

The bot transcribes your voice messages with whisper.cpp and handles the text as if you wrote it. Set a model to turn it on:

```nix
programs.herdr.matrix.transcribe = {
  model = {
    url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin";
    hash = "sha256-G+OpsgY4Z7k35k4ux0gzZKeZF+FX+pjF2UtcH//qmHs=";
  };
  language = "en"; # default: "auto", which detects the language
};
```

- The model stays out of the Nix closure.
- Each start of the bot starts the user service `herdr-matrix-models`. It downloads a missing model to `~/.config/herdr/models` and checks its hash. After a download it restarts the bot.
- Watch the download with `journalctl --user -u herdr-matrix-models -f`.
- While the model downloads, the bot refuses voice messages with a note that says so. Without `transcribe.model`, it refuses them.
- `language` takes what `whisper-cli -l` takes.

### Files

#### To the agent

- A picture, video or file is saved. The prompt holds its caption and path: `See the file <path>` without a caption.
- Files that come within 5 s of each other for one agent go as one prompt: the captions, then one path a line. A text or voice message hands the waiting files over first.
- The files stay in `$XDG_CACHE_HOME/herdr-matrix`, else `~/.cache/herdr-matrix`, for 30 days. An agent that needs a file for longer copies it. A confined agent reads its own files in `agents/<id>` under that directory.
- The bot refuses an encrypted file. The upload size of the homeserver limits files in both directions.

#### From the agent

An agent posts a file in its chat with you:

```sh
herdr-matrix send <file> [caption]...
```

- The file posts in the agent's own chat, else in the main chat, within one poll (3 s). The caption is the text of the same message. A reply to the file goes to the agent.
- PNG, JPEG, GIF and WebP show as pictures, audio and video as players, all else as a file.
- The command works only in a herdr pane: the bot finds the agent by `HERDR_PANE_ID`. It refuses a path that is not a regular file, and an empty file, with one line of error and exit code 1.
- The command copies the file to the bot's queue and exits 0. The bot never opens the path the agent names.
- The bot drops a file when the agent is gone, or when the file is older than 10 minutes, for example after downtime.
- A file that the homeserver fails to take (no connection, or a 5xx answer) is tried 3 times, one minute apart, within 10 minutes. Then one notice, `Could not send <name>: <why>`, says why.
- Another failure gives that notice at once.

### Starting agents

- `!new` lists the folders under `workspace`, 20 at most. `!new <part of a name>` looks one or two levels deep. One match starts an agent there; several matches, or none, list folders.
- The agent is Claude Code, or the `kind` of the folder's settings. A confined agent starts through its command `herdr-matrix-agent-<id>`.
- The bot says `Started <kind> as <folder> in <path>; this chat is attached to it.` When the agent is idle, after its first choice if it starts on one, the bot says `<folder> is ready.` quietly and sends the kept bundle.
- When the pane stops while the agent starts, the bot says `<folder> stopped while it started. Run its command in a pane to see why.`
- When an agent runs in that folder already, the bot attaches to it.
- The bot refuses a folder inside the folder of a confined agent, because that agent writes there: `<path> is inside the folder of the confined agent <id>; start it with !new <name>.`

### Away

- Away is off until `!away`, and on until `!here`. Off, only the agents you prompted post in it, from the chat or with `herdr-matrix prompt`.
- On, every agent that stops or waits on a choice posts in the main chat, under its name. The name of the main chat ends in `· away`, and `!agents` starts with `Agents (away is on):`.
- When away turns on, the bot posts every agent that waits on a choice. It posts each message of an agent once, also after a restart of the bot. An agent that stops on `NOOP` posts nothing.

### Own chats

- `!dm <name>` makes the user `@herdr_<name>`, which invites you to a direct chat. Accept the invite.
- What you send there (text, pictures, files, voice messages, answers, 👍) reaches only that agent. It answers there, with progress, typing and read receipts. A turn started there, and the away notices of that agent, go to that chat only.
- `!last`, `!screen`, `!stop`, `!exit`, `!away`, `!here`, `!help` and `!leave` work there. `!agents`, `!attach`, `!new`, `!dm` and `!leave <name>` work in the main chat only.
- The chat ends in these cases. The agent keeps running, except after `!exit now`.
  - You send `!leave` there, or `!leave <name>` in the main chat. The bot says `Bye. You can leave this room.` and leaves.
  - You send `!exit now` there. The bot closes the agent, then says the same goodbye.
  - The agent has no settings in `agents`, and is missing from herdr's list for three polls and a minute. The chat says `The agent ended.`, then the same goodbye. A message sent there meanwhile is not delivered; the chat says so.
  - You remove the settings of the agent. The chat ends at the first start of the bot after that, with the same goodbye.
  - `owner` changes. The bot says goodbye in each chat of the old owner.
  - Another user joins or is invited. The bot leaves without a word.
- The chat of an agent that has settings in `agents` stays while the agent is in the configuration, also while the agent is not running.
  - It follows the agent to a new pane when the bot starts the agent (`!new`, `autostart`).
  - After a start by hand, `!dm <name>` continues the same chat.
  - `chat = true` opens the chat by itself only for an agent the bot started.
  - After `!leave`, the bot opens no chat for the agent again until `!dm`, also after a restart.

## Settings

### Bot and own chats

These options of `programs.herdr.matrix` set the bot and the own chats:

| Option | Default | What it sets |
|---|---|---|
| `displayName` | `"herdr"` | The display name of the bot |
| `avatar` | The herdr logo, `assets/logo.png` in the source of `programs.herdr.package` | The picture of the bot, a PNG |
| `ownChat.prefix` | `true` | Puts `displayName` before the agent's name in the name of the user of an own chat, as `herdr · repo`. An agent's `prefix` replaces it |
| `ownChat.avatar` | `avatar` | The picture of the user of an own chat, a PNG. An agent's `avatar` replaces it |
| `workspace` | `~/Workspace` | The folder whose folders `!new` lists and starts agents in |
| `package` | `pkgs.callPackage ./package.nix { }` | The herdr-matrix package. It is in `home.packages`, so `herdr-matrix` works in an agent's pane |

### Per agent

`programs.herdr.matrix.agents.<id>` holds the settings of one agent: the agent that herdr reports in the folder `cwd`. An agent has four identifiers:

| Identifier | Example | What it names |
|---|---|---|
| Attribute name `<id>` | `assistant` | The units `herdr-matrix-prompt-<id>-<name>`; for a confined agent, its command `herdr-matrix-agent-<id>`, its directories and its socket |
| `cwd` | `~/Workspace/assistant` | Which herdr agent the settings apply to: the agent that works in this folder |
| `displayName` | `Kate` | The display name of the user of its own chat (`!dm`) |
| herdr's name | As `!agents` shows it | The agent in `!attach`, `!agents` and `herdr-matrix prompt` |

- `cwd` is an absolute path. A trailing `/` is ignored. Each agent has its own folder.
- `displayName`, `prefix` and `avatar` set the user of the agent's own chat (`!dm`). Unset, `prefix` and `avatar` are `ownChat.prefix` and `ownChat.avatar`.
- `chat = true` opens the agent's own chat when the bot starts the agent (`!new`, `autostart`). Without it, `!dm` opens one. See [Own chats](#own-chats).
- `autostart = true` starts the agent when the bot comes up and no agent works in `cwd`.
  - The bot starts it as `!new` does. The main chat stays attached as it is.
  - The bot starts the agent once in a run of the bot. It does not start it again after the agent exits, or after a start that fails.
  - When `cwd` is missing, the bot logs it and does not start the agent.
- Each `prompts.<name>` is a user service and timer `herdr-matrix-prompt-<id>-<name>`. See [Prompts from the machine](#prompts-from-the-machine).
- `kind` and `sandbox` are in [Agent kinds](#agent-kinds) and [Confined agents](#confined-agents).

```nix
programs.herdr.matrix.agents.assistant = {
  cwd = "${config.home.homeDirectory}/Workspace/assistant";
  displayName = "Kate"; # default: herdr's name for the agent
  prefix = false; # default: ownChat.prefix
  avatar = ./kate.png; # default: ownChat.avatar
  chat = true; # default: false
  autostart = true; # default: false
  prompts.wake = {
    text = "Check the mail. When nothing is new, answer NOOP only.";
    onCalendar = "hourly";
  };
};
```

## Prompts from the machine

A program on the same machine, such as a systemd timer, prompts an agent through the running bot:

```sh
herdr-matrix prompt <agent> <text>...
herdr-matrix prompt --cwd <dir> <text>...
```

- The command queues the prompt and exits. It names the agent like `!attach`, or by its working directory.
- The bot hands the text to the agent as written, without `[Element]`. It posts the last message of the turn in the agent's own chat, else in the main chat.
- The bot drops the prompt when the agent works or waits on a choice, when the name matches no agent or several, or after 10 minutes.
- When herdr fails to take the prompt, it goes back to the queue.
- Home Manager puts the binary on your `PATH`. A systemd unit, whose `PATH` is not yours, uses `${config.programs.herdr.matrix.package}/bin/herdr-matrix`.

## Chats for the agents

The chat gate lets the agents read, search and send in your bridged chats with your own Matrix account. It runs as a system service. Home Manager must run as a NixOS module: the NixOS module of this flake reads `home-manager.users` and makes the gate's system user, sockets and services, also the socket of each confined agent with `allow.chats`. Standalone Home Manager cannot run the gate.

```nix
# NixOS
{ inputs, ... }:
{
  imports = [
    inputs.home-manager.nixosModules.home-manager
    inputs.herdr-matrix.nixosModules.default
  ];
  home-manager.users.alice = {
    # The Home Manager block of Getting Started, and:
    programs.herdr.matrix = {
      gate = {
        enable = true;
        tokenFile = "/run/agenix/matrix-gate-token"; # your own access token
        bridges = [ "whatsapp" "signal" "telegram" ]; # default: the same list
        bot = "herdr"; # default: "herdr"; the sender_localpart of the bot's registration
      };
    };
  };
}
```

- `tokenFile` holds the access token of your own account. In Element: Settings, Help & About, Access Token. The token ends when that session logs out, so use a session that you keep.
- `gate.bot` must equal the `sender_localpart` of the registration. A wrong value lifts the guard that keeps your token out of the bot's chats.

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

`read` gives the last 20 messages, or a whole range with `--since`. One request walks through at most 5000 events; `end` in its answer continues with `--from`. Times are in UTC. The gate cannot read end-to-end encrypted rooms.

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

A pi agent:

```nix
programs.herdr.matrix.agents.nora = {
  cwd = "${config.home.homeDirectory}/Workspace/nora";
  kind = "pi"; # default: "claude"
};
```

## Confined agents

An agent without `sandbox.enable` has full trust: it runs as your user and can do what your user can.

A confined agent runs in a bubblewrap sandbox and can use only what its options grant. The pi agent `nora` of [Agent kinds](#agent-kinds) gets the `sandbox` block below, with each grant set. Its model is on another host:

```nix
programs.herdr.matrix.agents.nora.sandbox = {
  enable = true;
  program = [ "pi" "--provider" "ollama" "--model" "qwen3:8b" ]; # default: [ kind ]
  environment = { }; # default: { }
  files."models.json" = ./models.json; # default: { }
  allow = {
    readPaths = [ ]; # default: [ ]
    network = false; # default: true
    ports."11434" = "10.23.0.18:11444"; # default: { }
    chats.read = true; # default: false
    chats.send = false; # default: false
    chats.rooms = [ ]; # default: [ ], which means every bridged chat
    sendFiles = true; # default: true
  };
};
```

pi in the sandbox reads its settings from its state directory, `$XDG_STATE_HOME/herdr-matrix/agents/nora/pi`. `files` puts `models.json` there, read-only. In that file, the model address `http://127.0.0.1:11434/v1` reaches the model through `allow.ports`.

Home Manager puts the command `herdr-matrix-agent-nora` on your `PATH`. Start the agent in a pane with `exec herdr-matrix-agent-nora`; `!new` in the agent's folder types the same. With `exec`, the pane closes when the agent exits, and no shell outside the sandbox gets your next message. With `autostart = true`, the bot starts the agent through this command when it comes up, so you do not type it by hand.

Inside the sandbox, the agent has:

- its `cwd`, read and write;
- its own state directory, `$XDG_STATE_HOME/herdr-matrix/agents/nora/<kind>`, read and write, as `CLAUDE_CONFIG_DIR` for Claude Code or `PI_CODING_AGENT_DIR` for pi;
- files you send to it, in `$XDG_CACHE_HOME/herdr-matrix/agents/nora`, read-only;
- the Nix store, `/etc`, `/run/current-system` and `allow.readPaths`, read-only;
- an empty home and `/tmp`, its own `/proc` and `/dev`, and its terminal;
- only the environment variables `HOME`, `PATH`, `TERM`, `LANG`, the variable of its state directory, `HERDR_AGENT`, `environment`, and those of its grants. `HERDR_AGENT` names the kind, so herdr knows the agent.

It does not see the rest of your home, `/run/user`, `/run/agenix`, herdr's socket, the Nix daemon, the bot's files and queues, other agents' folders, or the processes of the host.

Each grant is one path that only this sandbox holds:

- `allow.network`, on by default, shares the host's network, which an agent needs to reach its model. Off, the agent has no network, not even `127.0.0.1`, but `allow.ports`.
- `allow.ports` gives an agent without `allow.network` a fixed set of TCP addresses: inside, `127.0.0.1:11434` reaches `10.23.0.18:11444`, and no other address exists.
  - Each port is a user socket `herdr-matrix-port-nora-11434.socket` on `$XDG_RUNTIME_DIR/herdr-matrix/ports/nora/11434`, served by `systemd-socket-proxyd`.
  - The command binds that directory read-only and starts a `socat` per port in the sandbox before the agent.
  - While the sockets are not up, the command stops with an error.
- `files` puts files in the state directory, read-only: the key is the path in the state directory, the value the file. Each start shows the file of the current generation. The mount point stays there as an empty file after the agent exits; the bot does not take it for a session file.
- `allow.chats.read` gives `herdr-matrix chats rooms`, `read` and `search`; `allow.chats.send` gives `rooms` and `send`. They go to the socket `/run/herdr-matrix-gate/<user>.nora/socket`, whose gate refuses other commands with `not allowed for this agent`. `allow.chats.rooms` limits the agent to these room ids; to the agent, other rooms do not exist. It needs `gate.enable`; the rules of the [chat gate](#chats-for-the-agents) apply first.
- `allow.sendFiles`, on by default, gives `herdr-matrix send`; you already get every text of the agent. The file goes to the queue `$XDG_CACHE_HOME/herdr-matrix/uploads/nora`, and the bot posts it as the agent working in `cwd`. The queue names the agent, never what the agent writes in it.

The bot reads the agent's answers from the newest session file in its state directory. It reads what the agent writes there and in its queue as untrusted: it follows no symlink and opens only regular files.

What is not enforced:

- An agent started without `herdr-matrix-agent-<id>`, for example `claude` or `pi` typed by hand in the folder, is not confined.
- An agent with `allow.network` shares the host's network namespace.
  - It reaches each service on the host's loopback addresses and each abstract unix socket. A service that trusts a local caller or your uid trusts the agent.
  - An X server that accepts local users by uid (Xwayland with `SI:localuser`) lets it read the clipboard and send input to X11 programs.
  - For an agent that needs only local services, set `allow.network = false` and name the ports in `allow.ports`.
- `environment` and `files` are in the Nix store: each user and each confined agent on the host can read them. Do not put a secret there.
- `!new` and a start by hand run your shell in the agent's folder before the command starts. A shell hook that runs code from the folder, such as direnv with an allowed `.envrc` or a prompt that runs `git` there, runs it outside the sandbox.
- `autostart` starts your shell in the agent's folder at each start of the bot, so a shell hook there runs without any action of yours.
- `!new` in a folder that holds a confined agent's folder starts an unconfined agent that can read what the confined agent wrote.
- Your other, unconfined agents run as your user, and can use the sockets and queues of a confined agent.
- The sandbox runs as your user. The namespaces are the boundary: no other mount, process or IPC view.
- herdr takes a pane's directory from what the pane's terminal reports. A confined agent can report the folder of another agent: its chat user then takes that agent's name and picture, and a prompt for that folder is dropped or reaches the wrong agent. It gains no file, socket or chat. For this reason the bot binds an own chat without your command only to a pane it started.
- What a confined agent writes in its folder is untrusted: do not run its Git hooks or scripts, and tell agents that read the folder so.

The agent in the sandbox starts with a state directory of its own that holds only `files`.

## Command line

```text
herdr-matrix                                run the bot, as its service does
herdr-matrix prompt <agent> <text>...       queue a prompt for an agent; dropped when the agent is working or waits on a choice, when the name matches no agent or several, or after 10 minutes
herdr-matrix prompt --cwd <dir> <text>...   the same, for the one agent working in <dir>
herdr-matrix send <file> [caption]...       post a file to the owner's chat, from a herdr pane
herdr-matrix chats rooms|read|search|send   the owner's bridged chats, through the gate
herdr-matrix gate                           answer one request of the gate, on stdin and stdout
herdr-matrix help                           this text
```

- `herdr-matrix help`, `-h` and `--help` print this text and exit with code 0.
- After `send` and `prompt`, only `-h` and `--help` print it: `herdr-matrix send help` posts a file named `help`. `chats --help` prints the usage of `chats`.
- `gate` takes no `--help`: the gate service runs it.
- Arguments that fit no line print the text on stderr and exit with code 2.
- The bot's own command without arguments does the same when `HERDR_MATRIX_HOMESERVER` and `HERDR_MATRIX_TOKEN` are not set.
- A failure is one line on stderr, `herdr-matrix: <error>`, with exit code 1.

## Troubleshooting

`journalctl --user -u herdr-matrix` shows errors as `error: <error>`, and one line per delivered prompt, voice message, file, key, answer, reaction and command: the pane, the kind, and the length of a text. It never shows the text. A prompt from the machine has the kind `local`; a dropped prompt shows why. Data from herdr or the homeserver that the bot cannot read shows as `unexpected data`. In the chat, a failure reads `Failed to <what>: <error>`.

| Symptom | Cause | Fix |
|---|---|---|
| The bot does not start | `tokenFile` or `environmentFile` is missing, or `homeserver` is plain `http://` to another host. The service retries every 10 s. | Make the file; use an `https://` URL. |
| The bot does not start; the journal names the state file | The state file cannot be read. | Fix the file's owner or mode. |
| Every message fails with `401` | The token in the secret file is not the `as_token` of the registration. | Correct the secret file, then `systemctl --user restart herdr-matrix`. |
| A token changed | The service reads the secret file at its start. | Update the secret file, then `systemctl --user restart herdr-matrix`. |
| The chat is lost or broken | | Invite `@herdr` to a new direct chat with only you; it becomes the chat. Or delete `control` from the state file and run `systemctl --user restart herdr-matrix`. |
| A new chat came, and `matrix-state.json.broken` exists | The state file was broken. The bot moved it, started afresh and made a new chat. | Nothing. |
| A new owner | | Set `owner`. The bot makes a new chat and invites the owner. |
| Voice messages are refused | The model did not download. | Read `journalctl --user -u herdr-matrix-models`, then run `systemctl --user start herdr-matrix-models`. |
| Files and voice messages fail | The files directory is not the bot user's own, mode `0700`, or it is a symlink. | Read the journal, then fix the directory. |
| `herdr-matrix chats` says `the homeserver answered 401 M_UNKNOWN_TOKEN` | The token in `gate.tokenFile` is wrong, or its session logged out. | Put a token of a live session in the file. |
| `herdr-matrix chats` fails otherwise | | Read `journalctl -u 'herdr-matrix-gate-*'`: one line per request, the command and `ok` or the error. |

Files:

- State: `~/.config/herdr/matrix-state.json` (in `$XDG_CONFIG_HOME/herdr`), mode `0600`.
- Whisper models: `~/.config/herdr/models`.
- Files of the chat and the queues of the machine: `$XDG_CACHE_HOME/herdr-matrix`, else `~/.cache/herdr-matrix`.
- The state directory of a confined agent: `$XDG_STATE_HOME/herdr-matrix/agents/<id>/<kind>`.

## Limitations

- The kinds of agent differ in what the chat shows: see [Agent kinds](#agent-kinds).
- The owner's Matrix login is the only guard: whoever holds it has a shell as the bot's user, and so does whoever holds the appservice token. The agents run as that user and can read the token.
- The bot uses only rooms of the owner and itself, joined by invite. When anyone else joins the main chat or is invited, it leaves and makes a new one; for an own chat, see [Own chats](#own-chats). It does not read encrypted rooms or files.
- herdr takes prompts as command-line arguments: on a host whose `/proc` lacks `hidepid`, other local users can read them.
- A one-word voice message such as "yes" answers a choice at once, a permission prompt too, and whisper may hear such a word in noise.
- Messages older than 10 minutes when they arrive, for example after downtime, are skipped.
- Files go in both directions up to the homeserver's upload limit (`m.upload.size`). The homeserver keeps its own copy of each file. The files on this machine are readable by your user; see [Files](#files).
- The bot works on one thread. While it moves a file, it answers no chat, up to 1 minute. While it hears a voice message, it answers no chat either: up to 1 minute to fetch the recording, then up to 2 minutes each for `ffmpeg` and `whisper-cli`. Pictures from the agent post without a thumbnail; the homeserver makes one.
- The last message of a turn comes 0.5 s to 1 s after herdr reports the agent idle. While a turn is under way, the bot polls herdr every 0.5 s; otherwise every 3 s, which is also the delay of an away notice and of a prompt from the machine. herdr's own idle detection and the homeserver add to it.
- The bot keeps the 5 s wait of files and the kept message of a chat without an agent in memory; a restart loses them. While the bot is down, a change of an agent's state is not reported, also with away on.
- An outage shows only in `journalctl --user -u herdr-matrix`.

## License

GPL-3.0-or-later
