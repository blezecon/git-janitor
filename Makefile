# Detect container engine: prefer podman, fallback to docker
CONTAINER_ENGINE := $(shell which podman 2>/dev/null || which docker 2>/dev/null)

IMAGE_NAME := git-janitor
OUTPUT_DIR := target/bin
DIST_DIR := target/dist
CARGO_VERSION := $(shell grep '^version =' Cargo.toml | head -1 | cut -d '"' -f 2)
VERSION ?= v$(CARGO_VERSION)

# Supported release targets
TARGET_LINUX_X86_64    := x86_64-unknown-linux-gnu
TARGET_LINUX_AARCH64   := aarch64-unknown-linux-gnu
TARGET_WINDOWS_X86_64  := x86_64-pc-windows-gnu
TARGET_WINDOWS_AARCH64 := aarch64-pc-windows-gnullvm
TARGET_MACOS_X86_64    := x86_64-apple-darwin
TARGET_MACOS_AARCH64   := aarch64-apple-darwin
TARGET_FREEBSD_X86_64  := x86_64-unknown-freebsd

ALL_TARGETS := $(TARGET_LINUX_X86_64) $(TARGET_LINUX_AARCH64) $(TARGET_WINDOWS_X86_64) $(TARGET_WINDOWS_AARCH64) $(TARGET_MACOS_X86_64) $(TARGET_MACOS_AARCH64) $(TARGET_FREEBSD_X86_64)

.PHONY: all build test fmt-check clippy check-all extract-binary run build-target build-all-targets package-all release clean help

all: build

# Build the container image (runs nix build internally)
build:
	@if [ -z "$(CONTAINER_ENGINE)" ]; then \
		echo "Error: Neither podman nor docker is installed."; exit 1; \
	fi
	$(CONTAINER_ENGINE) build -t $(IMAGE_NAME) .

# Run test suite & hermetic checks inside container
test: build
	$(CONTAINER_ENGINE) run --rm $(IMAGE_NAME) nix flake check

# Run code formatting check inside container via Nix devShell
fmt-check: build
	$(CONTAINER_ENGINE) run --rm -v $(PWD):/src:Z -w /src $(IMAGE_NAME) \
		nix develop --command cargo fmt --check

# Run Clippy linter inside container via Nix devShell
clippy: build
	$(CONTAINER_ENGINE) run --rm -v $(PWD):/src:Z -w /src $(IMAGE_NAME) \
		nix develop --command cargo clippy -- -D warnings

# Run all checks (flake check + tests + fmt + clippy)
check-all: test fmt-check clippy

# Extract the compiled reproducible binary out to the host
extract-binary: build
	mkdir -p $(OUTPUT_DIR)
	$(CONTAINER_ENGINE) run --rm -v $(PWD)/$(OUTPUT_DIR):/host-out:Z $(IMAGE_NAME) cp /src/result/bin/git-janitor /host-out/
	@echo "Binary extracted to $(OUTPUT_DIR)/git-janitor"

# Run the tool against the current directory
run: build
	$(CONTAINER_ENGINE) run --rm -it -v $(PWD):/repo:Z -w /repo $(IMAGE_NAME)

# Cross-compile a specific target using Nix-provided toolchain
build-target: build
	@if [ -z "$(TARGET)" ]; then \
		echo "Error: TARGET is required. Example: make build-target TARGET=x86_64-unknown-linux-gnu"; exit 1; \
	fi
	$(CONTAINER_ENGINE) run --rm -v $(PWD):/src:Z -w /src $(IMAGE_NAME) \
		nix develop --command cargo zigbuild --target $(TARGET) --release
	@echo "Built target: $(TARGET)"

# Cross-compile all supported release targets
build-all-targets: build
	$(CONTAINER_ENGINE) run --rm -v $(PWD):/src:Z -w /src $(IMAGE_NAME) \
		nix develop --command sh -c "\
			cargo zigbuild --target $(TARGET_LINUX_X86_64) --release && \
			cargo zigbuild --target $(TARGET_LINUX_AARCH64) --release && \
			cargo zigbuild --target $(TARGET_WINDOWS_X86_64) --release && \
			cargo zigbuild --target $(TARGET_WINDOWS_AARCH64) --release && \
			cargo zigbuild --target $(TARGET_MACOS_X86_64) --release && \
			cargo zigbuild --target $(TARGET_MACOS_AARCH64) --release && \
			cargo zigbuild --target $(TARGET_FREEBSD_X86_64) --release"
	@echo "All targets built successfully."

# Package all target binaries into release archives with SHA256 checksums
package-all: build-all-targets
	mkdir -p $(DIST_DIR)
	$(CONTAINER_ENGINE) run --rm -v $(PWD):/src:Z -w /src $(IMAGE_NAME) \
		nix develop --command sh -c "\
			rm -rf $(DIST_DIR)/* && \
			tar -czf $(DIST_DIR)/git-janitor-$(VERSION)-linux-x86_64.tar.gz -C target/$(TARGET_LINUX_X86_64)/release git-janitor && \
			tar -czf $(DIST_DIR)/git-janitor-$(VERSION)-linux-aarch64.tar.gz -C target/$(TARGET_LINUX_AARCH64)/release git-janitor && \
			zip -j $(DIST_DIR)/git-janitor-$(VERSION)-windows-x86_64.zip target/$(TARGET_WINDOWS_X86_64)/release/git-janitor.exe && \
			zip -j $(DIST_DIR)/git-janitor-$(VERSION)-windows-aarch64.zip target/$(TARGET_WINDOWS_AARCH64)/release/git-janitor.exe && \
			tar -czf $(DIST_DIR)/git-janitor-$(VERSION)-macos-x86_64.tar.gz -C target/$(TARGET_MACOS_X86_64)/release git-janitor && \
			tar -czf $(DIST_DIR)/git-janitor-$(VERSION)-macos-aarch64.tar.gz -C target/$(TARGET_MACOS_AARCH64)/release git-janitor && \
			tar -czf $(DIST_DIR)/git-janitor-$(VERSION)-freebsd-x86_64.tar.gz -C target/$(TARGET_FREEBSD_X86_64)/release git-janitor && \
			cd $(DIST_DIR) && sha256sum git-janitor-$(VERSION)-* > SHA256SUMS"
	@echo "Release artifacts packaged in $(DIST_DIR):"
	@ls -la $(DIST_DIR)

# Full release workflow: build all targets, package archives, compute checksums
release: package-all

# Clean build artifacts and container image
clean:
	rm -rf target
	-$(CONTAINER_ENGINE) rmi $(IMAGE_NAME) 2>/dev/null || true

help:
	@echo "git-janitor build targets:"
	@echo "  make build             - Build container and compile native binary with Nix"
	@echo "  make test              - Run cargo tests inside the Nix sandbox"
	@echo "  make fmt-check         - Check code formatting inside Nix sandbox"
	@echo "  make clippy            - Run clippy linter inside Nix sandbox"
	@echo "  make check-all         - Run tests, format check, and clippy"
	@echo "  make extract-binary    - Copy native binary to ./target/bin/git-janitor"
	@echo "  make run               - Run git-janitor container on the current repo"
	@echo "  make build-target      - Cross-compile specific target (TARGET=<triple>)"
	@echo "  make build-all-targets - Cross-compile all 7 supported targets"
	@echo "  make package-all       - Package all targets into tar.gz/zip with SHA256SUMS"
	@echo "  make release           - Build and package complete multi-platform release"
	@echo "  make clean             - Remove build artifacts and container image"