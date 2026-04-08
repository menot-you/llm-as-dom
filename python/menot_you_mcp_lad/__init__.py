"""example-org-mcp-lad: AI browser pilot MCP server binary wrapper."""

import os
import platform
import stat
import subprocess
import sys
import urllib.request

__version__ = "0.9.0"

BINARY = "llm-as-dom-mcp"
REPO = "example-org/llm-as-dom"


def _get_platform() -> str:
    system = platform.system().lower()
    machine = platform.machine().lower()

    if system == "darwin" and machine in ("arm64", "aarch64"):
        return "macos-arm64"
    if system == "linux" and machine in ("x86_64", "amd64"):
        return "linux-x86_64"

    raise RuntimeError(
        f"Unsupported platform: {system}-{machine}. "
        f"Supported: darwin-arm64, linux-x86_64. "
        f"Install from source: cargo install example-org-mcp-lad"
    )


def _binary_path() -> str:
    return os.path.join(os.path.dirname(__file__), BINARY)


def _ensure_binary() -> str:
    path = _binary_path()
    if os.path.isfile(path) and os.access(path, os.X_OK):
        return path

    plat = _get_platform()
    artifact = f"{BINARY}-{plat}"
    url = f"https://github.com/{REPO}/releases/download/v{__version__}/{artifact}"

    print(f"mcp-lad: downloading {BINARY} v{__version__} for {plat}...", file=sys.stderr)

    try:
        urllib.request.urlretrieve(url, path)
        st = os.stat(path)
        os.chmod(path, st.st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
        size_mb = os.path.getsize(path) / 1024 / 1024
        print(f"mcp-lad: installed {BINARY} ({size_mb:.1f} MB)", file=sys.stderr)
    except Exception as e:
        print(f"mcp-lad: failed to download — {e}", file=sys.stderr)
        print("mcp-lad: install from source: cargo install example-org-mcp-lad", file=sys.stderr)
        sys.exit(1)

    return path


def main():
    binary = _ensure_binary()
    result = subprocess.run([binary] + sys.argv[1:], env=os.environ)
    sys.exit(result.returncode)
