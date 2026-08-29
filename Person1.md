# Person 1 — Pipeline `git-jan` (~70% of work)

You are implementing the **foundation and branch subsystem** of `git-jan` (aka `git-janitor`),
a Zero Dependency Hackathon Rust CLI. After you finish, Person 2 implements the secrets
scanner **on top of your real, working code**. Your code must be production-quality and correct.

This file is written for a competent human or an AI coding agent. Follow it top to bottom.
Each task ends with a **Verify** block: run it before moving on.

---

## 0. Hard constraints (non-negotiable)

1. **Zero third-party dependencies.** `Cargo.toml` `[dependencies]` stays empty forever.
   `std::` only. Never add a crate.
2. **No shelling out.** NEVER use `std::process::Command` — not for `git`, not for anything.
   No external binary at runtime. All git data comes from reading `.git/` directly.
3. **Single file.** Everything lives in `src/main.rs`: your modules are *inline*
   `mod name { ... }` blocks. No other source files may be created (except Person2's,
   which are also inline mods in the same file).
4. **Build/verify only through Podman + Nix.** Never run host `cargo`/`rustc`.
5. Nix ignores **untracked** files. After creating/editing any file you intend to build,
   run `git add <file>` first, then build.
6. No inline explanatory comments. `///` doc comments on public items only.
7. Keep all `use` statements *inside* your `mod` block. Reference sibling modules with
   `crate::module::item`. Do not `pub use` anything from another module's scope.

---

## 1. Environment & verification loop

The host has Podman only (no host Rust/Nix workflows). Everything builds in a container:

```bash
# one-time: build the dev image (nix + rust toolchain)
make build

# run the full check: nix flake check -> crane -> cargo test
make test

# pull the binary to the host
make extract-binary
./target/bin/git-janitor ...
```

**Your daily loop:** edit `src/main.rs` → `git add src/main.rs` → `make test`.
Each `make test` re-runs `nix flake check` incrementally using caches, so it is fast once
the image is built. All your tests are `#[cfg(test)]` modules inside `main.rs`.

A real fixture repo (packed + branch topology) may be committed under
`/tmp/opencode/fixture-repos/`. It is created with system git **by you in the shell** during
development only — the product + its Rust tests never spawn git.

---

## 2. Ownership map (do not touch each other's regions)

| You own (fill stubs) | Person 2 owns (STUB UNTIL THEN — never edit) |
|---|---|
| `mod inflate`, `mod objects`, `mod repo`, `mod index`, `mod graph`, `mod cli`, `mod output`, `mod branch`, `mod hook`, `mod doctor`, `fn main` | `mod secrets`, `mod patterns`, `mod entropy`, `mod leakignore` |
| `mod testdata`, `mod testutil` (shared fixtures, frozen — you may add) | their `#[cfg(test)]` mods |

Person 2's modules already exist as **stubs** in `main.rs` (empty `Ok(vec![])` / `false`
bodies, exact signatures). Leave their bodies untouched. Your `main()` must wire the frozen
`secrets::run(&Repo, ScanTarget) -> Result<Vec<Finding>, ScannerError>` signature (returns
empty today, real findings after Person 2). This keeps Person 2's work handoff clean.

---

## 3. The contract — implement exactly these items

### `mod inflate`
```rust
pub struct InflateError;                       // Display: "inflate error: <reason>"
pub fn inflate_zlib(data: &[u8]) -> Result<Vec<u8>, InflateError>;  // full zlib stream
pub fn adler32(data: &[u8]) -> u32;            // Adler-32, used internally + exposed for tests
pub fn inflate_raw(data: &[u8]) -> Result<Vec<u8>, InflateError>;   // DEFLATE, no wrapper
```
- zlib wrapper: verify header (CMF/FLG, CM==8, reject FDICT), inflate, verify Adler-32.
- DEFLATE: 32KB sliding window, stored/fixed/dynamic blocks, canonical Huffman decode,
  MSB-first bit order, end-of-block handling, corrupted-stream errors.
- No allocation bombs: cap output (see `max_inflate_len` below, default 256 MiB).

### `mod objects`
```rust
pub struct Oid([u8; 20]);                      // SHA-1
impl Oid { pub fn from_hex(s:&str)->Result<Oid,ObjError>; pub fn to_hex(&self)->String; pub fn is_zero(&self)->bool }
pub enum ObjError { Corrupt(String), NotFound(Oid), Unsupported(String), TooLarge(usize) }  // + Display/Error
pub enum Obj { Commit(Commit), Tree(Vec<TreeEntry>), Blob(Vec<u8>), Tag(Oid) }
pub struct Commit { pub tree: Oid, pub parents: Vec<Oid> }
pub struct TreeEntry { pub mode: String, pub name: Vec<u8>, pub oid: Oid }
pub fn load_object(repo:&repo::Repo, oid:&Oid) -> Result<Obj,ObjError>;
pub fn load_commit(repo:&repo::Repo, oid:&Oid) -> Result<Commit,ObjError>;
pub fn load_tree  (repo:&repo::Repo, oid:&Oid) -> Result<Vec<TreeEntry>,ObjError>;
pub fn load_blob  (repo:&repo::Repo, oid:&Oid) -> Result<Vec<u8>,ObjError>;
pub const max_inflate_len: usize;              // 256 MiB guard
pub fn object_path(repo:&repo::Repo, oid:&Oid) -> Option<PathBuf>;  // loose .git/objects/xx/yyy… if present
```
- **Loose** objects: `objects/xx/rest`, header `<type> <size>\0`, zlib stream.
- **Packed** objects: parse `.idx` (v2, fanout + 20-byte OIDs → offset) then `.pack`
  (entry header varint, OFS_DELTA / REF_DELTA resolution, recurrent base lookup across
  loose + all packs). Memoize decoded objects per `Repo` in a `RefCell<HashMap<[u8;20], Rc<Obj>>>`.
- Refuse SHA-256 repos (detect via `config` `extensions.objectformat`) with
  `Err(ObjError::Unsupported("sha256 repos are not supported"))`.
- Commit parsing: `tree`, `parent` lines, gpgsig continuation lines, ignore unknown header
  lines, split body after blank line. Tree parsing: `<mode> <name>\0<20-byte oid>`,
  recurse nothing (flat only). Tag: dereference `object` line target.

### `mod repo`
```rust
pub struct Repo { pub work_dir: PathBuf, pub git_dir: PathBuf }
pub enum RepoError { NotARepository, Io(io::Error), Parse(String) }   // + Display/Error
pub enum HeadRef { Branch(String), Detached(Oid), Unborn }
pub struct Config { pub bare: bool, pub sha256: bool, pub branch_merge: HashMap<String,String>,
                    pub branch_remote: HashMap<String,String>, pub remotes: Vec<String>,
                    pub protected: Vec<String>, pub base_branch: Option<String>, pub leak_ignore_paths: Vec<String> }
pub struct Ref { pub name: String, pub oid: Oid }
pub fn find_repo_from(start:&Path) -> Result<Repo,RepoError>;         // walk up; .git dir or "gitdir:" file (worktree)
pub fn read_config(repo:&Repo) -> Result<Config,RepoError>;           // INI-ish: sections, [sub "x"], +continued lines, %? no, quoted values, # ;
pub fn head(repo:&Repo) -> Result<HeadRef,RepoError>;
pub fn local_branches(repo:&Repo) -> Result<Vec<Ref>,RepoError>;      // loose + packed, loose wins, no dups, sorted
pub fn remote_branch_oid(repo:&Repo, remote:&str, branch:&str) -> Result<Option<Oid>,RepoError>;
pub fn upstream_oid(repo:&Repo, cfg:&Config, branch:&str) -> Result<Option<Oid>,RepoError>;   // via cfg.branch_remote/merge
pub fn oid_of_ref(repo:&Repo, name:&str) -> Result<Oid,RepoError>;    // refs/heads/x, refs/remotes/x/y, tags, or raw HEAD~n? (no: raw oid only)
pub fn resolve_refish(repo:&Repo, s:&str) -> Result<Oid,RepoError>;   // accept <branch>, "HEAD", raw 40-hex or short hex
pub fn delete_local_branch(repo:&Repo, name:&str) -> Result<(),RepoError>;  // rm ref + reflog + strip from packed-refs (temp+rename, keep peeled lines)
pub fn default_branch(repo:&Repo) -> Result<Option<String>,RepoError>; // HEAD symref; fall back to main/master existing
```
- Config parser: handle `[core]`, `[branch "k"]`, `[remote "k"]`, `[git-janitor]`, `[extensions]`,
  quoted values with `\"`/`\\` escapes, `#`/`;` comments, backslash continuation, bare section
  values. Keep it robust but small. `protected` default: `["main","master","develop"]` unless
  `git-janitor.protected` (comma-separated) overrides. `base_branch` from `git-janitor.base`.
- `packed-refs`: header comment line, `<oid> <refname>` lines, `^<oid>` peeled lines.
- Deleting a branch must refuse protected/current/HEAD branches (check in `branch` module, not here).

### `mod index`
```rust
pub struct IndexEntry { pub path: String, pub mode: u32, pub oid: Oid, pub stage: u8 }
pub enum IndexError { Io(io::Error), Corrupt(String), Missing, Unsupported(String) }  // + Display/Error
pub fn staged_entries(repo:&Repo) -> Result<Vec<IndexEntry>,IndexError>;
```
- Parse `.git/index`: `DIRC` magic, version 2/3/4, entry count, per-entry fixed header,
  extended flags (v3+), path padding (v2/3), v4 path prefix-compression + varint,
  hash-verify footer (SHA-1 of all preceding bytes) when readable.
- Skip entries with `--assume-unchanged` / `--skip-worktree` bits? (yes: skip-worktree).
- Return concat of the index; `Repo::work_dir`-relative paths.

### `mod graph`
```rust
pub fn is_reachable(repo:&repo::Repo, from:&Oid, target:&Oid) -> Result<bool,objects::ObjError>;  // BFS over parents; from→mut reach target
pub fn commits_only_in(repo:&repo::Repo, left:&Oid, right:&Oid) -> Result<usize,objects::ObjError>;  // count of commits reachable from left but not right
pub fn files_in_commit(repo:&repo::Repo, oid:&Oid) -> Result<Vec<String>,objects::ObjError>;          // tree walk, sorted, slash-joined; name bytes as UTF-8 lossy
pub fn changed_files_between(repo:&repo::Repo, from:&Oid, to:&Oid) -> Result<Vec<String>,objects::ObjError>;  // recursive tree diff, path list
```
- Use `HashSet<[u8;20]>` visited per walk. Cycle-safe. Stop early on match.
- Empty/missing parent => terminal. Missing object => propagate `ObjError::NotFound`.
- `changed_files_between` compares entries by name; recurse on mode!=equal or oid!=equal
  subtrees; include added/modified/removed paths (removed: Person 2 scans nothing for them,
  but include for completeness).

### `mod cli`
```rust
pub enum Format { Human, Json }
pub struct ScanOpts { pub staged: bool, pub since: Option<String>, pub commit: Option<String>, pub format: Format }
pub enum Command { BranchList { base: Option<String> }, BranchClean { base: Option<String>, apply: bool },
                   SecretsScan(ScanOpts), InstallHook { force: bool }, Doctor, Help, Version }
pub enum CliError { Unknown(String), Missing(String), Parse(String) }   // + Display/Error
pub fn parse<I: Iterator<Item=String>>(args:I) -> Result<Command,CliError>;   // skip argv0; supports "git jan …" and "git-jan …"
pub fn usage() -> &'static str;
```
- Flags: `--apply`, `--base <b>`, `--staged`, `--since <refish>`, `--commit <refish>`,
  `--format json|human`, `--force`, `--help`, `--version`.
- `--staged` + `--since` + `--commit` are mutually exclusive (error otherwise).
- Errors → `CliError`; main prints `usage()` + message to stderr and exits `2`.

### `mod output`
```rust
pub fn is_tty() -> bool;                       // stdout is_terminal() && env NO_COLOR unset
pub fn paint(s:&str, code:u8) -> String;       // "\x1b[{code}m{s}\x1b[0m" only when is_tty()
pub const GREEN:u8; RED:u8; YELLOW:u8; CYAN:u8; BOLD:u8;
pub fn json_escape(s:&str) -> String;          // manual JSON string escaping
pub fn redact(s:&str) -> String;               // first 4 + "…" + last 4 chars, else "…"
pub fn now_label() -> String;                  // for reports, optional
```
- `redact` never leaks full secrets: len<=8 → `"…"`.

### `mod branch`
```rust
pub struct BranchInfo { pub name:String, pub oid:objects::Oid, pub merged:bool, pub ahead:usize, pub behind:usize,
                        pub has_upstream:bool, pub protected:bool, pub current:bool }
pub enum BranchError { Repo(repo::RepoError), Obj(objects::ObjError), Refuse(String) }   // + Display/Error
pub fn analyze(repo:&repo::Repo, cfg:&repo::Config, base:Option<&str>) -> Result<Vec<BranchInfo>,BranchError>;
pub fn delete_branch(repo:&repo::Repo, cfg:&repo::Config, name:&str) -> Result<(),BranchError>;  // guards protected/current
pub fn format_list(infos:&[BranchInfo], base:&str) -> String;
pub fn format_clean(infos:&[BranchInfo], delete:&[String], keep:&[String], apply:bool) -> String;
```
- `analyze` per local branch: current (checked out), protected (cfg.protected), merged
  (`graph::is_reachable(base_tip, branch_oid)`), upstream (`repo::upstream_oid`),
  ahead/behind (`graph::commits_only_in`, both directions vs upstream).
- `list` output mirrors the spec example (`✓`/`⚠`/`○` + per-branch reasons + summary counts).
- `clean` proposes deletion only when: !protected && !current && merged && has_upstream && ahead==0.
  Dry-run by default; `--apply` deletes via `repo::delete_local_branch`. Report exactly what
  happened. Exit 0. Empty repo / detached HEAD handled gracefully.

### `mod hook`
```rust
pub fn install_hook(repo:&repo::Repo, force:bool) -> Result<PathBuf,HookError>;  // writes git_dir/hooks/pre-commit, chmod 755
pub const HOOK_SCRIPT: &'static str;          // POSIX sh: exec git-jan secrets scan --staged (exit 1 on finding)
```
- never overwrite existing hook unless `force`.
- Hook errors: cannot write, exists w/o force.

### `mod doctor`
```rust
pub struct DoctorReport { pub ok:Vec<String>, pub warn:Vec<String>, pub errs:Vec<String> }
pub fn run(repo:&repo::Repo) -> DoctorReport;  // HEAD, config parse, refs read, index parse, objects (load HEAD commit), leakignore (via Person2 stub = ok)
pub fn format_report(r:&DoctorReport) -> String;
```
- `ok`/`errs` printed with `✓`/`✗`. Exit 0 if no errs else 2.

### `fn main`
- `cli::parse(env::args())`; dispatch; exit codes:
  - `0` success/no findings/clean
  - `1` secrets found (scan) or doctor problems? No: doctor=2. Secrets=1 when findings.
  - `2` any error (parse, io, operational).
- Secrets path: build target from `ScanOpts` → `secrets::run(&repo, target)` →
  findings empty → exit 0, else print (human or json for `--format json`) → exit 1.
  `--format json` output is the *only* stdout (machine-readable).

---

## 4. Test infrastructure (shared)

`mod testdata` (frozen, M0): embedded git-zlib fixtures (hex strings), historically
generated with real git. Use `testutil::hex(...)` to decode. Add more fixtures as needed.
`mod testutil` (frozen, M0): `fn hex(s:&str)->Vec<u8>; fn unique_tempdir(tag:&str)->PathBuf;`
and cleanup helper. Do not add `tempfile`-like deps. Tests create `.git` fixtures with
`std::fs` only — never `Command::new`.

---

## 5. Tasks (do in order; Verify each)

### Task A — `mod inflate`
Implement A fully + tests:
- known blobs (from `testdata`): "hello world\n", multiline, an AWS-secret line,
  a commit fixture, a large repeated-pattern blob (>64KB, exercises distance).
- error cases: truncated, bad header, bad adler, FDICT set.
- **Verify:** `make test`; tests `inflate_zlib_hello`, `inflate_zlib_multiline`,
  `inflate_zlib_large`, `inflate_bad_header`, `inflate_truncated`,
  `inflate_bad_adler`, `adler32_known` all pass.

### Task B — `mod objects`
Implement loose + pack + delta. Fixtures: loose object(s) copied from the real fixture repos;
a packed set. Load the repo's own HEAD commit through both paths.
Tests: `loose_commit_parse`, `loose_blob_roundtrip`, `pack_commit_parse`,
`pack_ofs_delta_blob`, `pack_ref_delta_blob`, `tree_parse_entries`, `tag_deref`,
`not_found_object`, `sha256_repo_rejected`.
- **Verify:** `make test` green with those tests; a small dev-only test gated by env
  `GJ_DEV_REPO=/path/to/fixture repo` that asserts `load_commit(head)` works when set
  (skip when unset).

### Task C — `mod repo`
Tests: walk-up discovery, worktree `gitdir:` file, config parsing (sections, subsections,
quoted, comments, continuation), packed-refs + loose precedence, upstream resolution,
`resolve_refish`, `delete_local_branch` (incl. packed-refs rewrite keeping peeled lines),
detached/unborn HEAD.
- **Verify:** `make test`; `repo_discovery_walk_up`, `repo_worktree_file`,
  `config_branch_section`, `config_quoted_value`, `config_continuation`,
  `packed_refs_precedence`, `upstream_oid_resolves`, `delete_branch_removes_loose_and_packed`,
  `head_unborn`, `head_detached` pass.

### Task D — `mod index`
Hand-built index bytes of versions 2 and 4 (fixtures in `testdata`; you may add hex).
Tests: `index_v2_entries`, `index_v4_prefix_paths`, `index_corrupt`, `index_stage_conflicts`.
- **Verify:** `make test` green with those tests.

### Task E — `mod graph`
Build a small commit chain fixture in `testdata` (embedded compressed commits) to test
reachability + ahead counts. Tests: `reachable_true`, `reachable_false`,
`ahead_one`, `behind_two`, `same_commit_zero`, `cycle_safe`, `files_in_commit`,
`changed_files_between`.
- **Verify:** `make test` green with those tests.

### Task F — `mod cli` + `mod output`
Tests: every command form, flag combos, mutual-exclusion error, unknown cmd, json escape
quotes/backslash/unicode/control, redact short/long, `redact_never_leaks_full`.
- **Verify:** `make test`; `cli_branch_list`, `cli_clean_apply`, `cli_scan_all_flags`,
  `cli_scan_conflict_error`, `cli_unknown`, `json_escape_basic`, `json_escape_unicode`,
  `redact_long`, `redact_short` pass.

### Task G — `mod branch`
Fixtures: multi-branch repos (loose branches + packed, upstreams via config).
Tests: merged-safe, unmerged-keep, protected-keep, current-keep, ahead>0-keep,
no-upstream-keep, dry-run lists no deletion, `--apply` deletes only eligible,
refuses current/protected, empty-repo ok, detached HEAD ok.
- **Verify:** `make test`; test names `analyze_merged_safe`, `analyze_unmerged_keep`,
  `analyze_protected`, `analyze_current`, `analyze_ahead_unpushed`,
  `analyze_no_upstream`, `clean_dry_run_no_delete`, `clean_apply_deletes_eligible`,
  `delete_refuses_current`, `delete_refuses_protected`, `empty_repo_ok`,
  `detached_head_ok` pass. **Live check:** against the fixture repo, `branch list`
  prints the expected ✓/⚠/○ blocks and `branch clean` dry-run prints no changes.

### Task H — `mod hook`, `mod doctor`, `fn main`
Tests: hook installed (perm 755), refuses overwrite, force overwrites; doctor ok/errs.
**Verify after wiring main:**
- `make test` full green (including Person 2 stub tests — they already pass).
- `make extract-binary` then run against the fixture repo:
  - `./target/bin/git-janitor branch list` (and `--base` variant)
  - `./target/bin/git-janitor branch clean` (dry-run), note echo "No changes made."
  - `./target/bin/git-janitor secrets scan` → exit 0 (stub, no findings), prints empty report
  - `./target/bin/git-janitor secrets scan --staged --format json` → exit 0, `{"findings":[]}`
  - `./target/bin/git-janitor doctor` → all ✓, exit 0
  - `./target/bin/git-janitor` (no args) → usage, exit 2
  - unknown flag → usage, exit 2

---

## 6. Definition of Done (your phase)

- All above tests passing in `make test`.
- `main.rs` still a single file; `Cargo.toml` deps empty; grep for `std::process` → 0 hits.
- The 4 Person 2 modules still untouched stubs.
- Signature conformance: every contract item in §3 exists with the exact name/signature
  (Person 2 relies on it). `grep -n "pub fn secrets::run\|pub fn staged_entries\|pub fn load_blob"` present.

## 7. Handoff to Person 2 (checklist)

1. Run `make test` and confirm the full suite is green.
2. Confirm contract conformance (§3) — especially the APIs Person 2 consumes:
   `repo::Repo/Repo::find`, `repo::Config`, `objects::load_blob`,
   `objects::load_tree`, `objects::Oid`,
   `index::staged_entries`, `graph::changed_files_between`,
   `graph::files_in_commit`, `cli::ScanOpts/Command/Format`,
   `output::json_escape/redact/is_tty`, `secrets::run` (stub).
3. Commit your work (`git add src/main.rs Person1.md && git commit`), leave the tree green.
4. Tell the organizer exactly which signature names you changed (you're not allowed to change them
   without double-checking Person2.md first).

Good luck — the branch side is the heavy end. Get `inflate` right and everything else follows.