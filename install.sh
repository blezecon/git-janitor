#!/bin/sh
# git-janitor (git-jan) universal POSIX installer for Linux, macOS, and FreeBSD.
# Repository: https://github.com/blezecon/git-janitor
#
# Quick Install:
#   curl -fsSL https://raw.githubusercontent.com/blezecon/git-janitor/release/install.sh | sh
#
# Uninstall:
#   curl -fsSL https://raw.githubusercontent.com/blezecon/git-janitor/release/install.sh | sh -s -- --uninstall

set -e

REPO="blezecon/git-janitor"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
BIN_NAME="git-janitor"
ALIAS_NAME="git-jan"

# Colors and formatting
setup_colors() {
    if [ -t 1 ]; then
        RED='\033[0;31m'
        GREEN='\033[0;32m'
        YELLOW='\033[1;33m'
        BLUE='\033[0;34m'
        CYAN='\033[0;36m'
        BOLD='\033[1m'
        DIM='\033[2m'
        NC='\033[0m'
    else
        RED=''
        GREEN=''
        YELLOW=''
        BLUE=''
        CYAN=''
        BOLD=''
        DIM=''
        NC=''
    fi
}

info() {
    printf "${BLUE}${BOLD}==>${NC} %s\n" "$1"
}

success() {
    printf "${GREEN}${BOLD}✓${NC} %s\n" "$1"
}

warn() {
    printf "${YELLOW}${BOLD}⚠${NC} %s\n" "$1"
}

error() {
    printf "\r\033[K${RED}${BOLD}✗ Error:${NC} %s\n" "$1" >&2
    exit 1
}

# Uninstall routine
uninstall() {
    setup_colors
    info "Uninstalling git-janitor from ${INSTALL_DIR}..."
    if [ -f "${INSTALL_DIR}/${BIN_NAME}" ] || [ -L "${INSTALL_DIR}/${BIN_NAME}" ]; then
        rm -f "${INSTALL_DIR}/${BIN_NAME}"
        success "Removed ${INSTALL_DIR}/${BIN_NAME}"
    fi
    if [ -f "${INSTALL_DIR}/${ALIAS_NAME}" ] || [ -L "${INSTALL_DIR}/${ALIAS_NAME}" ]; then
        rm -f "${INSTALL_DIR}/${ALIAS_NAME}"
        success "Removed ${INSTALL_DIR}/${ALIAS_NAME}"
    fi
    success "git-janitor has been completely uninstalled."
    exit 0
}

# Detect OS
detect_os() {
    case "$(uname -s)" in
        Linux*)  OS="linux" ;;
        Darwin*) OS="macos" ;;
        FreeBSD*) OS="freebsd" ;;
        *) error "Unsupported operating system: $(uname -s). Supported: Linux, macOS, FreeBSD" ;;
    esac
}

# Detect Architecture
detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)  ARCH="x86_64" ;;
        aarch64|arm64) ARCH="aarch64" ;;
        *) error "Unsupported CPU architecture: $(uname -m). Supported: x86_64, aarch64" ;;
    esac

    if [ "$OS" = "freebsd" ] && [ "$ARCH" != "x86_64" ]; then
        error "FreeBSD currently only supports x86_64"
    fi
}

# Find download tool (curl or wget)
detect_downloader() {
    if command -v curl >/dev/null 2>&1; then
        DOWNLOADER="curl"
    elif command -v wget >/dev/null 2>&1; then
        DOWNLOADER="wget"
    else
        error "Neither curl nor wget is installed. Please install curl or wget."
    fi
}

download_text() {
    url="$1"
    if [ "$DOWNLOADER" = "curl" ]; then
        curl -fsSL "$url" 2>/dev/null || true
    else
        wget -qO- "$url" 2>/dev/null || true
    fi
}

# Animated downloader with smooth spinner
download_with_animation() {
    url="$1"
    output="$2"
    label="$3"

    if [ "$DOWNLOADER" = "curl" ]; then
        curl -fsSL "$url" -o "$output" 2>/dev/null &
    else
        wget -qO "$output" "$url" 2>/dev/null &
    fi
    pid=$!

    if [ -t 1 ]; then
        # Unicode braille spinner frames
        frames="⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏"
        while kill -0 "$pid" 2>/dev/null; do
            for frame in $frames; do
                printf "\r${CYAN}${BOLD}%s${NC} %s ${DIM}(downloading...)${NC}" "$frame" "$label"
                sleep 0.08
                if ! kill -0 "$pid" 2>/dev/null; then
                    break
                fi
            done
        done
        printf "\r\033[K"
    fi

    wait "$pid" || error "Failed to download release archive from ${url}"
    success "Downloaded ${label}"
}

# Resolve latest release version
get_latest_version() {
    LATEST_JSON=$(download_text "https://api.github.com/repos/${REPO}/releases/latest")
    VERSION=$(echo "$LATEST_JSON" | grep '"tag_name":' | head -n 1 | sed -E 's/.*"([^"]+)".*/\1/')

    if [ -z "$VERSION" ]; then
        VERSION="v0.1.0"
    fi
}

# Configure Shell PATH persistence
configure_path() {
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            warn "${INSTALL_DIR} is not currently in your PATH."
            CURRENT_SHELL="$(basename "$SHELL" 2>/dev/null || echo "sh")"
            RC_FILE=""
            case "$CURRENT_SHELL" in
                zsh)  RC_FILE="$HOME/.zshrc" ;;
                bash)
                    if [ -f "$HOME/.bashrc" ]; then
                        RC_FILE="$HOME/.bashrc"
                    else
                        RC_FILE="$HOME/.bash_profile"
                    fi
                    ;;
                *)    RC_FILE="$HOME/.profile" ;;
            esac

            if [ -n "$RC_FILE" ]; then
                EXPORT_CMD="export PATH=\"\$HOME/.local/bin:\$PATH\""
                if ! grep -qs "$EXPORT_CMD" "$RC_FILE" 2>/dev/null; then
                    printf "\n# Added by git-janitor installer\n%s\n" "$EXPORT_CMD" >> "$RC_FILE"
                    info "Added ${INSTALL_DIR} to ${RC_FILE}."
                    printf "${DIM}  Run ${BOLD}source %s${NC}${DIM} or restart your shell to apply.${NC}\n" "$RC_FILE"
                fi
            fi
            ;;
    esac
}

# Main installer routine
main() {
    setup_colors

    for arg in "$@"; do
        case "$arg" in
            --uninstall|-u) uninstall ;;
            --help|-h)
                echo "git-janitor universal installer"
                echo "Usage: install.sh [OPTIONS]"
                echo "Options:"
                echo "  --uninstall, -u   Remove git-janitor binary and aliases"
                echo "  --help, -h        Show this help message"
                exit 0
                ;;
        esac
    done

    printf "\n${BOLD}${BLUE}🧹 git-janitor (git-jan) Installer${NC}\n\n"

    detect_os
    detect_arch
    detect_downloader
    get_latest_version

    info "Target Platform: ${BOLD}${OS}-${ARCH}${NC} (${VERSION})"

    mkdir -p "${INSTALL_DIR}"

    ARCHIVE_NAME="git-janitor-${VERSION}-${OS}-${ARCH}.tar.gz"
    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE_NAME}"
    TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t 'git-janitor')"
    trap 'rm -rf "$TMP_DIR"' EXIT INT TERM

    download_with_animation "${DOWNLOAD_URL}" "${TMP_DIR}/${ARCHIVE_NAME}" "${ARCHIVE_NAME}"

    info "Extracting binary..."
    tar -xzf "${TMP_DIR}/${ARCHIVE_NAME}" -C "${TMP_DIR}"

    if [ ! -f "${TMP_DIR}/${BIN_NAME}" ]; then
        error "Archive did not contain ${BIN_NAME} executable"
    fi

    mv "${TMP_DIR}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
    chmod +x "${INSTALL_DIR}/${BIN_NAME}"

    # Create git-jan alias symlink
    ln -sf "${INSTALL_DIR}/${BIN_NAME}" "${INSTALL_DIR}/${ALIAS_NAME}"

    success "Installed binaries to ${INSTALL_DIR}"

    configure_path

    printf "\n${GREEN}${BOLD}✨ git-janitor successfully installed!${NC}\n"
    printf "You can now run:\n"
    printf "  ${CYAN}${BOLD}git-janitor --help${NC}\n"
    printf "  ${CYAN}${BOLD}git-jan --help${NC}\n"
    printf "  ${CYAN}${BOLD}git jan --help${NC}\n\n"
}

main "$@"
