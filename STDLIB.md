# STDLIB Substitutions Log

This log tracks every third-party crate replaced with custom Rust standard library (`std::*`) implementations for `git-janitor`.

| # | Target Crate | Standard Library Replacement | Description & File Location |
|---|---|---|---|
| 1 | `miniz_oxide` / `flate2` | `std::io`, bitwise arithmetic | RFC 1950 zlib wrapper & RFC 1951 DEFLATE engine with fixed/dynamic Huffman trees and Adler-32 in `mod inflate` |
| 2 | `git2` / `libgit2` | `std::fs`, `std::path` | Direct filesystem parsing of loose objects, packfiles (v2 idx & delta resolution), refs, index, and git config in `mod objects`, `mod repo`, `mod index` |
| 3 | `clap` / `structopt` | `std::env::args()` | Iterative command-line parser with flags, options, mutual exclusion, and help formatting in `mod cli` |
| 4 | `serde` / `serde_json` | `std::fmt::Write`, string escaping | Custom machine-readable JSON formatter and JSON string escaper in `mod output` and `mod secrets` |
| 5 | `walkdir` | `std::fs::read_dir` | Recursive tree directory traversal and path prefix matching in `mod repo` and `mod graph` |
| 6 | `tempfile` | `std::env::temp_dir()`, `std::sync::atomic` | Unique temporary test directory generator with monotonic counters and timestamp entropy in `mod testutil` |
| 7 | `colored` / `ansi_term` | `std::io::IsTerminal`, ANSI escape codes | TTY-aware colored terminal painting respecting `NO_COLOR` in `mod output` |
| 8 | `sha1` / `ring` | bitwise arithmetic, FIPS 180-1 | Complete 80-round SHA-1 message digest engine in `mod objects::sha1` |
| 9 | `anyhow` / `thiserror` | `std::error::Error`, `std::fmt::Display` | Custom structured error enumerations across all modules with descriptive display formatting |
| 10 | `hex` | `std::fmt::Write`, `std::char::to_digit` | Hexadecimal encoding and decoding utilities in `mod objects::Oid` and `mod testutil::hex` |
| 11 | `petgraph` | `std::collections::{VecDeque, HashSet}` | BFS-based reachability and graph commit topological distance traversals in `mod graph` |
| 12 | `lazy_static` / `once_cell` | `std::cell::RefCell`, `thread_local!` | Thread-local object memoization cache in `mod objects` |