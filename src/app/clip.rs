//! Copying and pasting text, in a file and in the commit message.
//!
//! The browser copies *files* (`Ctrl+C` there is a path, see `fileops`); a
//! pane that types copies *text*, through [`crate::clipboard`], which is the
//! system's clipboard wherever it can be reached. With nothing selected, a
//! copy or a cut takes the whole of every line a cursor is on, and pastes
//! back in above the line, the way most editors do it.
//!
//! A paste can also arrive from the terminal itself (its own paste key, or a
//! right click) as one event holding all of the text, which goes in exactly
//! as it was given: no indentation added, however many lines.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::keys::Action;
use crate::text::editor::Editor;

use super::App;
use super::mode::{Focus, Mode, Settings, Window};
use super::parts::plural;
use super::source::GitFocus;

impl App {
    /// The text the keyboard is typing into: the file in the editor, or the
    /// commit message. `None` anywhere else.
    fn typing_into(&mut self) -> Option<&mut Editor> {
        match self.window {
            Window::Source if self.git.focus == GitFocus::Message => self.git.message.as_mut(),
            Window::Main if self.focus == Focus::Editor => self.active_buffer_mut(),
            _ => None,
        }
    }

    /// `Ctrl+C`, or with `cut`, `Ctrl+X`: what is selected, or with nothing
    /// selected the whole line, onto the clipboard.
    pub(super) fn copy_text(&mut self, cut: bool) {
        let Some(ed) = self.typing_into() else {
            return;
        };
        let (text, whole) = if cut {
            ed.cut()
        } else {
            match ed.selected_text() {
                Some(text) => (text, false),
                None => (ed.cursor_lines_text(), true),
            }
        };
        let verb = if cut { "cut" } else { "copied" };
        self.status = format!("{verb} {}", amount(&text, whole));
        self.text_clipboard.copy(text, whole);
    }

    /// `Ctrl+V` in a file: what was copied, at every cursor.
    pub(super) fn paste_text(&mut self) {
        let Some((text, whole)) = self.text_clipboard.paste() else {
            self.status = format!(
                "nothing copied, {} copies",
                self.key_name(Action::EditorCopy)
            );
            return;
        };
        if let Some(ed) = self.typing_into() {
            ed.paste(&text, whole);
        }
    }

    /// Text the terminal pasted, all at once.
    ///
    /// Into a file it goes as it is, see the module docs. Into the bar or a
    /// setting being typed it is typed, as one line, since that is all those
    /// hold. Anywhere else there is nothing to put it in.
    pub fn on_paste(&mut self, text: String) {
        self.record_paste(&text);
        if self.focus == Focus::Terminal && matches!(self.mode, Mode::Normal) {
            return self.paste_into_terminal(&text);
        }
        let field = match &self.mode {
            Mode::Bar(_) => true,
            Mode::Settings(Settings {
                editing: Some(_), ..
            }) => true,
            Mode::Normal => false,
            _ => return,
        };
        if field {
            // Typed a character at a time, through the keys the field already
            // answers, so it does everything typing does. Not recorded twice
            // over: the paste itself was.
            let recording = self.recording.take();
            for c in text.lines().collect::<Vec<_>>().join(" ").chars() {
                self.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            }
            self.recording = recording;
            return;
        }
        let whole = self.text_clipboard.whole_lines(&text);
        if let Some(ed) = self.typing_into() {
            ed.paste(&text, whole);
        }
    }
}

/// How much was copied, for the status line.
fn amount(text: &str, whole: bool) -> String {
    let lines = text
        .strip_suffix('\n')
        .unwrap_or(text)
        .matches('\n')
        .count()
        + 1;
    if whole || lines > 1 {
        format!("{lines} line{}", plural(lines))
    } else {
        let n = text.chars().count();
        format!("{n} character{}", plural(n))
    }
}

#[cfg(test)]
mod tests {
    use super::amount;

    #[test]
    fn the_amount_is_lines_or_characters() {
        assert_eq!(amount("one\n", true), "1 line");
        assert_eq!(amount("one\ntwo\n", true), "2 lines");
        assert_eq!(amount("a\nb", false), "2 lines");
        assert_eq!(amount("word", false), "4 characters");
        assert_eq!(amount("é", false), "1 character");
    }
}
