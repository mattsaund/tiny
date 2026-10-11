//! Running a `*command`.
//!
//! Commands are the long tail — the things that deserve a name rather than a
//! key, and the things that need an argument. They are written as sentences:
//! `*copy temp.txt to notes`, `*goto function main`, `*replace hi with hello
//! in greetings.txt`. [`App::run_command`] is the whole of running one, in
//! three steps that each live elsewhere:
//!
//! 1. [`super::sentence`] reads the line into a verb and its parts;
//! 2. [`super::plan`] finds what it names and works out what it would do;
//! 3. the plan is carried out.
//!
//! The bar does the first two on every pause in typing and shows the answer
//! under the line, so what Enter will do is on screen before it is pressed.
//!
//! # Adding one
//!
//! A verb in `sentence` (and its spellings), a [`super::plan::Plan`] it
//! becomes, a line saying what it does in `App::describe`, and the doing of it
//! in `App::carry_out`. Then its first word in `sentence::VERBS`, so Tab
//! offers it, and a row in the help.
//!
//! # Paths in arguments
//!
//! Every path a command acts on goes through [`super::parts::safe_join`],
//! which is what keeps `*new ../../etc/passwd` inside the project. A command
//! that joins a path itself is a bug.

use anyhow::{Result, anyhow};

use super::App;
use super::parts::plural;
use super::plan;

impl App {
    // ---- commands ---------------------------------------------------------

    /// Read, plan and carry out a `*` command.
    ///
    /// # What is a command and what is a key
    ///
    /// The split is by what the thing *is*, not by how often it is done.
    /// Moving around and changing a file are keys: they happen constantly, in
    /// the middle of something else, and a key is the only shape fast enough.
    /// Everything a command does is the other kind — deleting a path, re-reading
    /// the disk, opening the settings, replacing a word across the project —
    /// done deliberately, often with an argument to type, and none the worse
    /// for costing a sentence instead of a key.
    ///
    /// That is why there is no `*w`, `*q` or `*wq`: saving and quitting are
    /// `Ctrl+S` and `Ctrl+Q`, and a second way in earned nothing.
    ///
    /// The project is listed afresh first, so Enter acts on the disk as it is
    /// rather than as it was when the bar opened.
    pub(super) fn run_command(&mut self, line: &str) {
        self.forget_names();
        self.planning_for_preview = false;
        let said = plan::read(line);
        let result = match self.plan(&said, line) {
            Ok(plan) => self.carry_out(plan, line),
            Err(why) => Err(anyhow!(why)),
        };
        // Whatever it did, the listing may now be out of date.
        self.forget_names();
        match result {
            Ok(msg) if !msg.is_empty() => self.status = msg,
            Ok(_) => {}
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    /// What the command typed so far would do, for the line under the bar.
    pub(super) fn preview_command(&mut self, line: &str) -> Result<String, String> {
        self.planning_for_preview = true;
        let said = plan::read(line);
        let planned = self.plan(&said, line);
        self.planning_for_preview = false;
        planned.map(|p| self.describe(&p))
    }

    /// `*set key value`, or `*set key` to report the current value.
    ///
    /// Remaining arguments are rejoined with spaces, so a style spec like
    /// `*set theme.heading cyan bold` arrives intact.
    pub(super) fn cmd_set(&mut self, args: &[String]) -> Result<String> {
        let key = args
            .first()
            .ok_or_else(|| anyhow!("set [setting] to [value]"))?;
        // `set tab_width to 2` reads better than `set tab_width 2`, so the
        // joining word is allowed and skipped.
        let args: Vec<String> = match args.get(1) {
            Some(w) if w.eq_ignore_ascii_case("to") => {
                let mut rest = vec![key.clone()];
                rest.extend_from_slice(&args[2..]);
                rest
            }
            _ => args.to_vec(),
        };
        let args = &args[..];
        if args.len() < 2 {
            // With no value, report the current one rather than erroring.
            let value = self
                .config
                .get(key)
                .ok_or_else(|| anyhow!("unknown setting `{key}`"))?;
            return Ok(format!("{key} = {value}"));
        }
        let value = args[1..].join(" ");
        self.config.set(key, &value)?;
        self.apply_config();
        Ok(format!("{key} = {value}"))
    }

    /// `*reload`: read the disk, and the plugins folder, again.
    pub(super) fn cmd_reload(&mut self) -> Result<String> {
        self.refresh();
        // The plugins folder is on disk too, and this is how a plugin just
        // dropped into it starts working.
        self.load_plugins();
        let n = self.plugins.loaded.len();
        Ok(match (n, self.plugins.errors.is_empty()) {
            (0, true) => String::new(),
            (n, true) => format!("refreshed — {n} plugin{}", plural(n)),
            // `load_plugins` has already named the one that failed.
            _ => String::new(),
        })
    }
}
