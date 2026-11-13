# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0]

### Added

- A Matrix chat attached to one herdr agent at a time: prompts, pictures, files and answers to its choices go to it, and its answers come back as it works.
- Away notices: after `!away`, every agent that stops or waits on a choice posts in the chat, until `!here`.
- An agent's own chat (`!dm`), with an appservice user of its own.
- Voice messages, transcribed by whisper.cpp.
- The Home Manager module `programs.herdr.matrix`, and the user service `herdr-matrix-models` that downloads the whisper model.
