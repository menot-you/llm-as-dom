"""llm-as-dom — alias for menot-you-mcp-lad."""

__version__ = "0.10.0"


def main():
    """Delegate to menot-you-mcp-lad."""
    from menot_you_mcp_lad import main as _main

    _main()
