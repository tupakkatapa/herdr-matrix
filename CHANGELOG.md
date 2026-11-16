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
- The chat gate: the NixOS module `services.herdr-matrix-gate` and `herdr-matrix chats rooms|read|search|send`, with which agents read, search and send in the owner's bridged chats. Only the gate, under its own system user, reads the owner's token; it refuses every room of the bot.

### Changed

- The binary refuses unknown arguments with a usage message and exit code 2, and does not start the bot.

## [0.1.0]

### Added

- A Matrix chat attached to one herdr agent at a time: prompts, pictures, files and answers to its choices go to it, and its answers come back as it works.
- Away notices: after `!away`, every agent that stops or waits on a choice posts in the chat, until `!here`.
- An agent's own chat (`!dm`), with an appservice user of its own.
- Voice messages, transcribed by whisper.cpp.
- The Home Manager module `programs.herdr.matrix`, and the user service `herdr-matrix-models` that downloads the whisper model.
