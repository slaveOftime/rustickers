//! Controlling-terminal I/O: clipboard selection never consumes child stdin or pollutes stdout.
use std::fs::{File, OpenOptions};
use std::io::Write;

use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use zeroize::Zeroizing;

use crate::clipboard::{filter::filter_indices, items::ClipboardItem};

/// The app's primary color, rgb(255, 218, 60) — the same yellow as the desktop `Button::primary()`
/// and the `Icon::Locked` accent — so the terminal chooser reads as the same product.
const PRIMARY: Color = Color::Rgb {
    r: 255,
    g: 218,
    b: 60,
};

struct Terminal {
    output: File,
    alternate: bool,
    raw: bool,
    #[cfg(windows)]
    original_code_page: u32,
}

impl Terminal {
    fn open(alternate: bool) -> Result<Self> {
        #[cfg(windows)]
        let path = "CONOUT$";
        #[cfg(not(windows))]
        let path = "/dev/tty";
        let output = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .context("clipboard selection needs a terminal; use --list or --id <ID> for scripts")?;
        let mut terminal = Self {
            output,
            alternate: false,
            raw: false,
            #[cfg(windows)]
            original_code_page: unsafe {
                windows_sys::Win32::System::Console::GetConsoleOutputCP()
            },
        };
        #[cfg(windows)]
        unsafe {
            if windows_sys::Win32::System::Console::SetConsoleOutputCP(65001) == 0 {
                return Err(std::io::Error::last_os_error())
                    .context("enable UTF-8 terminal output");
            }
        }
        if !terminal::is_raw_mode_enabled()? {
            terminal::enable_raw_mode().context("enable terminal input")?;
            terminal.raw = true;
        }
        if alternate {
            terminal.alternate = true;
            execute!(
                terminal.output,
                EnterAlternateScreen,
                Hide,
                EnableBracketedPaste
            )?;
        }
        Ok(terminal)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if self.alternate {
            let _ = execute!(
                self.output,
                ResetColor,
                SetAttribute(Attribute::Reset),
                Show,
                DisableBracketedPaste,
                LeaveAlternateScreen
            );
        }
        #[cfg(windows)]
        if self.original_code_page != 0 {
            unsafe {
                windows_sys::Win32::System::Console::SetConsoleOutputCP(self.original_code_page);
            }
        }
        if self.raw {
            let _ = terminal::disable_raw_mode();
        }
    }
}

#[derive(Default)]
struct Selection {
    query: String,
    selected: usize,
}

impl Selection {
    fn move_by(&mut self, delta: isize, count: usize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(count.saturating_sub(1));
    }

    fn query_changed(&mut self) {
        self.selected = 0;
    }
}

pub(super) fn choose(rows: &[ClipboardItem], initial_query: &str) -> Result<Option<i64>> {
    let mut terminal = Terminal::open(true)?;
    let mut selection = Selection {
        query: initial_query.to_owned(),
        selected: 0,
    };
    loop {
        let matches = filter_indices(rows, &selection.query);
        let (width, height) = terminal::size().unwrap_or((80, 24));
        let page_size = usize::from(height.saturating_sub(6)).max(1);
        selection.selected = selection.selected.min(matches.len().saturating_sub(1));
        let colored = std::env::var_os("NO_COLOR").is_none();
        queue!(terminal.output, MoveTo(0, 0), Clear(ClearType::All))?;
        if colored {
            queue!(
                terminal.output,
                SetForegroundColor(PRIMARY),
                SetAttribute(Attribute::Bold)
            )?;
        }
        queue!(
            terminal.output,
            Print(" Clipboard history"),
            ResetColor,
            SetAttribute(Attribute::Reset)
        )?;
        queue!(
            terminal.output,
            MoveTo(0, 1),
            Print(format!(
                " Search  {}",
                safe_preview(&selection.query, usize::from(width.saturating_sub(10)))
            ))
        )?;
        let start = selection.selected / page_size * page_size;
        if matches.is_empty() {
            queue!(
                terminal.output,
                MoveTo(1, 3),
                Print("No matches. Backspace to refine; Ctrl+U to clear.")
            )?;
        }
        for (offset, index) in matches.iter().skip(start).take(page_size).enumerate() {
            let row = &rows[*index];
            let selected = start + offset == selection.selected;
            let badge = if row.retention.protected_ {
                "  [🔒 Protected]"
            } else if row.retention.explicit {
                "  [Reserved]"
            } else if row.retention.edited {
                "  [Edited]"
            } else {
                ""
            };
            let available = usize::from(width.saturating_sub(5)).saturating_sub(badge.len());
            let preview = safe_preview(&row.preview_text(), available);
            queue!(terminal.output, MoveTo(0, (offset + 3) as u16))?;
            if selected {
                queue!(terminal.output, SetAttribute(Attribute::Reverse))?;
            }
            queue!(
                terminal.output,
                Print(format!(
                    " {} {}{}",
                    if selected { "›" } else { " " },
                    preview,
                    badge
                )),
                SetAttribute(Attribute::Reset)
            )?;
        }
        if height > 1 {
            queue!(terminal.output, MoveTo(0, height - 1))?;
            if colored {
                queue!(terminal.output, SetForegroundColor(Color::DarkGrey))?;
            }
            queue!(
                terminal.output,
                Print(safe_preview(
                    &format!(
                        " ↑↓ choose · Enter use · Esc cancel · {} match{}",
                        matches.len(),
                        if matches.len() == 1 { "" } else { "es" }
                    ),
                    usize::from(width.saturating_sub(1))
                )),
                ResetColor
            )?;
        }
        terminal.output.flush()?;
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.code == KeyCode::Esc
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    return Ok(None);
                }
                match key.code {
                    KeyCode::Enter => {
                        if let Some(index) = matches.get(selection.selected) {
                            return Ok(Some(rows[*index].id));
                        }
                    }
                    KeyCode::Up => selection.move_by(-1, matches.len()),
                    KeyCode::Down => selection.move_by(1, matches.len()),
                    KeyCode::PageUp => selection.move_by(-(page_size as isize), matches.len()),
                    KeyCode::PageDown => selection.move_by(page_size as isize, matches.len()),
                    KeyCode::Home => selection.selected = 0,
                    KeyCode::End => selection.selected = matches.len().saturating_sub(1),
                    KeyCode::Backspace => {
                        selection.query.pop();
                        selection.query_changed();
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        selection.query.clear();
                        selection.query_changed();
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        selection.query.push(c);
                        selection.query_changed();
                    }
                    _ => {}
                }
            }
            Event::Paste(text) => {
                selection.query.push_str(&text);
                selection.query_changed();
            }
            _ => {}
        }
    }
}

/// Hidden input on the controlling terminal, never process stdin, argv, or the environment.
pub(super) fn password(prompt: &str) -> Result<Option<Zeroizing<String>>> {
    let mut terminal = Terminal::open(false)?;
    write!(terminal.output, "{prompt}")?;
    terminal.output.flush()?;
    let mut password = Zeroizing::new(String::new());
    loop {
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.code == KeyCode::Esc
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    write!(terminal.output, "\r\n")?;
                    return Ok(None);
                }
                match key.code {
                    KeyCode::Enter => {
                        write!(terminal.output, "\r\n")?;
                        return Ok(Some(password));
                    }
                    KeyCode::Backspace => {
                        password.pop();
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        password.push(c)
                    }
                    _ => {}
                }
            }
            Event::Paste(text) => password.push_str(&text),
            _ => {}
        }
    }
}

/// Strip terminal commands and bidi controls from display text, not from stored/pasted payloads.
pub(super) fn safe_preview(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut count: usize = 0;
    for c in text.chars() {
        let c = if c.is_whitespace() {
            ' '
        } else if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            '�'
        } else {
            c
        };
        // A conservative two-column allowance keeps CJK and emoji inside the viewport.
        let columns = if c.is_ascii() { 1 } else { 2 };
        if count + columns > max {
            while count + 1 > max {
                let Some(last) = out.pop() else {
                    break;
                };
                count = count.saturating_sub(if last.is_ascii() { 1 } else { 2 });
            }
            if max > 0 {
                out.push('…');
            }
            break;
        }
        out.push(c);
        count += columns;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_clamps_and_search_resets_selection() {
        let mut selection = Selection::default();
        selection.move_by(-1, 3);
        assert_eq!(selection.selected, 0);
        selection.move_by(20, 3);
        assert_eq!(selection.selected, 2);
        selection.query_changed();
        assert_eq!(selection.selected, 0);
        selection.move_by(1, 0);
        assert_eq!(selection.selected, 0);
    }

    #[test]
    fn previews_cannot_execute_terminal_sequences() {
        let preview = safe_preview("hello\x1b[2J\nworld\u{202e}", 80);
        assert!(!preview.contains('\x1b'));
        assert!(!preview.contains('\n'));
        assert!(!preview.contains('\u{202e}'));
        assert_eq!(safe_preview("日本語", 4), "日…");
    }
}

#[cfg(test)]
mod interactive_smoke {
    use super::*;
    use crate::clipboard::items::{ClipboardItemKind, RetentionState};

    // Run in a real PTY: cargo test terminal_chooser_smoke -- --ignored --nocapture
    #[test]
    #[ignore = "requires an interactive terminal"]
    fn terminal_chooser_smoke() {
        let rows = ["First copy", "日本語 second copy", "Third copy"]
            .iter()
            .enumerate()
            .map(|(index, text)| ClipboardItem {
                id: index as i64 + 1,
                kind: ClipboardItemKind::Plain,
                content: (*text).into(),
                plain_preview: None,
                public_label: None,
                byte_size: text.len() as i64,
                created_at: 1,
                last_copied_at: 1,
                last_used_at: None,
                last_edited_at: None,
                retention: RetentionState::default(),
            })
            .collect::<Vec<_>>();
        let chosen = choose(&rows, "").unwrap();
        println!("TERMINAL_SELECTION={chosen:?}");
        assert_eq!(chosen, Some(2));
    }
}
