# Development with Podman + Nix

You only need Git, GNU Make, and Podman on your machine. The container provides
Nix, Rust, and Cargo—do not install or run host Cargo for this project.

## Start the development environment

From the repository root, build the development image:

```bash
make build
```

The first build downloads the Nix container image and the project's pinned Nix
inputs. Later builds reuse Podman and Nix caches where possible.

Open an interactive shell with the current checkout mounted into the container:

```bash
podman run --rm -it \
  -v "$(pwd)":/repo:Z \
  -w /repo \
  --entrypoint sh \
  git-janitor
```

Inside that shell, run Cargo through the Nix development environment:

```bash
nix develop --command rustc --version
nix develop --command cargo --version
nix develop --command cargo check --locked
nix develop --command cargo test --locked
nix develop --command cargo build --release --locked
nix develop --command ./target/release/git-janitor
```

Exit the container shell with `exit`.

## Common commands

Build the container and Nix package:

```bash
make build
```

Run the CLI against the current directory:

```bash
make run
```

Run the Nix checks:

```bash
make test
```

Copy the Nix-built release executable to the host:

```bash
make extract-binary
./target/bin/git-janitor
```

The executable is copied to `target/bin/git-janitor`.

## Docker

Docker can be used in place of Podman. Replace `podman` with `docker` in the
shell command above and remove the `:Z` suffix from the volume mount.
