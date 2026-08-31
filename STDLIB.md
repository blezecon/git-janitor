# STDLIB Substitutions Log

This log documents every third-party crate replaced with custom Rust standard library (`std::*`) implementations for **`git-janitor`** (`git-jan`).

---

## Zero-Dependency Guarantee

* **`Cargo.toml` `[dependencies]` is 100% empty.**
* **Standard Library Only (`std::*`)**: Zero external crates, zero vendored C libraries, zero network-fetched code at runtime.
* **No Shelling Out**: Zero calls to `std::process::Command` (no external `git` binary required).

---

## Comprehensive Substitution Table

| # | Target Crate | Standard Library Replacement | Description & File Location |
|---|---|---|---|
| 1 | `miniz_oxide` / `flate2` | `std::io`, bitwise arithmetic | RFC 1950 zlib container wrapper & RFC 1951 DEFLATE decompression engine with stored blocks, fixed Huffman codes, dynamic Huffman tree reconstruction, and Adler-32 checksum calculation in `mod inflate`. |
| 2 | `git2` / `libgit2` | `std::fs`, `std::path` | Direct filesystem parsing of loose object database, binary packfiles (v2 idx fanout table & OFS/REF delta reconstruction), refs (`refs/heads/*`, `packed-refs`), binary git index (DIRC v2/v3/v4 with prefix compression), and git config in `mod objects`, `mod repo`, and `mod index`. |
| 3 | `clap` / `structopt` | `std::env::args()` | Iterative command-line argument parser supporting subcommands, options, boolean flags, mutual exclusion rules (`--staged` vs `--since` vs `--commit`), and usage help formatting in `mod cli`. |
| 4 | `serde` / `serde_json` | `std::fmt::Write`, string escaping | Custom machine-readable JSON serializer and RFC 8259-compliant JSON string escaper in `mod output` and `mod secrets`. |
| 5 | `walkdir` | `std::fs::read_dir` | Recursive directory traversal engine with `.git` directory skipping and 1 MiB maximum scan file size limiter in `mod repo`, `mod graph`, and `mod secrets`. |
| 6 | `tempfile` | `std::env::temp_dir()`, `std::sync::atomic::AtomicUsize`, `std::time::SystemTime` | Thread-safe, collision-free temporary test directory generator with monotonic counters and timestamp entropy in `mod testutil`. |
| 7 | `colored` / `ansi_term` | `std::io::IsTerminal`, raw ANSI codes | TTY-aware colored terminal painting with full support for the `NO_COLOR` environment variable standard in `mod output`. |
| 8 | `sha1` / `ring` | bitwise arithmetic, FIPS 180-1 | Complete 80-round SHA-1 message digest engine computing 160-bit cryptographic object IDs in `mod objects::sha1`. |
| 9 | `anyhow` / `thiserror` | `std::error::Error`, `std::fmt::Display` | Custom structured error enumerations across all modules with descriptive display formatting and source error chaining. |
| 10 | `hex` | `std::fmt::Write`, `std::char::to_digit` | Hexadecimal encoding and decoding utilities in `mod objects::Oid` and `mod testutil`. |
| 11 | `petgraph` | `std::collections::{VecDeque, HashSet}` | BFS-based reachability engine, cycle-tolerant topological commit graph distance calculator, and recursive tree diffing in `mod graph`. |
| 12 | `lazy_static` / `once_cell` | `std::cell::RefCell`, `thread_local!` | Thread-local object memoization cache in `mod objects` for high-performance loose and packfile object loading. |
| 13 | `regex` / `glob` | custom zero-allocation tokenizers & glob matcher | Shannon entropy calculator ($H = -\sum p \log_2 p$), pattern detectors (AWS, GitHub, JWT, Slack, Google, SSH keys, Bearer headers, key-value assignments), and `.leakignore` wildcard matcher (`*`, `**`, `?`, `/prefix` anchoring, `!` negation) in `mod entropy`, `mod patterns`, and `mod leakignore`. |