# git-janitor

**`git-jan`** — a zero-dependency git housekeeping and secret-scanner written entirely in Rust.

`git-janitor` opens `.git/` directories directly and does **not** shell out to `git`,
uses **zero third-party crates** (standard library only), and is built **reproducibly**
via a pinned **Nix** flake inside an isolated **Podman** container.

---

## Features

### Secrets scanning (`secrets scan`)

Detect leaked credentials before they reach a remote:

- **High-entropy detection** — Shannon entropy scoring with configurable threshold
  (`ENTROPY_THRESHOLD_LOW = 3.8`, minimum token length 12).
- **Built-in pattern detectors** — AWS access keys, GitHub tokens, private keys,
  JWTs, Slack tokens, generic API keys, Google credentials, Bearer auth headers,
  and `key=value` assignments for common secret names (`password`, `token`,
  `secret`, `api_key`, `client_secret`, etc.).
- **`.leakignore` support** — glob-based ignore rules (`*`, `**`, `?`) with
  directory-prefix matching, anchors, and negation, layered over `leak_ignore_paths`
  from git config.
- **Multiple scan targets**:
  - worktree (all files, default)
  - `--staged` (files in the index)
  - `--since <ref>` (files changed since a ref)
  - `--commit <ref>` (a single commit's tree)
- **Redacted output** — full secret values are **never** printed; only a truncated
  preview is shown.
- **Machine-readable output** — `--format json` emits a valid JSON document,
  hand-serialized with proper string escaping (no JSON crate).

### Git housekeeping

- **`branch list`** — list local branches with their merged/upstream status.
- **`branch clean`** — delete merged local branches that have a tracking upstream.
  **Dry-run by default; pass `--apply` to actually delete.**
- **`install-hook`** — install a pre-commit hook that runs `git-jan secrets scan --staged`.
- **`doctor`** — run repository diagnostics (git dir, config, HEAD, refs, index,
  object integrity, leakignore validity).

---

## Exit codes

| Code | Meaning |
|------|---------|
| `0`  | Success / no secrets found / clean |
| `1`  | Potential secret(s) detected / blocked |
| `2`  | Scanner, configuration, or operational error |

Secrets scanning is **fail-block**: it exits `1` when secrets are found so it can be
used in hooks and CI/pre-commit pipelines. Destructive branch operations never run
silently — dry-run by default, requiring `--apply` to act.

---

## Usage

```
git-jan <COMMAND> [OPTIONS]

COMMANDS:
    branch list [--base <branch>]
        List local branches and their merged/upstream status.

    branch clean [--base <branch>] [--apply]
        Delete merged local branches with tracking upstream (dry-run by default).

    secrets scan [--staged] [--since <ref>] [--commit <ref>] [--format human|json]
        Scan repository for secret tokens.

    install-hook [--force]
        Install git pre-commit hook into .git/hooks/pre-commit.

    doctor
        Run repository diagnostics.

    help, --help
        Print this help message.

    version, --version
        Print version information.
```

### Examples

```bash
# Scan the whole worktree (human-friendly output)
git-jan secrets scan

# Scan only staged (index) files, JSON output — ideal for a pre-commit hook
git-jan secrets scan --staged --format json

# Scan files changed since a ref, and a specific commit
git-jan secrets scan --since HEAD~1
git-jan secrets scan --commit <sha>

# List and clean merged branches (dry-run by default)
git-jan branch list
git-jan branch clean --apply

# Install the pre-commit hook / run diagnostics
git-jan install-hook
git-jan doctor
```

`--staged`, `--since`, and `--commit` are mutually exclusive.

---

## Installation & build

The project is developed and built exclusively inside a **Podman + Nix** container.
Your host only needs **Git**, **GNU Make**, and **Podman** — no host Rust/Cargo (see
[`DEV.md`](DEV.md) for full development instructions).

```bash
# Build the container image and compile the binary via Nix
make build

# Run the test suite & hermetic checks (cargo tests inside the Nix sandbox)
make test

# Copy the reproducible release binary to ./target/bin/git-janitor
make extract-binary

# Run git-janitor inside the container against the current directory
make run
```

> Note: Nix ignores untracked files. Git-add new source files (`git add <file>`)
> before running `make build` / `make test`.

---

## Zero-dependency guarantee

`Cargo.toml`'s `[dependencies]` stays **100% empty**. Everything is built on the Rust
standard library:

- CLI parsing via `std::env::args()`
- Git object/ref/index/config parsing via `std::fs` and `std::path`
- Manual JSON serialization via `std::fmt::Write`
- Static init via `std::sync::LazyLock`, concurrency via `std::sync`
- TTY-aware color via raw ANSI escapes, respecting `NO_COLOR`

No external binaries are invoked at runtime — git state is read directly from `.git/`.
See [`STDLIB.md`](STDLIB.md) for a complete crate-for-stdlib substitution log
(one entry per replaced dependency, e.g. `git2`, `clap`, `serde_json`, `walkdir`,
`tempfile`, `sha1`, and more).

---

## Reproducible build

The build is reproducible end-to-end:

- **Pinned Nix inputs** in `flake.nix` pin the exact Rust toolchain (1.98.0, edition 2024).
- **Deterministic compiler flags**: `codegen-units = 1`, `lto = true`,
  `panic = "abort"`, `strip = true`.
- **Path remapping** via `--remap-path-prefix` so build paths do not leak into the binary.

---

## Hackathon: "Zero Dependency"

This project is a submission for the **Zero Dependency Hackathon**. Contributors
divided the codebase into self-contained modules:

- **Person 1**: `inflate`, `objects`, `repo`, `index`, `graph`, `cli`, `output`,
  `branch`, `hook`, `doctor`, `testdata`, `testutil`, and `fn main`.
- **Person 2**: `entropy`, `patterns`, `leakignore`, and `secrets`.

The entire implementation lives in a single file, `src/main.rs`.
