//! Recording keys and playing them back.
//!
//! `Ctrl+R` starts recording, and everything typed from then on is kept, in
//! any pane and any window, until `Ctrl+R` again. `Ctrl+P` plays it back, and
//! `*play 20` plays it twenty times. A recording is keys, not edits: played
//! back, each key does whatever it does where the cursor is now, which is
//! what makes one recording a change you can make to every line in turn.
//!
//! The two keys themselves are never recorded, so a recording cannot play
//! itself, and a recording that is playing cannot start another.

use crossterm::event::KeyEvent;

use crate::config::keys::{Action, Context as KeyContext};

use super::App;
use super::parts::plural;

/// One thing that happened while recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recorded {
    Key(KeyEvent),
    /// A paste from the terminal, which arrives as all of its text at once.
    Paste(String),
}

/// The most times one `*play` will play: enough for any file a person would
/// do this to, and a stop for a typo with three zeros too many.
pub const MAX_PLAYS: usize = 10_000;

impl App {
    /// Keep `key`, while recording. The keys that record and play are left
    /// out, see the module docs.
    pub(super) fn record_key(&mut self, key: &KeyEvent) {
        if self.recording.is_none() {
            return;
        }
        let own = matches!(
            self.keymap.find(KeyContext::Editor, key),
            Some(Action::MacroRecord | Action::MacroPlay)
        );
        if let (Some(keys), false) = (&mut self.recording, own) {
            keys.push(Recorded::Key(*key));
        }
    }

    pub(super) fn record_paste(&mut self, text: &str) {
        if let Some(keys) = &mut self.recording {
            keys.push(Recorded::Paste(text.to_string()));
        }
    }

    /// Whether keys are being recorded, for the status line.
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// `Ctrl+R`: start recording, or stop and keep what was recorded.
    pub(super) fn toggle_recording(&mut self) {
        match self.recording.take() {
            Some(keys) if keys.is_empty() => {
                self.status = "stopped, nothing recorded".into();
            }
            Some(keys) => {
                let n = keys.len();
                self.recorded = keys;
                self.status = format!(
                    "recorded {n} key{}, {} plays them",
                    plural(n),
                    self.key_name(Action::MacroPlay)
                );
            }
            None => {
                self.recording = Some(Vec::new());
                self.status = format!("recording, {} stops", self.key_name(Action::MacroRecord));
            }
        }
    }

    /// `Ctrl+P`, or `*play` with a count: what was recorded, `times` over.
    pub(super) fn play_macro(&mut self, times: usize) {
        if self.recording.is_some() {
            self.status = format!(
                "still recording, {} stops",
                self.key_name(Action::MacroRecord)
            );
            return;
        }
        if self.recorded.is_empty() {
            self.status = format!(
                "nothing recorded, {} starts",
                self.key_name(Action::MacroRecord)
            );
            return;
        }
        if self.playing {
            return;
        }
        self.playing = true;
        let keys = self.recorded.clone();
        let times = times.clamp(1, MAX_PLAYS);
        'all: for _ in 0..times {
            for thing in &keys {
                match thing {
                    Recorded::Key(key) => self.handle_key(*key),
                    Recorded::Paste(text) => self.on_paste(text.clone()),
                }
                if self.should_quit {
                    break 'all;
                }
            }
        }
        self.playing = false;
        if times > 1 {
            self.status = format!("played {times} times");
        }
    }
}
