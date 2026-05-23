# Changelog

All notable changes to this project will be documented here.

The format roughly follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] — initial release

### Added

- **Encrypted Discord-backed storage**
  - 9 MiB fixed-size chunking that fits under Discord's 10 MB attachment cap
  - AES-256-GCM per-chunk encryption with random nonces
  - SHA-256 plaintext integrity verification on every read
  - Hash-prefix routing across N data channels (default 4)
  - Master key generated at init, never leaves the local config

- **Storage layer**
  - In-Discord inode tree: root pointer message + directory inodes + file inodes
  - File inodes stored inline (small) or as attached `inode.json` (large)
  - Mutable directories via Discord's bot-can-edit-own-messages semantics
  - File-based local cache for file inodes (immutable) and decrypted chunks
  - 4-way parallel chunk uploads (`futures::stream::buffer_unordered`)
  - 4-way parallel chunk downloads with order preserved (`buffered`)
  - Streaming `tokio::fs` reads — RAM bounded regardless of file size
  - Range-aware chunk fetches — seeking 30% into a 1 GB file downloads
    only the one chunk that covers that offset

- **Filesystem**
  - WinFSP-backed `Z:\` drive on Windows 10/11
  - Read, write, delete, create, mkdir, rmdir, rename (incl. cross-dir move)
  - Per-file temporary buffer commits to Discord on `cleanup()` callback
  - Case-preserving, case-sensitive file system (matches POSIX semantics)
  - Volume-info reports 1 TiB total/free (we don't have a real metric)

- **Two binaries**
  - `oubliette.exe` — CLI with subcommands: `setup`, `init`, `mount`,
    `put`, `get`, `ls`, `mkdir`, `info`
  - `oubliette-gui.exe` — egui-based windowed app with
    - Multi-step setup wizard (WinFSP check → bot creation → token →
      server ID → channel creation)
    - System tray icon with Show/Mount/Unmount/Open/Quit menu
    - Hide-on-close (mount survives window close)
    - Drag-and-drop upload zone with per-file progress bars
    - Stats panel showing root files, total bytes, cache stats
    - Run-setup-again to re-init

- **Distribution**
  - PowerShell installer (`installer/install.ps1`) with:
    - WinFSP detection (prompts to install if missing)
    - Copies binaries to `%LOCALAPPDATA%\Oubliette`
    - Creates Start Menu shortcut
    - Optionally creates Desktop shortcut
    - Generates uninstaller
  - `Mount Oubliette.bat` + `Setup Oubliette.bat` for double-click use
  - Helper batch files refreshed automatically on every wizard run

- **Documentation**
  - `README.md` with animated SVGs and Mermaid diagrams
  - `docs/ARCHITECTURE.md` — module layout, threading model,
    storage layer, lifecycle of a write
  - `docs/INTERNALS.md` — chunking math, cipher choice, sharding,
    cache strategy, ToS-adjacent design notes
  - `CONTRIBUTING.md` — how to contribute, code style, testing
    suggestions
  - Educational Research License (MIT-derived with non-commercial
    and educational-intent clauses)

### Known limitations

- Root directory caps at ~50 entries (single mutable message)
- Read-modify-write of existing files zeros unwritten regions
- No rate-limit backoff on Discord 429 responses
- No atomic recovery from mid-upload crashes (orphaned chunks)
- No deduplication
- Single-user, single-process semantics (no CAS on root pointer)
- Windows-only (Linux/macOS via `fuser` is on the roadmap)

[0.1.0]: https://github.com/Nuu-maan/oubliette/releases/tag/v0.1.0
