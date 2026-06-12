//! Watchdog: replicate AMF's view pipeline (capture-pane -e -> strip
//! trailing newline -> \n => \r\n -> vt100 parse) and diff every row
//! against the plain text tmux reports for the same capture. Logs any
//! divergence. Run with:
//!   cargo run --example vtcheck -- <session:window> <cols> <rows>

use std::process::Command;
use std::time::Duration;

fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn main() {
    let mut args = std::env::args().skip(1);
    let target = args.next().unwrap_or_else(|| {
        "amf-agent-mainframe-cc-screen-bug:claude".to_string()
    });
    let cols: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(157);
    let rows: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(43);

    eprintln!("watching {target} at {cols}x{rows}");
    loop {
        let output = Command::new("tmux")
            .args(["capture-pane", "-t", &target, "-e", "-p"])
            .output()
            .expect("tmux capture failed");
        let raw = String::from_utf8_lossy(&output.stdout).into_owned();

        // AMF pipeline: normalize_captured_pane + vt100 parse.
        let stripped = raw.strip_suffix('\n').unwrap_or(&raw);
        let normalized = stripped.replace('\n', "\r\n");
        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser.process(normalized.as_bytes());
        let screen = parser.screen();

        // Reference: same capture with escapes stripped per line.
        let reference: Vec<String> = raw.lines().map(strip_ansi).collect();

        let mut mismatches = Vec::new();
        for r in 0..rows {
            let mut row_text = String::new();
            for c in 0..cols {
                if let Some(cell) = screen.cell(r, c) {
                    row_text.push_str(&cell.contents());
                }
            }
            let vt_row = row_text.trim_end();
            let ref_row = reference
                .get(r as usize)
                .map(|s| s.trim_end())
                .unwrap_or("");
            if vt_row != ref_row {
                mismatches.push((r, ref_row.to_string(), vt_row.to_string()));
            }
        }

        if !mismatches.is_empty() {
            println!(
                "=== {} row mismatch(es) at {}",
                mismatches.len(),
                chrono::Local::now().format("%H:%M:%S%.3f")
            );
            for (r, tmux_row, vt_row) in mismatches.iter().take(8) {
                println!("row {r:2}  tmux : {tmux_row:?}");
                println!("row {r:2}  vt100: {vt_row:?}");
            }
        }

        std::thread::sleep(Duration::from_millis(150));
    }
}
