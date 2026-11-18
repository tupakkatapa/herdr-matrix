# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Silent turns: a turn whose last message is `NOOP` posts nothing, also as an away notice.
- `herdr-matrix prompt <agent> <text>`: queues a prompt that the running bot hands to the agent; the last message of its turn posts in the agent's own chat, else in the main chat.
- The option `agents.<id>`: for the agent working in `cwd`, the `name`, `prefix` and `avatar` of its own chat user, and `prompts.<name>`, a prompt on a systemd calendar schedule (`herdr-matrix prompt --cwd <dir> <text>`).
- The option `spawnPrefix`: off, the user of an agent's own chat has the agent's name alone. The name follows the agent when it changes.
- `herdr-matrix send <file> [caption]...`: in an agent's pane, posts a file in the agent's own chat, else in the main chat. Pictures show inline, audio and video as players. The package is in `home.packages`.
- Videos from the owner reach the agent as files.
- The chat gate: the option `programs.herdr.matrix.gate` (`enable`, `tokenFile`, `bridges`, `bot`), run by the NixOS module `nixosModules.default`, and `herdr-matrix chats rooms|read|search|send`, with which agents read, search and send in the owner's bridged chats. Only the gate, under its own system user, reads the owner's token; it refuses every room of the bot.
- Confined agents: the option `agents.<id>.sandbox` (`enable`, `program`, `environment`, and `allow.readPaths`, `allow.network`, `allow.chats.read`, `allow.chats.send`, `allow.chats.rooms`, `allow.sendFiles`) makes the command `herdr-matrix-agent-<id>`, which runs the agent in a bubblewrap sandbox with only its folder, its own state directory and what `allow` grants. Each grant is a path of its own: a gate socket per agent with its commands and rooms, and a queue of `herdr-matrix send` that names the agent. `!new` starts a confined agent through its command.
- pi beside Claude Code: the bot takes each agent's kind from herdr and posts pi's answers from its session files, also from a confined agent. The option `agents.<id>.kind` (`"claude"` or `"pi"`) sets what `!new` starts and the default of `sandbox.program`; a confined pi agent keeps its state in `agents/<id>/pi`, as `PI_CODING_AGENT_DIR`. A confined agent gets `HERDR_AGENT=<kind>`, so herdr knows it. An agent of another kind posts its screen.
- The option `agents.<id>.sandbox.files`: files in a confined agent's state directory, for example pi's `models.json`, bound read-only at each start.
- The option `agents.<id>.sandbox.allow.ports`: for a confined agent without a network, `127.0.0.1:<port>` in the sandbox reaches a `host:port` outside, through a user socket `herdr-matrix-port-<id>-<port>` and `systemd-socket-proxyd`.

### Changed

- **Breaking:** the NixOS option `services.herdr-matrix-gate` is removed; the gate is set in Home Manager. Migration: move `enable`, `tokenFile`, `bridges` and `bot` to `programs.herdr.matrix.gate` of the user that was in `users`. `homeserver` and `package` come from `programs.herdr.matrix`. Keep the import of `nixosModules.default`.
- The binary refuses unknown arguments with a usage message and exit code 2, and does not start the bot.
- Files of the owner wait 5 s for more for the same agent. Files that come within 5 s of each other go as one prompt: the captions, then one path a line. A text or voice message hands the waiting files over first.
- The size limit of files is the homeserver's `m.upload.size`, in both directions; the limit of 25 MB is removed. Files move between the network and the disk in pieces.
- Files of the chat and the queues of `herdr-matrix prompt` and `send` move from `$XDG_RUNTIME_DIR/herdr-matrix` to `$XDG_CACHE_HOME/herdr-matrix`, else `~/.cache/herdr-matrix`, and stay 30 days, not 24 h. `$HERDR_MATRIX_STATE/files` is no longer a fallback.

## [0.1.0]

### Added

- A Matrix chat attached to one herdr agent at a time: prompts, pictures, files and answers to its choices go to it, and its answers come back as it works.
- Away notices: after `!away`, every agent that stops or waits on a choice posts in the chat, until `!here`.
- An agent's own chat (`!dm`), with an appservice user of its own.
- Voice messages, transcribed by whisper.cpp.
- The Home Manager module `programs.herdr.matrix`, and the user service `herdr-matrix-models` that downloads the whisper model.
