# Person 2 — Secret Scanner `git-jan` (~30% of work)

You are implementing the **secret scanner** of `git-jan` (aka `git-janitor`), a Zero
Dependency Hackathon Rust CLI. Person 1 has already finished and verified the foundation:
repo discovery, the git object layer (zlib inflate, loose + packed objects), the index
parser, commit-graph traversal, CLI, output, branch cleanup, hook, doctor — and `main()`
which already calls your entry point. **Your job: fill four stub modules in `src/main.rs`
live**: `mod entropy`, `mod patterns`, `mod leakignore`, `mod secrets`.

This file is written for a competent human or an AI coding agent. Follow it top to bottom.

---

## 0. Hard constraints (non-negotiable)

1. **Zero third-party dependencies.** `Cargo.toml` `[dependencies]` stays empty. `std::` only.
2. **No shelling out.** NEVER use `std::process::Command`. All git data comes from the
   read-only APIs Person 1 built (you call their functions; you never read `.git/` yourself —
   actually you MAY read `.git/` via the provided `repo::Repo` paths, but you never spawn a process).
3. **Single file.** You edit **only** the four stub `mod` blocks (`entropy`, `patterns`,
   `leakignore`, `secrets`) inside `src/main.rs`, plus your own `#[cfg(test)]` test modules
   inside `mod secrets` (or sibling test mods). **Do not touch** any module owned by Person 1
   (`inflate`, `objects`, `repo`, `index`, `graph`, `cli`, `output`, `branch`, `hook`,
   `doctor`, `testdata`, `testutil`, `fn main`).
4. **Build/verify only through Podman + Nix.** Never run host `cargo`/`rustc`.
5. `git add src/main.rs` before `make test` (Nix ignores untracked files).
6. No inline explanatory comments. `///` doc comments on public items only.
7. All `use` inside your `mod` blocks; reference Person 1's modules via `crate::name::item`.

---

## 1. Environment & verification loop

```bash
make build        # one-time (image already built by Person 1 — fast)
make test         # nix flake check -> crane -> cargo test  (your daily loop)
make extract-binary
./target/bin/git-janitor <cmd>   # live smoke against real fixture repos
```

---

## 2. The APIs Person 1 already built (consume these, never reimplement)

```rust
// discovery
fn repo::find_repo_from(cwd:&std::path::Path) -> Result<repo::Repo, repo::RepoError>;
//   Repo { work_dir: PathBuf, git_dir: PathBuf }
fn repo::read_config(repo:&Repo) -> Result<Config, repo::RepoError>;
//   Config { leak_ignore_paths: Vec<String>, .. }   // Person2 may read this too

// objects
type objects::Oid;                        // Oid::from_hex(&str)->Result<Oid,_>, Oid::to_hex(&self)->String
fn objects::load_blob(repo:&Repo, oid:&objects::Oid) -> Result<Vec<u8>, objects::ObjError>;
fn objects::load_tree(repo:&Repo, oid:&objects::Oid) -> Result<Vec<objects::TreeEntry>, objects::ObjError>;
//   TreeEntry { mode: String, name: Vec<u8>, oid: objects::Oid }

// index (staged)
struct index::IndexEntry { path:String, mode:u32, oid:objects::Oid, stage:u8 }
fn index::staged_entries(repo:&Repo) -> Result<Vec<index::IndexEntry>, index::IndexError>;

// graph (since / commit scans)
fn graph::changed_files_between(repo:&Repo, from:&objects::Oid, to:&objects::Oid)
    -> Result<Vec<String>, objects::ObjError>;              // pathes relative to repo
fn graph::files_in_commit(repo:&Repo, oid:&objects::Oid) -> Result<Vec<String>, objects::ObjError>;
fn repo::resolve_refish(repo:&Repo, s:&str) -> Result<objects::Oid, repo::RepoError>;  // "HEAD~10", branch, hex

// output helpers
fn output::json_escape(s:&str) -> String;
fn output::redact(s:&str) -> String;        // first4+…+last4, never full value
fn output::is_tty() -> bool;
fn output::paint(s:&str, code:u8) -> String;

// main already calls
secrets::run(repo:&Repo, target:ScanTarget) -> Result<Vec<Finding>, ScannerError>;
//   ScanTarget { Worktree | Staged | Since(String) | Commit(String) }
```

---

## 3. The contract — implement exactly these items

### `mod entropy`
```rust
pub fn shannon_bits(data:&[u8]) -> f64;            // H = -Σ p·log2 p bits per byte (0 for empty)
pub fn is_high_entropy(token:&str) -> bool;        // default: len>=12 (after trimming quote/punct),
                                                   // charset is alnum + _ - . / + = ~ and entropy>=3.8
pub const ENTROPY_THRESHOLD_LOW:f64;               // 3.8
pub const TOKEN_MIN_LEN:usize;                     // 12
```
- Tokens that are clearly words, dates, hex of fixed sizes, or repeated chars are de-boosted
  (see patterns context rules). Keep thresholds conservative to limit false positives.

### `mod patterns`
```rust
pub struct Hit { pub kind:&'static str, pub value:&'static str }   // 'static str values (no alloc)
pub fn detect(line:&str) -> Vec<Hit>;               // returns e.g. "aws-access-key", "github-token",
                                                    // "private-key", "jwt", "generic-api-key", "high-entropy-key"
```
- Known-prefix detectors (single line, first match wins per prefix group):
  - AWS access key id `AKIA[0-9A-Z]{16}` → `aws-access-key`
  - GitHub `ghp_|gho_|ghu_|ghs_|ghr_` + 36 chars `[0-9A-Za-z]` → `github-token`
  - Private key: line starts `-----BEGIN` and contains `PRIVATE KEY-----` → `private-key`
  - JWT-like: three dot-separated base64url segments, first `eyJ`, total len >= 64 → `jwt`
  - Slack `xox[baprs]-` + 12+ → `slack-token`; generic `sk_live_`, `sk-` + 16+ → `api-key`
  - Google `AIza[0-9A-Za-z_-]{35}` → `google-key`
  - Auth header `Basic ` base64 or `Bearer ` + 28+ chars → `auth-header`
- Context key==value: if line matches `(password|passwd|secret|token|api[_-]?key|apikey|
  client[_-]?secret|access[_-]?token|private[_-]?key|credential|auth)\s*[:=]\s*(.+)`
  the RHS is a candidate for `entropy::is_high_entropy` → `high-entropy-key`.
- Known non-secrets whitelist (embedded list: "changeme", "example", "placeholder",
  "your-", "xxxx", "demo", real hex/date literals) → excluded.

### `mod leakignore`
```rust
pub struct LeakIgnore { /* patterns: Vec<Glob> */ }
pub enum LeakError { Io(std::io::Error) }             // + Display/Error
pub fn load(repo:&repo::Repo) -> Result<LeakIgnore, LeakError>;   // repo/.leakignore if present (rooted); else empty
pub fn is_ignored(&self, path:&str) -> bool;          // normalized '/'-separated, relative to repo root
```
- Syntax: `#` comments, blank lines, `!` negation (re-include), `/prefix/` anchoring to repo
  root, trailing `/` dir-only, `*` matches within a path segment, `**` matches across,
  `?` single char. Later patterns override earlier ones. No shell-style brace/escape support.

### `mod secrets`
```rust
pub enum ScanTarget { Worktree, Staged, Since(String), Commit(String) }
pub struct Finding { pub kind:String, pub file:String, pub line:usize, pub redacted:String }
pub enum ScannerError { Repo(repo::RepoError), Obj(objects::ObjError), Config(repo::RepoError),
                        Index(index::IndexError), Walk(std::io::Error), Leak(leakignore::LeakError) }  // + Display/Error
pub fn run(repo:&Repo, target:ScanTarget) -> Result<Vec<Finding>, ScannerError>;
pub fn scan_text(file:&str, path:&str, data:&[u8]) -> Vec<Finding>;
pub fn human_report(findings:&[Finding]) -> String;
pub fn json_report(findings:&[Finding]) -> String;   // {"findings":[{"kind":..,"file":..,"line":..,"redacted":..}]}
```
- **Binary detection:** if the first 8000 bytes contain a NUL and the file isn't a known
  text type, treat as binary: scan only the filename; if filename itself is a hit, report at
  line 0 (line==0 means "whole file/filename"). Never attempt to render binary content.
- **Worktree:** walk `repo.work_dir` recursively (skip `.git`), skip ignored
  (`leakignore::is_ignored` with repo-relative path), read text files ≤ `max_scan_bytes`
  (default 1 MiB), `scan_text` per line (split on `\n`).
- **Staged:** `index::staged_entries` → for each entry, `objects::load_blob(oid)` →
  `scan_text` with the entry path. Do not scan working-tree files (respect the index).
- **Since / Commit:** resolve refish (`repo::resolve_refish`); for `Commit` use
  `graph::files_in_commit` (treat as added) and for `Since` `graph::changed_files_between`
  (added + modified), then load the **`to`-side** blob (or empty for removed) and scan.
- **Multiple findings per file** sorted by line. De-dup identical (kind,file,line,redacted).
- `redacted` = `output::redact` of the matched value (`&str` from the Hit).
- `human_report`: header, per finding `✗ {kind} — file:line`, summary
  `{n} potential secret(s) found.`, red via `output::paint`.
- `json_report`: exact schema above, `` str escapes via `output::json_escape`, sorted.
- **Read config**: `repo::read_config(repo)?.leak_ignore_paths` may add ignore patterns.

### `mod main` — you do NOT touch it. It already:
- builds `ScanTarget` from `cli::ScanOpts`, calls `run`, prints `human_report` or
  `json_report`, and exits `1` when findings exist, else `0`. Errors exit `2`.

---

## 4. Fixtures & test helpers (already in the tree)

- `mod testdata` / `mod testutil` (Person 1, frozen): `testutil::hex(&str)->Vec<u8>`,
  `testutil::unique_tempdir(tag)->PathBuf` + cleanup. zlib-compressed git blob fixtures for
  common secret lines. You may read these; do not modify.
- To test `Staged`/`Since`/`Commit`, build a fixture repo **in the shell** with system git
  (`git init`, add secret file, `git add`, `git commit`) under a temp dir, then write Rust
  tests that open it via `repo::find_repo_from`. System git is a *development tool only* —
  your Rust code never spawns anything.

---

## 5. Tasks (do in order; Verify each)

### Task A — `mod entropy` + `mod patterns`
Tests (inside a `#[cfg(test)] mod` you add):
`entropy_empty_zero`, `entropy_uniform_max`, `entropy_low_word`,
`is_high_entropy_random`, `is_high_entropy_short_rejected`,
`aws_key_detected`, `github_token_detected`, `private_key_detected`,
`jwt_detected`, `basic_auth_detected`, `key_value_high_entropy`,
`whitelisted_example_ignored`, `no_hit_normal_line`, `single_line_no_multiline_false_pos`.
- **Verify:** `make test`; all above pass.

### Task B — `mod leakignore`
Tests: `empty_ignores_nothing`, `star_ignores_segment`, `doublestar_ignores_path`,
`negation_reincludes`, `comment_and_blank`, `anchored_prefix_only_root`,
`dir_only_pattern`.
- **Verify:** `make test`; all above pass.

### Task C — `mod secrets` — core scan
Tests: `scan_text_aws`, `scan_text_multiline_no_bleed`, `binary_detected_skips_body`,
`binary_filename_hit_reported`, `worktree_finds_secret`, `worktree_respects_ignore`,
`worktree_skips_gitdir`, `line_numbers_sequential`, `dedup_findings`,
`findings_sorted_by_line`.
- **Verify:** `make test`; all above pass.

### Task D — `mod secrets` — git-backed targets + reports
Fixture repos (built with system git in shell, committed):
Tests: `staged_finds_secret`, `staged_ignores_unstaged_changes`,
`since_scans_committed`, `commit_target_works`, `root_resolve_head_tilde`,
`human_report_shape`, `json_report_schema`, `json_escape_round_trip`,
`exit_code_contract_no_findings`. (Main handles exit codes; test `run` returns empties.)
- **Verify:** `make test`; all above pass.

### Task E — end-to-end smoke (live)
Against fixture repos and this repo:
- `branch list` still works (Person 1 regression)
- `secrets scan` (worktree) — finding counts sane, real secrets redacted
- `secrets scan --staged` after `git add`ing a secret file → exit 1 (capture `$?`)
- `secrets scan --since HEAD~1 --format json | python3 -m json.tool` → valid JSON
- `secrets scan --commit HEAD~0` exit semantics correct
- `doctor` → ✓; unknown flag → exit 2
- `grep -c "std::process" src/main.rs` → 0
- `cargo test` full suite green (`make test`)

---

## 6. Definition of Done (your phase)

1. Every item in §3 implemented with exact signatures.
2. `make test` fully green (the whole suite, including Person 1's).
3. Live smoke in Task E passes.
4. Your code never touches Person 1's modules; diff of `src/main.rs` vs Person 1's last
   commit touches only `entropy`, `patterns`, `leakignore`, `secrets` and your test mods.
5. No new modules/tasks/files added without updating this doc and Person1.md.

## 7. Handoff to the integrator (checklist)

1. `make test` green; `make extract-binary`; smoke commands in §5.E pass.
2. Update nothing else. Commit: `git add src/main.rs && git commit`.
3. Leave `git status` clean.