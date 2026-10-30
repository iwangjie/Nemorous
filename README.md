<div align="center">

<a href="https://github.com/iwangjie/Nemorous"><img src="assets/icon.png" width="108" alt="Nemorous"></a>

# Nemorous

### Forest of Repositories, Unified Glance

**Ultra-fast, lightweight, and native Git multi-repository cockpit built for the AI coding era**<br>
Crafted with [MyGo Native UI](https://mygo.egoist.dev/) · 100% Native GPU rendering · Zero WebView/Electron overhead · `CGO=0` single-binary distribution

[![Release](https://img.shields.io/github/v/release/iwangjie/Nemorous?label=release&color=10B981)](https://github.com/iwangjie/Nemorous/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/iwangjie/Nemorous/ci.yml?branch=main&label=CI&color=10B981)](https://github.com/iwangjie/Nemorous/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-6366F1)](LICENSE)
[![Go Report Card](https://goreportcard.com/badge/github.com/iwangjie/Nemorous)](https://goreportcard.com/report/github.com/iwangjie/Nemorous)

![macOS](https://img.shields.io/badge/macOS-Metal-000000?logo=apple&logoColor=white)
![Windows](https://img.shields.io/badge/Windows-Direct3D_11-0078D4?logo=windows&logoColor=white)
![Linux](https://img.shields.io/badge/Linux-OpenGL-FCC624?logo=linux&logoColor=black)
![Architecture](https://img.shields.io/badge/Arch-arm64_%7C_amd64-059669)

<br>

**[Downloads](https://github.com/iwangjie/Nemorous/releases/latest)** &nbsp;·&nbsp; **[Features](#core-features)** &nbsp;·&nbsp; **[Safety Invariants](#safety-invariants)** &nbsp;·&nbsp; **[Shortcuts](#keyboard-shortcuts)** &nbsp;·&nbsp; **English** · [简体中文](README.zh-CN.md)

<br>

<picture>
  <img src="assets/banner.png" width="100%" alt="Nemorous Banner">
</picture>

<br>

<picture>
  <img src="assets/preview.gif" width="100%" alt="Nemorous Promo Preview">
</picture>

<sub>🎬 Watch the Full HD 60fps showcase video in [assets/promo.mp4](assets/promo.mp4)</sub>

</div>

<br>

## Why Nemorous?

In modern microservice architectures, business features and architectural refactors frequently span across multiple Git repositories (e.g., `cloud-caring`, `gateway`, `monorepo`, `risk-cloud`, `sby-libs`, `tax-pay-ext`, etc.).

As AI coding assistants and autonomous agents (Claude Code, Codex, Cursor, OpenCode, Pi) write code at machine speed, developers face a new bottleneck: **multi-repository coordination friction**.

| Traditional Git Pitfalls | AI Era Amplified Pain Points | Nemorous's Solution |
|---|---|---|
| **Branch Drift** | AI creates branches across multiple repos; developers forget to align branches before editing. | **`⌘B` 1-Click Multi-Repo Switch**: Atomically aligns all repos to the feature branch, auto-stashes unstaged changes, and recalls saved branch presets. |
| **Pull Conflicts & Skips** | `git pull` across multiple repos skips repositories with dirty working trees, breaking local builds. | **Auto Stash & Pop**: Silently stashes dirty files before pulling, fast-forwards clean commits, and seamlessly pops changes back. |
| **Fragmented Code Review** | Tabbing through dozens of IDE windows to inspect diffs loses cross-repo context. | **Unified Multi-Repo Changes Tree**: Hierarchical IDEA-style tree, side-by-side diffs, and inline micro-editing with `⌘S` save. |
| **High-Risk Merges** | Merging feature branches into staging/main pollutes the local working tree. | **Worktree-Isolated Direct Merge**: Merges execute inside ephemeral `git worktree` sandboxes, leaving your active tree 100% clean. |
| **Unreliable AI Conflict Fixes** | AI conflict resolution might hallucinate, edit unrelated files, or leave conflict markers. | **6-Layer Machine Verified Invariants (`VerifyAIMerge`)**: Boundary checking, parent SHA validation, and exhaustive 0-conflict-marker scans. |
| **Bloated Electron Apps** | Running multiple heavyweight Git GUIs eats gigabytes of RAM and drains battery life. | **Pure Native GPU Engine (Metal / D3D11 / OpenGL)**: Instant cold start (<0.3s), ~20MB RAM idle, zero web engine lag. |

---

## Core Features

### 1. Multi-Repo Branch Matrix & Batch Fast-Forward (`⌘B`)
- **Bird’s-Eye Branch Matrix**: Instantly view the checked-out branch, ahead/behind counters, and uncommitted change flags for all workspace repositories.
- **One-Click Synchronization**: Switch all or selected repositories to a target branch simultaneously with RE2 regex and fuzzy matching.
- **Branch Presets**: Save complex multi-repo branch combinations (e.g. `Billing-V2-Sprint`) and restore them with one click from the header.
- **Fail-Safe Guard**: Repositories with uncommitted modifications are never clobbered or unsafely overwritten.

<picture>
  <img src="assets/screenshots/04_branch_switch_light.png" width="100%" alt="Multi-Repo Branch Matrix">
</picture>

---

### 2. IDEA-Grade Changes Tree & Diff Micro-Editing (`⌘1`)
- **Hierarchical Directory Tree**: Clean, verified tree indentation (`Repo → Directory → File`), with single-child directory compaction (Compact Middle Packages).
- **Tri-State Checkboxes**: Check/uncheck by file, directory, or repository; Shift-click range selection; keyboard-driven flow (`Space` toggle, `S` stage, `U` unstage).
- **Unified & Split Diff View**: Crisp syntax highlighting for additions and deletions, with direct cursor micro-editing and `⌘S` instant disk sync.
- **Unified Multi-Repo Commit & Push**: Commit checked files across multiple repos in one atomic operation, with `--amend` and safe soft-undo support.

<picture>
  <img src="assets/screenshots/01_changes_diff_light.png" width="100%" alt="Changes Tree and Diff">
</picture>

---

### 3. Zero-Index Millisecond Code Explorer & Git Blame (`⌘2`)
- **Zero-Index Instant Launch**: Browse multi-million-line workspaces without waiting for indexing.
- **Fuzzy File Finder (`⌘P` / `⌘K`)**: Millisecond file lookup across all workspace repositories.
- **30+ Language Syntax Highlighting**: GPU-accelerated code rendering with buttery smooth scrolling.
- **IntelliJ-Grade Git Blame**: File-wide date and author annotations with commit recency heat-map coloring; single-click line annotation to drill straight into commit graph details.

<picture>
  <img src="assets/screenshots/02_code_explorer_light.png" width="100%" alt="Code Explorer & Git Blame">
</picture>

---

### 4. Global Commit Forest & Colorful Swimlanes (`⌘3`)
- **Topological Commit Forest**: Combines commit histories across all repositories into a unified timeline graph.
- **Multi-Repo Swimlanes**: Displays up to 10 active parallel swimlanes with intelligent overflow folding to prevent tangled visual noise.
- **Power Filters**: Search commits by message, SHA, repository, branch, author ("My commits" prioritized), file path, or Ripgrep glob.

<picture>
  <img src="assets/screenshots/03_commit_forest_light.png" width="100%" alt="Global Commit Forest">
</picture>

---

### 5. Worktree-Isolated Merge & AI Conflict Resolution
- **Zero Working-Tree Dirtying**: Merges execute inside an isolated ephemeral `git worktree`. Your active working directory remains completely untouched.
- **Fast-Forward & Direct Merge**: Clean merges automatically advance the target branch and optionally push to origin.
- **AI Conflict Resolution**: Connect your local AI Agent CLI (`pi`, `claude`, `codex`, `omp`) to automatically resolve merge conflicts.
- **6-Layer Machine Verified Security (`VerifyAIMerge`)**: AI merge results must strictly pass all 6 automated invariant checks before committing.

---

### 6. Native `mani.yaml` & `AGENTS.md` Integration (`⌘4`)
- **Mani Tool Window**: Automatically discovers workspace task pipelines and projects from `mani.yaml`. Run commands workspace-wide or target specific repos.
- **AI Agent Guidance**: Auto-discovers root and per-repo `AGENTS.md` definitions and guidelines.

---

### 7. Embedded PTY Ghostty Terminal & Global Command Palette
- **Embedded Terminal**: Press `⌘3` / click terminal rail to launch a full-featured native PTY terminal simulator.
- **Global Search in Files (`⇧⌘F`)**: Fast multi-threaded search across all repositories, powered by Ripgrep (`rg`) with automatic fallback to built-in scan.

---

## Safety Invariants

Automation and AI must never compromise code integrity. Nemorous enforces mathematically rigorous safety invariants:

```
                    ┌────────────────────────────┐
                    │     Pre-Merge Validation   │
                    │ (Target branch check / no  │
                    │  illegal dirty checkout)   │
                    └─────────────┬──────────────┘
                                  │
                                  ▼
                    ┌────────────────────────────┐
                    │  Isolated Worktree Sandbox │  ◄── Zero touch on working tree
                    └─────────────┬──────────────┘
                                  │
                    ┌─────────────┴──────────────┐
                    │                            │
             [Clean Fast-Forward]         [Merge Conflict]
                    │                            │
                    │                   AI Assistant Resolution (CLI)
                    │                            │
                    │                            ▼
                    │            ┌───────────────────────────────┐
                    │            │   6-Layer Machine Verification│
                    │            │    VerifyAIMerge Invariants   │
                    │            ├───────────────────────────────┤
                    │            │ 1. MERGE_HEAD clean commit    │
                    │            │ 2. git diff --diff-filter=U=0 │
                    │            │ 3. Working tree 100% clean    │
                    │            │ 4. Exact dual-parent matching │
                    │            │ 5. Strict boundary file check │
                    │            │ 6. 0 remaining <<<<<<< markers│
                    │            └───────────────┬───────────────┘
                    │                            │ (All passed)
                    ▼                            ▼
            ┌────────────────────────────────────────────┐
            │        Human-Confirmed Safe Push           │
            │  (Strictly forbids destructive --force)    │
            └────────────────────────────────────────────┘
```

1. **Working Tree Invariant**: Batch branch switching, switching to main, and `pull --ff-only` safely skip repos with uncommitted changes or auto-stash them; single-file discard requires explicit confirmation.
2. **Never Force Push**: Any push command categorically bans `--force` and `--force-with-lease`.
3. **Bounded Undo**: `git reset --soft` is only permitted for commits that have not been pushed to remote.
4. **AI Merge 6-Layer Machine Verification (`VerifyAIMerge`)**:
   - **Check 1**: `MERGE_HEAD` must be finalized with a valid merge commit;
   - **Check 2**: `git diff --diff-filter=U` must report exactly zero unresolved files;
   - **Check 3**: `git status --porcelain` in the worktree must be completely clean;
   - **Check 4**: `HEAD` must be a dual-parent commit matching `[preTargetSHA, sourceTipSHA]`;
   - **Check 5**: Changed files must strictly be a subset of the union of both branches' modified sets—AI is forbidden from modifying arbitrary files;
   - **Check 6**: Full content scan guarantees 0 leftover `<<<<<<<`, `=======`, `>>>>>>>`, or `|||||||` conflict markers.

---

## Keyboard Shortcuts

| Shortcut | View / Action | Description |
|---|---|---|
| `⌘1` | **Changes & Staging** | Multi-repo hierarchical changes tree, diff view, micro-edit, staging & commit |
| `⌘2` | **Code Explorer** | Zero-index file viewer, 30+ language highlights, Git blame annotations |
| `⌘3` | **Git Forest / Terminal** | Unified commit swimlane graph, branch manager, worktree merge |
| `⌘4` | **Mani Panel** | `mani.yaml` project and task runner (auto-hides when not configured) |
| `⌘K` / `⌘P` | **Command Palette** | Instant search across files, repos, branch actions, and Mani tasks |
| `⇧⌘F` | **Find in Files** | Cross-repo full-text search with Ripgrep (`rg`) acceleration |
| `⌘B` | **Branch Switcher** | Multi-repo branch matrix, create branch, manage branch presets |
| `⌘⏎` | **Commit** | Commit checked files across all selected repositories (supports `--amend`) |
| `⌘⇧⏎` | **Commit & Push** | Unified commit and safe push to remote tracking branches |
| `↑` / `↓` | **Navigate Changes** | Move selection in the changes tree when no text field has focus |
| `Space` | **Toggle Checkbox** | Toggle check/uncheck state of selected file or directory |
| `S` / `U` | **Stage / Unstage** | Single-key stage (`S`) or unstage (`U`) for the active change item |
| `⌘S` | **Save Micro-Edit** | Persist inline edits in diff or code viewer directly to disk |
| `⌘R` | **Refresh** | Rescan and update status across all Git repositories |
| `⌘,` | **Settings** | Configure workspace path, themes, AI Agent CLI, update check frequency |

---

## Installation & Downloads

Pre-built binaries for macOS, Windows, and Linux are available under [GitHub Releases](https://github.com/iwangjie/Nemorous/releases/latest):

| Platform | Format | Architecture | Notes |
|---|---|---|---|
| **macOS** | `.dmg` | Apple Silicon (`arm64`) & Intel (`amd64`) | Hardware-accelerated Metal GPU rendering |
| **Windows** | `-setup.exe` / `.tar.gz` | `amd64` / `arm64` | Direct3D 11 native graphics, NSIS installer |
| **Linux** | `.deb` / `.tar.gz` | `amd64` / `arm64` | Native OpenGL pipeline, tested on Ubuntu / Debian |

### Signed Auto-Updates
- **Ed25519 Cryptographic Verification**: Every release manifest and archive is signed with private keys; the desktop client validates signatures using built-in public keys.
- **Smart Background Updates**: Periodically checks for updates in the foreground (interval configurable from 5m to 24h); automatically installs and restarts when the app is in the background and no dirty edits or active tasks are running.
- **Fast Mirror Acceleration**: Includes automatic mirror fallback (`ghfast.top`) for high-speed downloads worldwide, gracefully falling back to official GitHub Releases.

---

## Building from Source

Nemorous is written in Go and uses [MyGo Native UI](https://mygo.egoist.dev/). No CGO required; builds clean static binaries:

```bash
# 1. Clone repository
git clone https://github.com/iwangjie/Nemorous.git
cd Nemorous

# 2. Static lifetime vet
go tool mygo vet .

# 3. Run all unit and headless UI regression tests
CGO_ENABLED=0 go test -v ./...

# 4. Build native package for current OS (.app / .dmg / .exe / .deb)
go tool mygo build
```

---

## License

Nemorous is open source under the [MIT License](LICENSE). Contributions, bug reports, and pull requests are warmly welcome!

<div align="center">
  <sub>Built with ❤️ by the Nemorous Open Source Team · Forest of Repositories, Unified Glance</sub>
</div>
