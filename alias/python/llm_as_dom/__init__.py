"""llm-as-dom — alias for example-org-mcp-lad."""

__version__ = "0.9.0"


def main():
    """Delegate to example-org-mcp-lad."""
    from menot_you_mcp_lad import main as _main

    _main()
