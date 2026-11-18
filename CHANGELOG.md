# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Silent turns: a turn whose last message is `NOOP` posts nothing, also as an away notice.
- `herdr-matrix prompt <agent> <text>`: queues a prompt that the running bot hands to the agent; the last message of its turn posts in the agent's own chat, else in the main chat.
- The option `agents.<id>`: for the agent working in `cwd`, the `displayName` (the display name of its own chat user), `prefix` and `avatar`, and `prompts.<name>`, a prompt on a systemd calendar schedule (`herdr-matrix prompt --cwd <dir> <text>`).
- The option `ownChat.prefix`: off, the user of an agent's own chat has the agent's name alone. The name follows the agent when it changes.
- `herdr-matrix send <file> [caption]...`: in an agent's pane, posts a file in the agent's own chat, else in the main chat. Pictures show inline, audio and video as players. The package is in `home.packages`.
- Videos from the owner reach the agent as files.
- The chat gate: the option `programs.herdr.matrix.gate` (`enable`, `tokenFile`, `bridges`, `bot`), run by the NixOS module `nixosModules.default`, and `herdr-matrix chats rooms|read|search|send`, with which agents read, search and send in the owner's bridged chats. Only the gate, under its own system user, reads the owner's token; it refuses every room of the bot.
- Confined agents: the option `agents.<id>.sandbox` (`enable`, `program`, `environment`, and `allow.readPaths`, `allow.network`, `allow.chats.read`, `allow.chats.send`, `allow.chats.rooms`, `allow.sendFiles`) makes the command `herdr-matrix-agent-<id>`, which runs the agent in a bubblewrap sandbox with only its folder, its own state directory and what `allow` grants. Each grant is a path of its own: a gate socket per agent with its commands and rooms, and a queue of `herdr-matrix send` that names the agent. `!new` starts a confined agent through its command.
- pi beside Claude Code: the bot takes each agent's kind from herdr and posts pi's answers from its session files, also from a confined agent. The option `agents.<id>.kind` (`"claude"` or `"pi"`) sets what `!new` starts and the default of `sandbox.program`; a confined pi agent keeps its state in `agents/<id>/pi`, as `PI_CODING_AGENT_DIR`. A confined agent gets `HERDR_AGENT=<kind>`, so herdr knows it. An agent of another kind posts its screen.
- The option `agents.<id>.sandbox.files`: files in a confined agent's state directory, for example pi's `models.json`, bound read-only at each start.
- The option `agents.<id>.sandbox.allow.ports`: for a confined agent without a network, `127.0.0.1:<port>` in the sandbox reaches a `host:port` outside, through a user socket `herdr-matrix-port-<id>-<port>` and `systemd-socket-proxyd`.
- The option `tokenFile`: the file with the appservice token, passed to the bot as a systemd credential. Set it or `environmentFile`, not both.
- The option `workspace`: the folder whose folders `!new` lists and starts agents in. The default is `~/Workspace`.
- The option `ownChat.avatar`: the picture of the user of an agent's own chat.
- The option `agents.<id>.chat`: the bot opens the agent's own chat when it starts the agent (`!new`, `autostart`). After `!leave`, the chat stays closed until `!dm`.
- The option `agents.<id>.autostart`: the bot starts the agent once when it comes up and the agent is not running, as `!new` does.
- Evaluation checks with a message that names the option and the fix: herdr's module missing or disabled, `homeserver`, `owner`, `tokenFile` and `environmentFile`, `cwd`, unit names, and the settings of a confined agent (`sandbox.files` keys, `allow.ports`, `allow.chats`). Two agents with one `cwd`, and a confined agent with no network and no `allow.ports`, give a warning.
- `!new` and a start through `autostart` follow the pane until the agent is ready. The bot says `<name> is ready.` quietly when the agent is idle, after its first choice if it starts on one, and says when the pane stops while it starts.
- With no agent attached, the bot keeps what you send, files and texts together as one bundle, for 2 minutes after the last item. It lists the agents once, answers later items quietly with `Kept with the rest; !attach an agent and I send it.`, and sends the bundle after `!attach` or when a `!new` agent is ready. When an agent started with `!new` still waits on a choice after the 2 minutes, the bot says `**<name>** still waits on a choice; your message was not sent.`
- Away is shown: the name of the main chat ends in `· away`, and `!agents` starts with `Agents (away is on):`.
- `!agents` numbers the agents, and `!attach`, `!dm` and `!leave` take the number from the last list. A number that is not in it answers `No agent 2 in the last list.` and shows the list. A reply to a message of an agent names it for these commands.
- The bot says when the attached agent ends, and detaches. A message to it meanwhile is not sent, and the bot says so.
- A turn that ends without text says `Done.`, except a turn that came from `herdr-matrix prompt`, which posts nothing. A long last answer that already went out as progress is pointed to, not posted twice.
- A choice on the screen takes typed `yes`, `no`, `one` to `nine`, `enter` and `cancel` as keys; capitals, spaces and a closing punctuation mark do not matter.
- An unknown command gets the nearest command as a hint. Slips such as `!stop.`, `! stop` and `!Stop` work.
- `!help`, the first message of the main chat and the first message of an own chat list the commands that work there.
- `herdr-matrix help` and `herdr-matrix --help` print the usage with a line on each command. After `send` and `prompt`, only `-h` and `--help` print it; `herdr-matrix send help` posts a file named `help`. `chats --help` prints the usage of `chats`.
- The journal shows when the bot starts (contacts, attached agent, away), why an own chat ended (`chat ended: <reason>`), when it moved to a new pane (`chat moved to`) and when a failed step works again (`ok again`).

### Changed

- **Breaking:** the NixOS option `services.herdr-matrix-gate` is removed; the gate is set in Home Manager. Migration: move `enable`, `tokenFile`, `bridges` and `bot` to `programs.herdr.matrix.gate` of the user that was in `users`. `homeserver` and `package` come from `programs.herdr.matrix`. Keep the import of `nixosModules.default`.
- **Breaking:** `herdr-matrix` refuses unknown arguments with the usage and exit code 2, and does not start the bot. Without arguments, it does the same when `HERDR_MATRIX_HOMESERVER` and `HERDR_MATRIX_TOKEN` are not set.
- **Breaking:** `!exit now` works only after `!exit` for the same agent, within 2 minutes. Else the bot asks again.
- **Breaking:** the bot transcribes only an audio message that the client marks as a voice message. An audio file without the voice flag is a file for the agent.
- **Breaking:** spoken `new` and `attach` take one word at most, as in "new beta"; "new tests, please" is a prompt. `auto` is not a spoken command.
- **Breaking:** an agent's own chat ends when another user joins it or is invited, as the main chat does.
- **Breaking:** evaluation fails for a relative `cwd`, a confined `cwd` with a `.` or `..` part, an `owner` that is not `@local:server`, and a `homeserver` that is not `https://` (or `http://` to the same machine).
- The old names `spawnAvatar`, `spawnPrefix` and `agents.<id>.name` still work, with a warning. They are `ownChat.avatar`, `ownChat.prefix` and `agents.<id>.displayName`.
- The last message of a turn arrives within about 1 s after herdr reports the agent idle; before, up to 6 s. While a turn is under way, the bot polls herdr every 0.5 s.
- The plain text of a message, which notifications show, has no Markdown marks.
- `!screen abc` is an error, not a screen.
- The chat of an agent with settings in `agents` stays while the agent is in the configuration, also while the agent is not running. It follows the agent to a new pane when the bot starts the agent (`!new`, `autostart`); after a start by hand, `!dm <agent>` continues the same chat. It ends at the first start of the bot after you remove the settings.
- When the owner changes, the own chats of the old owner end.
- `herdr-matrix` errors start with `herdr-matrix:`. `herdr-matrix send` outside a pane says that `HERDR_PANE_ID` is not set.
- Files of the owner wait 5 s for more for the same agent. Files that come within 5 s of each other go as one prompt: the captions, then one path a line. A text or voice message hands the waiting files over first.
- The size limit of files is the homeserver's `m.upload.size`, in both directions; the limit of 25 MB is removed. Files move between the network and the disk in pieces.
- Files of the chat and the queues of `herdr-matrix prompt` and `send` move from `$XDG_RUNTIME_DIR/herdr-matrix` to `$XDG_CACHE_HOME/herdr-matrix`, else `~/.cache/herdr-matrix`, and stay 30 days, not 24 h. `$HERDR_MATRIX_STATE/files` is no longer a fallback.

### Fixed

- The bot takes nothing from the queues of `herdr-matrix prompt` and `send` while herdr cannot list the agents.
- A prompt that herdr fails to take goes back to the queue, and the bot drops it after 10 minutes.
- A file that the homeserver fails to take (no connection, or a 5xx answer) is tried 3 times, one minute apart, within 10 minutes. Then one `Could not send <name>: …` notice says why.
- An unreadable entry in a queue is removed and logged once.
- A reply to a message of an agent that is gone goes nowhere, not to the attached agent. `!exit now` as a reply to another agent's message leaves the attached agent alone.
- With away on, a restart of the bot does not post the waiting choices again.
- The main chat gets the name of the attached agent again after a restart.
- A herdr failure does not slow the chat. A clock step back does not stall the polls. A herdr call whose child keeps its pipes open does not hang the bot.
- A reaction in an agent's own chat keeps the rest of the turn there. A turn started at the desk after a turn without text stays at the desk.
- A status that herdr reports that is not `working`, `blocked` or `idle` does not end a turn early.
- A second pane in the folder of an agent shows its own screen, not the other agent's text.
- `!leave` and `!help` work in an own chat while its agent is missing. `!new` finds a running agent whose folder ends in `/`.
- A failure of herdr shows herdr's own message in the chat.
- Markdown: bold inside a code span, the language of an empty code block and the depth of a nested list show right.
- The summary that Claude Code writes when it compacts a session does not start a turn.
- Two equal parts of one long message both arrive.
- The gate counts the events it walks, so a room of encrypted events is not walked to its end: 5000 at most.
- A failing profile or own chat step is tried again at growing intervals, not every 0.5 s.
- Files of the chat are removed after 30 days within the hour, not only when a new file comes.

### Security

- `!new` cannot start an agent without the sandbox in the folder of a confined agent, or in a folder below it, or behind a symlink that the agent made.
- A named pipe in a directory that an agent writes cannot hang the bot.
- The upload queue of a confined agent takes five files at a time, and cuts a file name at 255 bytes.
- The gate checks its room list before it asks the homeserver about a room.
- A confined agent's `cwd` with a `.` or `..` part is refused.
- The bot does not read the sessions of a confined agent behind a symlink.
- A Markdown link whose text is not its address shows the host after it: `label (host)`.
- `ffmpeg` opens local files only, so a recording cannot make it open an address.
- `allow.network = true` shares the host's network namespace: a confined agent reaches loopback services and an X server that trusts local users. Use `allow.network = false` with `allow.ports` for local services.

## [0.1.0]

### Added

- A Matrix chat attached to one herdr agent at a time: prompts, pictures, files and answers to its choices go to it, and its answers come back as it works.
- Away notices: after `!away`, every agent that stops or waits on a choice posts in the chat, until `!here`.
- An agent's own chat (`!dm`), with an appservice user of its own.
- Voice messages, transcribed by whisper.cpp.
- The Home Manager module `programs.herdr.matrix`, and the user service `herdr-matrix-models` that downloads the whisper model.
