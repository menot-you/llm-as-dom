//! Terminal QR code rendering using Unicode half-block characters.
//!
//! Renders a scannable QR code to stderr so users can pair their iPhone
//! with the lad-relay by pointing their camera at the terminal.

use qrcode::{EcLevel, QrCode};

/// Print a QR code to stderr using Unicode half-block characters.
///
/// Each character cell encodes two vertical modules using:
/// - `\u{2588}` (full block) = both dark
/// - `\u{2580}` (upper half)  = top dark, bottom light
/// - `\u{2584}` (lower half)  = top light, bottom dark
/// - ` ` (space)              = both light
///
/// Includes a quiet zone border for reliable scanning.
pub fn print_qr_stderr(url: &str) {
    let code = match QrCode::with_error_correction_level(url, EcLevel::L) {
        Ok(c) => c,
        Err(e) => {
            // Fallback: just print the URL if QR generation fails.
            eprintln!("  (QR generation failed: {e})");
            eprintln!("  {url}");
            return;
        }
    };

    let modules = code.to_colors();
    let width = code.width() as usize;

    // Quiet zone: 2 modules on each side.
    let qz = 2;
    let total_w = width + qz * 2;

    // Collect rows with quiet zone.
    let mut rows: Vec<Vec<bool>> = Vec::new();

    // Top quiet zone.
    for _ in 0..qz {
        rows.push(vec![false; total_w]);
    }

    // Data rows.
    for y in 0..width {
        let mut row = vec![false; qz];
        for x in 0..width {
            row.push(modules[y * width + x].select(true, false));
        }
        row.extend(std::iter::repeat_n(false, qz));
        rows.push(row);
    }

    // Bottom quiet zone.
    for _ in 0..qz {
        rows.push(vec![false; total_w]);
    }

    // Render pairs of rows as half-block characters.
    // Invert colors: dark modules = light terminal pixels (for dark terminals).
    let height = rows.len();
    let mut y = 0;
    while y < height {
        let top = &rows[y];
        let bottom = if y + 1 < height {
            &rows[y + 1]
        } else {
            top // Duplicate last row if odd height.
        };

        eprint!("  "); // Left margin.
        for x in 0..total_w {
            let t = top[x]; // true = dark module
            let b = bottom[x];
            // Invert for dark terminals: dark module → space (background), light → block.
            match (t, b) {
                (true, true) => eprint!(" "),          // Both dark → background
                (true, false) => eprint!("\u{2584}"),  // Top dark, bottom light → lower half
                (false, true) => eprint!("\u{2580}"),  // Top light, bottom dark → upper half
                (false, false) => eprint!("\u{2588}"), // Both light → full block
            }
        }
        eprintln!();
        y += 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_qr_does_not_panic() {
        print_qr_stderr("ws://192.168.1.42:9876?token=123456");
    }

    #[test]
    fn qr_code_encodes_ws_url() {
        let url = "ws://192.168.1.42:9876?token=654321";
        let code = QrCode::with_error_correction_level(url, EcLevel::L);
        assert!(
            code.is_ok(),
            "QR code generation should succeed for ws:// URL"
        );
        let code = code.unwrap();
        assert!(code.width() > 0);
    }

    #[test]
    fn qr_code_encodes_long_url() {
        let url = "ws://192.168.1.42:9876?token=999999&extra=longparam";
        let code = QrCode::with_error_correction_level(url, EcLevel::L);
        assert!(code.is_ok());
    }
}
