//! What each `*command` does.
//!
//! Commands are the long tail — the things that deserve a name rather than a
//! key, and the things that need an argument. [`App::run_command`] is the
//! dispatch, and each `cmd_*` is one command.
//!
//! # Adding one
//!
//! Two places have to agree: an arm in [`App::run_command`], and an entry in
//! `complete_command`'s table over in [`super::bar`], or it works but nothing
//! suggests it.
//!
//! # Paths in arguments
//!
//! Every command that takes a path puts it through
//! [`super::parts::safe_join`], which is what keeps `*new ../../etc/passwd`
//! inside the project. A command that joins a path itself is a bug.

use anyhow::{Result, anyhow};

use crate::text::search::{self};

use super::App;
use super::mode::{Confirm, ConfirmKind, Focus, Mode};
use super::parts::{plural, safe_join, split_args};
use super::preview::Preview;

/// Split `copy <src> to <dst>` on the word `to`.
///
/// Everything before the separator is one path and everything after it is the
/// other, joined back together with the spaces they were typed with — which is
/// what lets `copy my notes to old work` mean two names rather than four
/// arguments. Without a `to`, exactly two arguments are still accepted, since
/// `copy a b` cannot be read any other way.
///
/// Returns the two sides as owned strings; the error is the sentence to show
/// when it does not parse.
fn split_on_to(args: &[String]) -> Result<(String, String)> {
    const HINT: &str = "say it like: copy README.md to notes";
    let sep = args.iter().position(|a| a.eq_ignore_ascii_case("to"));
    let (left, right) = match sep {
        Some(i) => (&args[..i], &args[i + 1..]),
        None if args.len() == 2 => (&args[..1], &args[1..]),
        None => return Err(anyhow!(HINT)),
    };
    if left.is_empty() || right.is_empty() {
        return Err(anyhow!(HINT));
    }
    Ok((left.join(" "), right.join(" ")))
}

impl App {
    // ---- commands ---------------------------------------------------------

    /// Parse and run a `:` command.
    ///
    /// Adding one means adding an arm here and an entry in `complete_command`'s
    /// `COMMANDS` list, or it will work but never tab-complete. Handlers return
    /// `Result<String>`: a non-empty `Ok` becomes the status message, an empty
    /// one means the command already set its own, and an `Err` is shown with
    /// its full context chain.
    pub(super) fn run_command(&mut self, line: &str) {
        let args = split_args(line);
        let Some(cmd) = args.first().map(String::as_str) else {
            self.status = "no command".into();
            return;
        };
        let rest = &args[1..];
        let result = match cmd {
            "set" => self.cmd_set(rest),
            "replace" | "sub" => self.cmd_replace(rest),
            "config" | "settings" => {
                self.open_settings();
                Ok(String::new())
            }
            "map" | "graph" | "web" => {
                self.open_map();
                Ok(String::new())
            }
            "w" | "write" | "save" => {
                self.save_active();
                Ok(String::new())
            }
            "q" | "quit" => {
                self.request_quit();
                Ok(String::new())
            }
            "wq" => {
                self.save_active();
                self.request_quit();
                Ok(String::new())
            }
            "help" => {
                self.mode = Mode::Help(0);
                Ok(String::new())
            }
            "reload" | "refresh" => {
                self.refresh();
                Ok(String::new())
            }
            "new" => self.cmd_new(rest, false),
            "mkdir" => self.cmd_new(rest, true),
            "delete" | "rm" => self.cmd_delete(rest),
            "copy" | "cp" => self.cmd_copy(rest),
            "line" | "go" => self.cmd_line(rest),
            // A bare number is a line number, the way `:42` is everywhere else.
            n if n.parse::<usize>().is_ok() => self.cmd_line(&args),
            other => Err(anyhow!("unknown command `{other}` — try :help")),
        };
        match result {
            Ok(msg) if !msg.is_empty() => self.status = msg,
            Ok(_) => {}
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    /// `:set key value`, or `:set key` to report the current value.
    ///
    /// Remaining arguments are rejoined with spaces, so a style spec like
    /// `:set theme.heading cyan bold` arrives intact.
    fn cmd_set(&mut self, args: &[String]) -> Result<String> {
        let key = args.first().ok_or_else(|| anyhow!(":set <key> <value>"))?;
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

    /// `:replace find replace`. Counts first and asks before writing anything —
    /// this rewrites files on disk and cannot be undone.
    fn cmd_replace(&mut self, args: &[String]) -> Result<String> {
        if args.len() < 2 {
            return Err(anyhow!(
                ":replace <find> <replace> — quote strings containing spaces"
            ));
        }
        let find = args[0].clone();
        let replace = args[1].clone();
        let report = search::count(self.tree.root_path(), &find, &self.search_opts());
        if report.occurrences == 0 {
            return Ok(format!("`{find}` is not in this project"));
        }
        // Rewriting many files at once is worth a second look first.
        self.mode = Mode::Confirm(Confirm {
            message: format!(
                "Replace {} occurrence{} of `{find}` with `{replace}` across {} file{}?  (y/n)",
                report.occurrences,
                plural(report.occurrences),
                report.files,
                plural(report.files),
            ),
            kind: ConfirmKind::Replace { find, replace },
        });
        Ok(String::new())
    }

    fn cmd_new(&mut self, args: &[String], is_dir: bool) -> Result<String> {
        let name = args
            .first()
            .ok_or_else(|| anyhow!("give a name: :new notes/today.md"))?;
        let base = self.creation_base();
        self.create_entry(&base, name, is_dir)
    }

    /// `:copy <src> to <dst>` — `:cp` is the same command.
    ///
    /// The word `to` is the separator, and everything on each side of it is one
    /// path, so a name with spaces in it needs no quoting: `copy my notes to
    /// old work` does what it reads like. Two arguments with no `to` between
    /// them are accepted as well, since `copy a b` is unambiguous.
    ///
    /// Both paths are relative to the project root and go through
    /// [`safe_join`], so neither end can point outside the project.
    fn cmd_copy(&mut self, args: &[String]) -> Result<String> {
        let (from, into) = split_on_to(args)?;
        let root = self.tree.root_path().to_path_buf();
        let src = safe_join(&root, &from, &root)?;
        let dst = safe_join(&root, &into, &root)?;
        self.copy_entry(&src, &dst)
    }

    /// `*line 42`, `*go to 42`, or just `*42` — put the cursor on that line of
    /// the open file.
    ///
    /// Counted from 1, the way every error message and every other editor
    /// counts, and clamped to the end of the file rather than refused: asking
    /// for line 900 of a 400-line file plainly means the end.
    ///
    /// This always lands in the editor, never in the rendered view. "Line 42"
    /// is a fact about the source; the rendered version of a note has its own
    /// line count that has nothing to do with it.
    fn cmd_line(&mut self, args: &[String]) -> Result<String> {
        // `go to 42` reads better than `go 42`, so the joining word is allowed
        // here as well and simply skipped.
        let number = args
            .iter()
            .find(|a| !a.eq_ignore_ascii_case("to") && !a.eq_ignore_ascii_case("line"))
            .ok_or_else(|| anyhow!("say it like: line 42"))?;
        let wanted: usize = number
            .parse()
            .map_err(|_| anyhow!("`{number}` is not a line number"))?;
        if wanted == 0 {
            return Err(anyhow!("lines are counted from 1"));
        }
        if !matches!(self.preview, Preview::Buffer { .. }) {
            return Err(anyhow!("no file open to jump inside"));
        }

        self.focus = Focus::Editor;
        let Some(ed) = self.active_buffer_mut() else {
            return Err(anyhow!("no file open to jump inside"));
        };
        let last = ed.line_count().saturating_sub(1);
        let line = (wanted - 1).min(last);
        ed.goto(line, 0);
        Ok(format!("line {}", line + 1))
    }

    /// `:delete`, or `:delete <path>`; `:rm` is the same command.
    ///
    /// With no argument it takes whatever the tree cursor is on, which is what
    /// `d` does. Opening a file moves that cursor onto it, so a bare `:delete`
    /// from inside the editor removes the file being edited — which is most of
    /// why this exists as a command as well as a key, since `d` in the editor
    /// is just the letter d.
    ///
    /// An argument is a path relative to the *project root*, not to the cursor,
    /// so `:delete notes/old.md` means the same thing wherever it is typed.
    /// [`safe_join`] keeps it inside the project.
    ///
    /// Like `d`, this only asks the question. Nothing is removed until `y`.
    fn cmd_delete(&mut self, args: &[String]) -> Result<String> {
        let path = match args.first() {
            None => self
                .selected_row()
                .map(|r| r.path.clone())
                .ok_or_else(|| anyhow!("nothing selected"))?,
            Some(name) => {
                let root = self.tree.root_path().to_path_buf();
                safe_join(&root, name, &root)?
            }
        };
        self.arm_delete(&path)?;
        Ok(String::new())
    }
}
