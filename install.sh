#!/usr/bin/env bash
# llm-as-dom installer
# Usage: curl -fsSL https://llm-as-dom.menot.sh/install.sh | sh
set -euo pipefail

REPO="example-org/llm-as-dom"
BINARIES="lad llm-as-dom-mcp lad-relay"
INSTALL_DIR="${HOME}/.local/bin"

detect_platform() {
    local os arch
    os="$(uname -s | tr '[:upper:]' '[:lower:]')"
    arch="$(uname -m)"
    case "${os}-${arch}" in
        darwin-arm64)   echo "macos-arm64" ;;
        linux-x86_64)   echo "linux-x86_64" ;;
        linux-amd64)    echo "linux-x86_64" ;;
        *) echo "Error: unsupported platform ${os}-${arch}" >&2; exit 1 ;;
    esac
}

get_latest_version() {
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' | head -1 \
        | sed 's/.*"tag_name": *"v\([^"]*\)".*/\1/'
}

main() {
    echo "llm-as-dom installer"
    echo "--------------------"
    local platform="$(detect_platform)"
    echo "Platform: ${platform}"
    local version="$(get_latest_version)"
    if [ -z "${version}" ]; then
        echo "Error: no release found. Check https://github.com/${REPO}/releases" >&2
        exit 1
    fi
    echo "Version:  v${version}"
    mkdir -p "${INSTALL_DIR}"
    for bin in ${BINARIES}; do
        local url="https://github.com/${REPO}/releases/download/v${version}/${bin}-${platform}"
        local dest="${INSTALL_DIR}/${bin}"
        echo ""
        echo "Downloading ${bin}..."
        if curl -fsSL -o "${dest}" "${url}"; then
            chmod +x "${dest}"
            echo "  Installed: ${dest}"
        else
            echo "  Warning: ${bin} not available for ${platform}" >&2
        fi
    done
    echo ""
    echo "Done! Installed to ${INSTALL_DIR}"
    if ! echo "${PATH}" | tr ':' '\n' | grep -qx "${INSTALL_DIR}"; then
        echo "  export PATH=\"${INSTALL_DIR}:\${PATH}\""
    fi
}

main "$@"
