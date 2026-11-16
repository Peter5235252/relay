//! Tiny standalone companion to Relay's interactive chat.
//!
//! `relay-clear` clears the visible screen and scrollback so a long relay-cli
//! session can start clean without opening the database or keyring.

use std::io::Write as _;

fn main() {
    print!("\x1b[2J\x1b[3J\x1b[H");
    let _ = std::io::stdout().flush();
}
