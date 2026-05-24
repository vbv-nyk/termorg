use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Translate a crossterm `KeyEvent` into bytes to send to the PTY.
///
/// Returns `None` for keys we don't handle (e.g. bare modifier presses).
pub fn key_to_action(key: KeyEvent) -> Option<Vec<u8>> {
    // Alt modifier: prepend ESC (0x1b) to whatever bytes the base key produces.
    // This is the standard encoding — Alt+x = ESC x — used by bash readline,
    // vim, emacs, etc.
    let has_alt = key.modifiers.contains(KeyModifiers::ALT);

    // Strip the ALT bit so the inner match can treat the key normally.
    let inner_key = KeyEvent::new(key.code, key.modifiers - KeyModifiers::ALT);

    let base_bytes: Option<Vec<u8>> = key_base_bytes(inner_key);

    if has_alt {
        return base_bytes.map(|mut b| { b.insert(0, 0x1b); b });
    }

    base_bytes
}

fn key_base_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    use KeyCode::*;
    // Ctrl-Space → NUL byte (0x00), the standard terminal encoding.
    if key.code == Char(' ') && key.modifiers == KeyModifiers::CONTROL {
        return Some(vec![0x00]);
    }

    let bytes: Vec<u8> = match key.code {
        // Printable characters (including Shift variants like uppercase).
        Char(c) if key.modifiers == KeyModifiers::NONE
                || key.modifiers == KeyModifiers::SHIFT =>
        {
            c.to_string().into_bytes()
        }

        // Ctrl+letter produces control codes 0x01–0x1a.
        // e.g. Ctrl-C → 0x03, Ctrl-D → 0x04, Ctrl-Z → 0x1a.
        Char(c) if key.modifiers == KeyModifiers::CONTROL => {
            let lower = c.to_ascii_lowercase();
            if lower.is_ascii_lowercase() {
                vec![lower as u8 - b'a' + 1]
            } else {
                return None;
            }
        }

        // Enter sends carriage return, not newline — that's what PTYs expect.
        Enter => vec![b'\r'],

        // Backspace sends DEL (0x7f), not BS (0x08) — modern terminal convention.
        Backspace => vec![0x7f],

        Tab => vec![b'\t'],

        Esc => vec![0x1b],

        Delete => vec![0x1b, b'[', b'3', b'~'],

        // Arrow keys in normal cursor mode (ANSI sequences).
        Up    => vec![0x1b, b'[', b'A'],
        Down  => vec![0x1b, b'[', b'B'],
        Right => vec![0x1b, b'[', b'C'],
        Left  => vec![0x1b, b'[', b'D'],

        Home    => vec![0x1b, b'[', b'H'],
        End     => vec![0x1b, b'[', b'F'],
        PageUp  => vec![0x1b, b'[', b'5', b'~'],
        PageDown => vec![0x1b, b'[', b'6', b'~'],

        // Function keys F1–F4 use SS3 sequences; F5+ use CSI.
        F(1) => vec![0x1b, b'O', b'P'],
        F(2) => vec![0x1b, b'O', b'Q'],
        F(3) => vec![0x1b, b'O', b'R'],
        F(4) => vec![0x1b, b'O', b'S'],
        F(n) => {
            // F5=15, F6=17, F7=18, F8=19, F9=20, F10=21, F11=23, F12=24
            let code: &[u8] = match n {
                5  => b"15",
                6  => b"17",
                7  => b"18",
                8  => b"19",
                9  => b"20",
                10 => b"21",
                11 => b"23",
                12 => b"24",
                _  => return None,
            };
            let mut seq = vec![0x1b, b'['];
            seq.extend_from_slice(code);
            seq.push(b'~');
            seq
        }

        _ => return None,
    };

    Some(bytes)
}

