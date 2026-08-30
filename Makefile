# Detect container engine: prefer podman, fallback to docker
CONTAINER_ENGINE := $(shell which podman 2>/dev/null || which docker 2>/dev/null)

IMAGE_NAME := git-janitor
OUTPUT_DIR := target/bin

.PHONY: all build test extract-binary run clean help

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

# Extract the compiled reproducible binary out to the host
extract-binary: build
	mkdir -p $(OUTPUT_DIR)
	$(CONTAINER_ENGINE) run --rm -v $(PWD)/$(OUTPUT_DIR):/host-out:Z $(IMAGE_NAME) cp /src/result/bin/git-janitor /host-out/
	@echo "Binary extracted to $(OUTPUT_DIR)/git-janitor"

# Run the tool against the current directory
run: build
	$(CONTAINER_ENGINE) run --rm -it -v $(PWD):/repo:Z -w /repo $(IMAGE_NAME)

# Clean build artifacts and container image
clean:
	rm -rf $(OUTPUT_DIR)
	-$(CONTAINER_ENGINE) rmi $(IMAGE_NAME) 2>/dev/null || true

help:
	@echo "git-janitor build targets:"
	@echo "  make build          - Build container and compile binary with Nix"
	@echo "  make test           - Run cargo tests inside the Nix sandbox"
	@echo "  make extract-binary - Copy compiled binary to ./target/bin/git-janitor"
	@echo "  make run            - Run git-janitor container on the current repo"