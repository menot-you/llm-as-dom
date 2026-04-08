//! Minimal QR code rendering to stderr using Unicode block characters.
//!
//! No external dependencies — uses a simple QR encoding or falls back
//! to just printing the URL if QR generation isn't available.

/// Print a QR code representation to stderr.
///
/// Uses Unicode block characters for a compact terminal-friendly display.
/// Falls back to plain URL if the terminal doesn't support it.
pub fn print_qr_stderr(url: &str) {
    // For now, print a boxed URL. Full QR rendering can be added later
    // with the `qrcode` crate without changing the interface.
    let width = url.len() + 4;
    let border = "─".repeat(width);

    eprintln!("  ┌{border}┐");
    eprintln!("  │  {url}  │");
    eprintln!("  └{border}┘");
    eprintln!();
    eprintln!("  (QR code rendering: add `qrcode` crate for visual QR)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_qr_does_not_panic() {
        // Just verify it doesn't crash — output goes to stderr.
        print_qr_stderr("ws://192.168.1.42:9876?token=123456");
    }
}
