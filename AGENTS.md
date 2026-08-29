# AGENT.md — Guidelines for AI Coding Assistants

This repository contains **`git-janitor` (`git-jan`)**, a submission for the **Zero Dependency Hackathon** built in **Rust** using an isolated **Podman + Nix** container environment[cite: 1, 2, 3, 5, 6].

Any automated agent or LLM working on this codebase **must** strictly adhere to the following guardrails[cite: 6].

---

## 1. The Zero-Dependency Hard Rule

* **`Cargo.toml` `[dependencies]` MUST REMAIN 100% EMPTY[cite: 1, 6].**
* **Never suggest, add, or invoke third-party crates** (e.g., no `clap`, `structopt`, `serde`, `serde_json`, `regex`, `tempfile`, `colored`, `walkdir`, `git2`, `lazy_static`, `anyhow`, or `thiserror`)[cite: 6].
* **Standard Library Only (`std::*`)**:
  * CLI Parsing: Use `std::env::args()`[cite: 5, 6].
  * Collections & Concurrency: Use `std::collections::*` and `std::sync::*`[cite: 6].
  * Filesystem & Path: Use `std::fs::*` and `std::path::*`[cite: 5].
  * Static Initialization: Use `std::sync::LazyLock` (Rust 1.80+)[cite: 6].
  * Output Formatting: Write manual JSON formatters and raw ANSI escape sequences[cite: 5, 6].
* **No Vendored Code**: Do not copy external library source code into `src/`[cite: 6].

---

## 2. No Shelling Out (Hackathon Out-of-Scope Rule)

* **NEVER call `std::process::Command::new("git")`** or invoke any external binary runtime dependencies[cite: 6].
* Git operations must be achieved by:
  1. Parsing filesystem data directly inside `.git/` (e.g., `.git/HEAD`, `.git/refs/heads/`, `.git/config`, `.git/packed-refs`) using `std::fs`[cite: 6].
  2. Parsing Git output passed directly into `stdin`[cite: 6].

---

## 3. Environment & Build Tooling Constraints

* **Do not execute or recommend host-level `cargo` or `rustc` commands.** The host machine does not have Rust or Nix installed.
* All testing and compilation occurs inside Podman via Nix[cite: 2, 3].
* Use the following commands when running tests or verifying the build:
  * **Run Tests:** `make test` (or `podman run --rm git-janitor nix flake check`)[cite: 3].
  * **Build Release Artifact:** `make build`[cite: 2, 3].
  * **Extract Binary to Host:** `make extract-binary` (writes to `target/bin/git-janitor`).
* **Nix Flake Rule**: Nix ignores untracked files[cite: 3]. Any new file must be added to Git (`git add <file>`) before running `nix build` or `nix flake check`[cite: 3].

---

## 4. Code Architecture & Bonus Alignment

* **Target Language Version**: Rust `1.98.0` (edition `2024`)[cite: 1, 6].
* **Target Bonuses**:
  * **Single File (+5)**: Keep the complete implementation inside `src/main.rs` if targeting this bonus[cite: 1, 6]. Keep tests inline via `#[cfg(test)]`[cite: 6].
  * **Reproducible Build (+5)**: Preserve deterministic compiler flags in `Cargo.toml` (`codegen-units = 1`, `lto = true`, `panic = "abort"`, `strip = true`) and path remapping in `flake.nix` (`--remap-path-prefix`)[cite: 1, 3, 6].
  * **STDLIB Log & Package Killer (+3 each)**: Maintain transparent documentation of all stdlib-for-crate substitutions in `STDLIB.md`[cite: 6].

---

## 5. Exit Code & Safety Contracts

* `0`: Success / no secrets found / clean[cite: 5].
* `1`: Potential secrets detected / blocked[cite: 5].
* `2`: Scanner, configuration, or operational error[cite: 5].
* **Safety Principle**: Never perform destructive branch operations silently; dry-run by default, requiring `--apply` for execution[cite: 5]. Never print complete detected secret tokens to stdout or logs[cite: 5].