#!/usr/bin/env bash
# llm-as-dom installer
# Usage: curl -fsSL https://raw.githubusercontent.com/menot-you/llm-as-dom/main/install.sh | sh
set -euo pipefail

REPO="menot-you/llm-as-dom"
BINARIES="lad llm-as-dom-mcp"
INSTALL_DIR="${HOME}/.local/bin"

# ── Platform detection ─────────────────────────────────────────────
detect_platform() {
    local os arch
    os="$(uname -s | tr '[:upper:]' '[:lower:]')"
    arch="$(uname -m)"

    case "${os}-${arch}" in
        darwin-arm64)   echo "macos-arm64" ;;
        linux-x86_64)   echo "linux-x86_64" ;;
        linux-amd64)    echo "linux-x86_64" ;;
        *)
            echo "Error: unsupported platform ${os}-${arch}" >&2
            echo "Supported: darwin-arm64, linux-x86_64" >&2
            echo "Install from source: cargo install llm-as-dom" >&2
            exit 1
            ;;
    esac
}

# ── Latest version ─────────────────────────────────────────────────
get_latest_version() {
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' \
        | head -1 \
        | sed 's/.*"tag_name": *"v\([^"]*\)".*/\1/'
}

# ── Main ───────────────────────────────────────────────────────────
main() {
    local platform version

    echo "llm-as-dom installer"
    echo "────────────────────"

    platform="$(detect_platform)"
    echo "Platform: ${platform}"

    version="$(get_latest_version)"
    if [ -z "${version}" ]; then
        echo "Error: could not determine latest version" >&2
        echo "Check https://github.com/${REPO}/releases" >&2
        exit 1
    fi
    echo "Version:  v${version}"

    mkdir -p "${INSTALL_DIR}"

    for bin in ${BINARIES}; do
        local url artifact dest
        artifact="${bin}-${platform}"
        url="https://github.com/${REPO}/releases/download/v${version}/${artifact}"
        dest="${INSTALL_DIR}/${bin}"

        echo ""
        echo "Downloading ${bin}..."
        if curl -fsSL -o "${dest}" "${url}"; then
            chmod +x "${dest}"
            echo "  Installed: ${dest}"
        else
            echo "  Warning: failed to download ${bin} (${url})" >&2
            echo "  This binary may not be available for ${platform} yet." >&2
        fi
    done

    echo ""
    echo "Done! Installed to ${INSTALL_DIR}"
    echo ""

    # Check if INSTALL_DIR is in PATH
    if ! echo "${PATH}" | tr ':' '\n' | grep -qx "${INSTALL_DIR}"; then
        echo "Add to your PATH:"
        echo "  export PATH=\"${INSTALL_DIR}:\${PATH}\""
        echo ""
        echo "Add this to your ~/.bashrc or ~/.zshrc to make it permanent."
    else
        echo "Verify:"
        echo "  lad --help"
        echo "  llm-as-dom-mcp --help"
    fi
}

main "$@"
