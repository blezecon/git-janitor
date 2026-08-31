# git-janitor (`git-jan`)

**`git-janitor`** (aliased as **`git-jan`**) is a blazing-fast, zero-dependency Git housekeeping and secret-leak scanner written entirely in Rust.

It parses `.git/` internals directly from disk with **zero external crates** (standard library only) and **never shells out** to the `git` binary. Built for the **Zero Dependency Hackathon**, it compiles into a self-contained, reproducible binary via a pinned **Nix** environment inside an isolated **Podman** container.

---

## Quick Install

### Linux, macOS, and FreeBSD (POSIX Shell)
```bash
curl -fsSL https://raw.githubusercontent.com/blezecon/git-janitor/release/install.sh | sh
```

### Windows (PowerShell)
```bash
irm https://raw.githubusercontent.com/blezecon/git-janitor/release/install.ps1 | iex
```

> The installer automatically detects your OS and architecture, downloads the matching pre-built binary, installs `git-janitor` and `git-jan` into `~/.local/bin`, and registers the directory in your shell `$PATH`.

---

## How to Run

Because `git-janitor` installs both `git-janitor` and `git-jan` into your `$PATH`, Git's built-in command discovery allows you to run it in any of these ways:

```bash
git-janitor <command>   # Full name
git-jan <command>       # Short binary
git janitor <command>   # Native git subcommand
git jan <command>       # Short native git subcommand
```

### Command Reference

```text
git-jan <COMMAND> [OPTIONS]

COMMANDS:
    branch list [--base <branch>]
        List local branches and their merged/upstream tracking status.

    branch clean [--base <branch>] [--apply]
        Delete merged local branches that have a tracking upstream (dry-run by default).

    secrets scan [--staged] [--since <ref>] [--commit <ref>] [--format human|json]
        Scan repository files for leaked credentials and high-entropy secrets.

    install-hook [--force]
        Install git pre-commit hook into .git/hooks/pre-commit.

    doctor
        Run repository health and integrity diagnostics.

    help, --help
        Print help information.

    version, --version
        Print version information.
```

### Examples

```bash
# 1. Scan the whole working tree for secrets (human-friendly colored output)
git jan secrets scan

# 2. Scan only staged index files (fast pre-commit check)
git jan secrets scan --staged

# 3. Output machine-readable JSON (ideal for CI/CD pipelines)
git jan secrets scan --staged --format json

# 4. Scan files modified since a ref, or in a specific commit
git jan secrets scan --since HEAD~5
git jan secrets scan --commit 4fe7414

# 5. List branches and their merged status against main
git jan branch list

# 6. Clean merged branches (dry-run preview by default)
git jan branch clean

# 7. Apply actual deletion of merged local branches
git jan branch clean --apply

# 8. Install the automated pre-commit hook
git jan install-hook

# 9. Run repository diagnostics
git jan doctor
```

---

## What It Does

### 1. Secret Scanner Subsystem (`secrets scan`)
* **High-Entropy Detector**: Shannon entropy calculation ($H = -\sum p \log_2 p$) with threshold filtering (`ENTROPY_THRESHOLD_LOW = 3.8`, token length $\ge 12$).
* **Built-in Pattern Detectors**:
  * AWS Access Key IDs (`AKIA...`)
  * GitHub Personal Access Tokens (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_` + 36 chars)
  * Private Keys (`-----BEGIN ... PRIVATE KEY-----`)
  * JSON Web Tokens (3-segment Base64URL `eyJ...`)
  * Slack API & Bot Tokens (`xoxb-`, `xoxp-`, etc.)
  * Generic API Keys (`sk_live_`, `sk-` + 16 chars)
  * Google API Credentials (`AIza` + 35 chars)
  * Authorization Headers (`Basic ...`, `Bearer ...`)
  * Common secret key-value assignments (`password`, `token`, `secret`, `api_key`, `credential`, etc.)
* **`.leakignore` Support**: Glob-based ignore patterns (`*`, `**`, `?`, `/prefix` anchoring, `!` negation) layered over repository config `leak_ignore_paths`.
* **Redacted Output**: Leaked tokens are **never** fully exposed in logs or stdout (always redacted as `AKIA…MPLE`).

### 2. Git Housekeeping Subsystem
* **`branch list`**: Identifies merged, unmerged, current, protected, and gone branches with ahead/behind commit distances.
* **`branch clean`**: Deletes merged local branches that have been pushed to tracking upstreams. Dry-run by default; requires `--apply` for execution. Protected branches (`main`, `master`, `develop`, `release`, etc.) are never deleted.
* **`install-hook`**: Installs an executable pre-commit hook into `.git/hooks/pre-commit` that blocks commits containing staged secrets.
* **`doctor`**: Verifies repository structure, HEAD resolution, config parsing, packed-refs, index entries, and object database integrity.

---

## Exit Codes

| Code | Meaning |
|:---:|---|
| **`0`** | Success / clean / no secrets found |
| **`1`** | Potential secrets detected / pre-commit blocked |
| **`2`** | Scanner, configuration, or operational error |

---

## Limitations & Guardrails

* **Zero Dependencies (`std::*` Only)**: No external crates are used. All algorithms (zlib inflate, SHA-1, git object/index/packfile parsers, JSON serialization, regex/glob matching) are implemented using standard library primitives.
* **No Shelling Out**: The binary never invokes `std::process::Command("git")`. All git metadata is parsed directly from `.git/` filesystem structures.
* **Scan Size Guard**: Files larger than **1 MiB** are skipped during text scanning to prevent memory spikes.
* **Decompression Guard**: Object decompression enforces a hard **256 MiB** safety guard (`MAX_INFLATE_LEN`) against decompression bombs.
* **Binary File Handling**: Files with NUL bytes in the initial 8 KB are treated as binary (only the file path is inspected, body content is never rendered).
* **Object Format Support**: Built for standard SHA-1 Git repositories (SHA-256 object formats are detected and rejected gracefully).
* **Destructive Operation Guard**: Branch cleanup is dry-run by default and requires an explicit `--apply` flag.

---

## Reproducible Build Verification

| Build Run | SHA-256 Hash | Status |
|---|---|---|
| **Run 1** | `77fb537b857bfc15d1c0cbec33ab0105c457fd360d0af77d757f8edd0acb8db9` | Baseline |
| **Run 2** | `77fb537b857bfc15d1c0cbec33ab0105c457fd360d0af77d757f8edd0acb8db9` | Byte-identical match |

---

## How to Build from Source

`git-janitor` uses an isolated **Podman + Nix** container environment for fully reproducible builds. No local Rust or Nix installation is required on your host machine.

### Prerequisites
* **Git**
* **GNU Make**
* **Podman** (or **Docker**)

### Build Commands

```bash
# 1. Build container and compile native binary via Nix
make build

# 2. Run the full test suite (106 unit & integration tests inside Nix sandbox)
make test

# 3. Extract the release binary to ./target/bin/git-janitor
make extract-binary

# 4. Check code formatting and run Clippy
make fmt-check
make clippy

# 5. Cross-compile for a specific target platform
make build-target TARGET=x86_64-unknown-linux-gnu

# 6. Cross-compile all 7 supported target platforms
make build-all-targets

# 7. Package complete release archives with SHA256SUMS
make release VERSION=v0.1.0
```

---

## How to Uninstall

### Linux, macOS, and FreeBSD
```bash
curl -fsSL https://raw.githubusercontent.com/blezecon/git-janitor/release/install.sh | sh -s -- --uninstall
```
*(or manually remove `~/.local/bin/git-janitor` and `~/.local/bin/git-jan`)*

### Windows (PowerShell)
```sh
irm https://raw.githubusercontent.com/blezecon/git-janitor/release/install.ps1 | iex -args "-Uninstall"
```

---

## Zero-Dependency Proof & Substitutions

* [`deps-proof.txt`](deps-proof.txt) — Formal verification proving `[dependencies]` is 100% empty.
* [`STDLIB.md`](STDLIB.md) — Comprehensive log of all 13 third-party crates replaced with standard library implementations (e.g. `flate2`, `git2`, `clap`, `serde_json`, `walkdir`, `tempfile`, `sha1`, `petgraph`, `regex`).

---

## License

Released under the [MIT License](LICENSE).
