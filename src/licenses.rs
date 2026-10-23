//! Who owns what is in the binary.
//!
//! Three layers, and `tiny --licenses` prints all three:
//!
//! - tiny itself, MIT, reproduced from the `LICENSE` file beside this source
//!   so the two can never drift apart.
//! - Everything it is compiled with, in `THIRD-PARTY-NOTICES.txt`: each crate
//!   with its copyright lines and the license it is offered under, Lua (which
//!   is built from source into the binary), and the color themes syntect
//!   bundles, then the text of each license once. Their code is *inside*
//!   the binary, which is what makes this a redistribution and their notices
//!   tiny's to carry. Every one is permissive; nothing here is copyleft,
//!   which is what keeps the MIT above honest.
//! - The syntax definitions, which are third-party data files and carry their
//!   own notices, see [`crate::text::highlight::acknowledgments`].
//!
//! The notices file goes into every release download beside `LICENSE` too,
//! for anyone reading them without running tiny.
//!
//! # Keeping it true
//!
//! The notices are generated, from the crates' own license files, by a test
//! that is skipped unless asked for:
//!
//! ```text
//! cargo test write_third_party_notices -- --ignored
//! ```
//!
//! What it documents is `CRATES`, the written-down list of every crate in the
//! binary, in the tests below. Two tests keep both honest: every crate in
//! `Cargo.lock` that ships has to be in the list, and every crate in the list
//! has to be in the notices, so a new dependency fails the suite until it is
//! listed and the notices are written again.
//!
//! To rebuild the list after changing dependencies, take every target a
//! release is built for (see `release.yml`), since Windows compiles in
//! crates the others do not:
//!
//! ```text
//! for t in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu \
//!          aarch64-unknown-linux-gnu x86_64-pc-windows-msvc \
//!          aarch64-pc-windows-msvc; do
//!   cargo tree -e normal,build --prefix none --no-dedupe --target $t
//! done | awk 'NF{print $1}' | sort -u
//! ```
//!
//! and take each crate's `license` field from `cargo metadata`.

/// tiny's own terms, everything it is compiled with, and the grammars it
/// bundles: the whole of what `tiny --licenses` prints.
pub fn notice() -> String {
    let mut out = String::new();
    out.push_str(concat!("tiny ", env!("CARGO_PKG_VERSION"), "\n\n"));
    out.push_str(include_str!("../LICENSE"));
    out.push('\n');
    out.push_str(include_str!("../THIRD-PARTY-NOTICES.txt"));
    out.push('\n');
    out.push_str(&crate::text::highlight::acknowledgments());
    out
}

/// Every crate compiled into tiny, on any platform it is released for, with
/// the license expression it declares: what the notices are written from.
///
/// Spelled exactly as each crate spells it: `MIT/Apache-2.0` and
/// `MIT OR Apache-2.0` are the same permission and different strings, and
/// reproducing a notice means reproducing it.
#[cfg(test)]
const CRATES: &[(&str, &str)] = &[
    ("adler2", "0BSD OR MIT OR Apache-2.0"),
    ("aho-corasick", "Unlicense OR MIT"),
    ("allocator-api2", "MIT OR Apache-2.0"),
    ("anyhow", "MIT OR Apache-2.0"),
    ("arrayvec", "MIT OR Apache-2.0"),
    ("autocfg", "Apache-2.0 OR MIT"),
    ("bincode", "MIT"),
    ("bit-set", "Apache-2.0 OR MIT"),
    ("bit-vec", "Apache-2.0 OR MIT"),
    ("bitflags", "MIT OR Apache-2.0"),
    ("bstr", "MIT OR Apache-2.0"),
    ("castaway", "MIT"),
    ("cc", "MIT OR Apache-2.0"),
    ("cfg-if", "MIT OR Apache-2.0"),
    ("cfg_aliases", "MIT"),
    ("compact_str", "MIT"),
    ("convert_case", "MIT"),
    ("crc32fast", "MIT OR Apache-2.0"),
    ("critical-section", "MIT OR Apache-2.0"),
    ("crossterm", "MIT"),
    ("crossterm_winapi", "MIT"),
    ("darling", "MIT"),
    ("darling_core", "MIT"),
    ("darling_macro", "MIT"),
    ("deranged", "MIT OR Apache-2.0"),
    ("derive_more", "MIT"),
    ("derive_more-impl", "MIT"),
    ("document-features", "MIT OR Apache-2.0"),
    ("downcast-rs", "MIT/Apache-2.0"),
    ("either", "MIT OR Apache-2.0"),
    ("equivalent", "Apache-2.0 OR MIT"),
    ("errno", "MIT OR Apache-2.0"),
    ("fancy-regex", "MIT"),
    ("filedescriptor", "MIT"),
    ("find-msvc-tools", "MIT OR Apache-2.0"),
    ("flate2", "MIT OR Apache-2.0"),
    ("fnv", "Apache-2.0 / MIT"),
    ("foldhash", "Zlib"),
    ("hashbrown", "MIT OR Apache-2.0"),
    ("heck", "MIT OR Apache-2.0"),
    ("ident_case", "MIT/Apache-2.0"),
    ("indexmap", "Apache-2.0 OR MIT"),
    ("indoc", "MIT OR Apache-2.0"),
    ("instability", "MIT"),
    ("itertools", "MIT OR Apache-2.0"),
    ("itoa", "MIT OR Apache-2.0"),
    ("kasuari", "MIT OR Apache-2.0"),
    ("lazy_static", "MIT OR Apache-2.0"),
    ("libc", "MIT OR Apache-2.0"),
    ("line-clipping", "MIT OR Apache-2.0"),
    (
        "linux-raw-sys",
        "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
    ),
    ("litrs", "MIT OR Apache-2.0"),
    ("lock_api", "MIT OR Apache-2.0"),
    ("log", "MIT OR Apache-2.0"),
    ("lru", "MIT"),
    ("lua-src", "MIT"),
    ("luajit-src", "MIT"),
    ("memchr", "Unlicense OR MIT"),
    ("miniz_oxide", "MIT OR Zlib OR Apache-2.0"),
    ("mio", "MIT"),
    ("mlua", "MIT"),
    ("mlua-sys", "MIT"),
    ("nix", "MIT"),
    ("num-conv", "MIT OR Apache-2.0"),
    ("num-traits", "MIT OR Apache-2.0"),
    ("num_threads", "MIT OR Apache-2.0"),
    ("once_cell", "MIT OR Apache-2.0"),
    ("parking_lot", "MIT OR Apache-2.0"),
    ("parking_lot_core", "MIT OR Apache-2.0"),
    ("pkg-config", "MIT OR Apache-2.0"),
    ("portable-pty", "MIT"),
    ("powerfmt", "MIT OR Apache-2.0"),
    ("proc-macro2", "MIT OR Apache-2.0"),
    ("pulldown-cmark", "MIT"),
    ("quote", "MIT OR Apache-2.0"),
    ("ratatui", "MIT"),
    ("ratatui-core", "MIT"),
    ("ratatui-crossterm", "MIT"),
    ("ratatui-macros", "MIT"),
    ("ratatui-widgets", "MIT"),
    ("regex", "MIT OR Apache-2.0"),
    ("regex-automata", "MIT OR Apache-2.0"),
    ("regex-syntax", "MIT OR Apache-2.0"),
    ("rustc-hash", "Apache-2.0 OR MIT"),
    ("rustc_version", "MIT OR Apache-2.0"),
    (
        "rustix",
        "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
    ),
    ("rustversion", "MIT OR Apache-2.0"),
    ("ryu", "Apache-2.0 OR BSL-1.0"),
    ("same-file", "Unlicense/MIT"),
    ("scopeguard", "MIT OR Apache-2.0"),
    ("semver", "MIT OR Apache-2.0"),
    ("serde", "MIT OR Apache-2.0"),
    ("serde_core", "MIT OR Apache-2.0"),
    ("serde_derive", "MIT OR Apache-2.0"),
    ("serde_json", "MIT OR Apache-2.0"),
    ("serde_spanned", "MIT OR Apache-2.0"),
    ("serial2", "BSD-2-Clause OR Apache-2.0"),
    ("shared_library", "Apache-2.0/MIT"),
    ("shell-words", "MIT/Apache-2.0"),
    ("shlex", "MIT OR Apache-2.0"),
    ("signal-hook", "Apache-2.0/MIT"),
    ("signal-hook-mio", "MIT OR Apache-2.0"),
    ("signal-hook-registry", "MIT OR Apache-2.0"),
    ("simd-adler32", "MIT"),
    ("smallvec", "MIT OR Apache-2.0"),
    ("static_assertions", "MIT OR Apache-2.0"),
    ("streaming-iterator", "MIT OR Apache-2.0"),
    ("strsim", "MIT"),
    ("strum", "MIT"),
    ("strum_macros", "MIT"),
    ("syn", "MIT OR Apache-2.0"),
    ("syntect", "MIT"),
    ("thiserror", "MIT OR Apache-2.0"),
    ("thiserror-impl", "MIT OR Apache-2.0"),
    ("time", "MIT OR Apache-2.0"),
    ("time-core", "MIT OR Apache-2.0"),
    ("toml", "MIT OR Apache-2.0"),
    ("toml_datetime", "MIT OR Apache-2.0"),
    ("toml_parser", "MIT OR Apache-2.0"),
    ("toml_writer", "MIT OR Apache-2.0"),
    ("tree-sitter", "MIT"),
    ("tree-sitter-javascript", "MIT"),
    ("tree-sitter-language", "MIT"),
    ("tree-sitter-python", "MIT"),
    ("tree-sitter-rust", "MIT"),
    ("tree-sitter-tags", "MIT"),
    ("two-face", "MIT OR Apache-2.0"),
    ("unicase", "MIT OR Apache-2.0"),
    ("unicode-ident", "(MIT OR Apache-2.0) AND Unicode-3.0"),
    ("unicode-segmentation", "MIT OR Apache-2.0"),
    ("unicode-truncate", "MIT OR Apache-2.0"),
    ("unicode-width", "MIT OR Apache-2.0"),
    ("vt100", "MIT"),
    ("vte", "Apache-2.0 OR MIT"),
    ("walkdir", "Unlicense/MIT"),
    ("which", "MIT"),
    ("winapi", "MIT/Apache-2.0"),
    ("winapi-util", "Unlicense OR MIT"),
    ("windows-link", "MIT OR Apache-2.0"),
    ("windows-sys", "MIT OR Apache-2.0"),
    ("winnow", "MIT"),
    ("winreg", "MIT"),
    ("zmij", "MIT"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Package names in `Cargo.lock`, which is the whole dependency graph
    /// including the ones only the tests use.
    fn locked() -> Vec<String> {
        include_str!("../Cargo.lock")
            .lines()
            .filter_map(|l| l.strip_prefix("name = \""))
            .filter_map(|l| l.strip_suffix('"'))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn every_listed_crate_is_one_we_actually_depend_on() {
        let locked = locked();
        for (name, _) in CRATES {
            assert!(
                locked.iter().any(|l| l == name),
                "{name} is listed as a dependency but is not in Cargo.lock"
            );
        }
    }

    #[test]
    fn every_dependency_we_ship_is_listed() {
        // The direct ones, read from the manifest rather than written down
        // again here: a crate added to `[dependencies]` and not to `CRATES`
        // is the mistake this is looking for.
        let manifest = include_str!("../Cargo.toml");
        let deps = manifest
            .split("[dependencies]")
            .nth(1)
            .expect("a dependencies section")
            .split("\n[")
            .next()
            .expect("the section ends");
        for line in deps.lines() {
            let Some(name) = line.split_whitespace().next() else {
                continue;
            };
            if name.is_empty() || name.starts_with('#') {
                continue;
            }
            assert!(
                CRATES.iter().any(|(c, _)| *c == name),
                "{name} is a dependency but is not in CRATES (see the module docs)"
            );
        }
    }

    #[test]
    fn the_list_is_sorted_and_says_each_name_once() {
        let names: Vec<&str> = CRATES.iter().map(|(n, _)| *n).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "CRATES should be sorted, with no repeats");
    }

    #[test]
    fn nothing_in_the_binary_is_copyleft() {
        // The whole reason tiny can be MIT. A GPL crate would make this a
        // licensing decision rather than a listing, so it should fail loudly
        // the moment one arrives.
        for (name, license) in CRATES {
            let l = license.to_ascii_uppercase();
            for bad in ["GPL", "AGPL", "LGPL", "MPL", "EUPL", "CDDL"] {
                assert!(!l.contains(bad), "{name} is {license}");
            }
        }
    }

    #[test]
    fn the_notice_carries_all_three_layers() {
        let out = notice();
        assert!(out.contains("MIT License"), "tiny's own terms");
        assert!(out.contains("ratatui"), "the crates it is built from");
        assert!(
            out.contains("Lua.org, PUC-Rio"),
            "Lua, built in from source"
        );
        assert!(out.contains("syntax definitions"), "the bundled grammars");
    }

    // ---- the notices file --------------------------------------------------

    const NOTICES: &str = include_str!("../THIRD-PARTY-NOTICES.txt");

    /// Each crate's entry in the notices: its name, and the license line under
    /// it. An entry is a line with nothing in front of it, in the crates part.
    ///
    /// Line endings are made one kind first: git on Windows checks the file
    /// out with `\r\n`, and a `\r` left at the end of the part was read as
    /// one more crate, with no name.
    fn entries() -> Vec<(String, String)> {
        let notices = NOTICES.replace("\r\n", "\n");
        let crates = notices
            .split("---- crates ")
            .nth(1)
            .expect("a crates part")
            .split("\n---- ")
            .next()
            .expect("it ends");
        let lines: Vec<&str> = crates.lines().skip(1).collect();
        lines
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with(' '))
            .map(|(i, l)| {
                let name = l.split_whitespace().next().unwrap_or("").to_string();
                let license = lines
                    .get(i + 1)
                    .and_then(|l| l.trim().strip_prefix("license: "))
                    .unwrap_or("")
                    .to_string();
                (name, license)
            })
            .collect()
    }

    #[test]
    fn the_notices_name_every_crate_and_nothing_else() {
        let entries = entries();
        let named: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        let listed: Vec<&str> = CRATES.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            named, listed,
            "THIRD-PARTY-NOTICES.txt is out of date: run \
             `cargo test write_third_party_notices -- --ignored`"
        );
        for ((name, said), (_, declared)) in entries.iter().zip(CRATES) {
            assert!(
                said.starts_with(declared),
                "{name}: the notices say `{said}`, the crate says `{declared}`"
            );
        }
    }

    #[test]
    fn every_license_taken_has_its_text_in_the_notices() {
        for (name, said) in entries() {
            let taken = said.rsplit("taken as ").next().unwrap_or(&said).to_string();
            for id in taken.split(" and ") {
                assert!(
                    NOTICES
                        .replace("\r\n", "\n")
                        .contains(&format!("\n---- {id} ")),
                    "{name} is taken under {id}, whose text is not in the notices"
                );
            }
        }
    }

    // ---- writing it -------------------------------------------------------

    /// Which of the licenses a crate offers tiny takes.
    ///
    /// MIT wherever it is offered: the shortest text, and the one tiny is
    /// under itself. Otherwise anything but Apache-2.0, whose text is longer
    /// than most of the crates it would cover. An `AND` takes every part.
    fn taken(expression: &str) -> Vec<String> {
        expression
            .split(" AND ")
            .map(|part| {
                let part = part.trim().trim_start_matches('(').trim_end_matches(')');
                let offered: Vec<&str> = part
                    .split(" OR ")
                    .flat_map(|o| o.split('/'))
                    .map(str::trim)
                    .collect();
                let pick = if offered.contains(&"MIT") {
                    "MIT"
                } else {
                    offered
                        .iter()
                        .find(|o| !o.starts_with("Apache-2.0"))
                        .unwrap_or(&offered[0])
                };
                pick.to_string()
            })
            .collect()
    }

    /// The file in a crate's folder that holds the text of `license`.
    fn license_file(dir: &std::path::Path, license: &str) -> Option<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| {
                let u = n.to_uppercase();
                u.starts_with("LICEN") || u.starts_with("COPYING")
            })
            .collect();
        names.sort();
        let marker = match license {
            "MIT" => "MIT",
            "BSL-1.0" => "BOOST",
            "BSD-2-Clause" => "BSD",
            "Zlib" => "ZLIB",
            "Unicode-3.0" => "UNICODE",
            other => other,
        };
        let read = |n: &String| std::fs::read_to_string(dir.join(n)).ok();
        // A file named for the license, or the one plain LICENSE when that is
        // the license it holds.
        names
            .iter()
            .find(|n| n.to_uppercase().contains(marker))
            .and_then(read)
            .or_else(|| {
                names
                    .iter()
                    .filter(|n| {
                        let u = n.to_uppercase();
                        matches!(
                            u.as_str(),
                            "LICENSE" | "LICENCE" | "LICENSE.MD" | "LICENSE.TXT"
                        )
                    })
                    .filter_map(read)
                    .find(|text| license != "MIT" || text.contains("Permission is hereby granted"))
            })
    }

    /// The copyright lines in a license's text, as written there.
    fn copyright_lines(text: &str) -> Vec<String> {
        text.lines()
            .map(|l| l.trim().trim_start_matches(['>', '*', '#', ' ']).trim())
            .filter(|l| {
                let lower = l.to_lowercase();
                lower.starts_with("copyright") || lower.starts_with("(c)") || l.starts_with('©')
            })
            // `[Name](https://...)` reads as `Name`.
            .map(|l| {
                let mut out = String::new();
                let mut rest = l;
                while let Some(open) = rest.find('[') {
                    match (rest[open..].find("]("), rest[open..].find(')')) {
                        (Some(mid), Some(close)) if mid < close => {
                            out.push_str(&rest[..open]);
                            out.push_str(&rest[open + 1..open + mid]);
                            rest = &rest[open + close + 1..];
                        }
                        _ => break,
                    }
                }
                out.push_str(rest);
                out
            })
            .collect()
    }

    /// One line, ruled out to a fixed width, that starts a part of the file.
    fn rule(title: &str) -> String {
        let head = format!("---- {title} ");
        let dashes = 76usize.saturating_sub(head.chars().count());
        format!("{head}{}\n", "-".repeat(dashes))
    }

    /// Rebuild `THIRD-PARTY-NOTICES.txt` from the crates' own license files.
    ///
    /// Skipped unless asked for, because it reads cargo's copy of every
    /// crate's source and writes into this one. Run it after changing
    /// dependencies and commit what it writes;
    /// `the_notices_name_every_crate_and_nothing_else` says when it is due.
    #[test]
    #[ignore]
    fn write_third_party_notices() {
        use std::collections::BTreeMap;
        use std::path::Path;

        let out = std::process::Command::new(env!("CARGO"))
            .args(["metadata", "--format-version", "1", "--locked"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("cargo metadata runs");
        let meta: serde_json::Value =
            serde_json::from_slice(&out.stdout).expect("cargo metadata speaks JSON");
        let packages = meta["packages"].as_array().expect("a list of packages");

        // tiny's own LICENSE, from the permission on: the MIT text, once, for
        // every crate taken under MIT.
        let own = include_str!("../LICENSE");
        let mit = &own[own
            .find("Permission is hereby granted")
            .expect("tiny is MIT")..];

        let mut texts: BTreeMap<String, Vec<(String, Vec<String>)>> = BTreeMap::new();
        let mut body = String::new();
        for (name, declared) in CRATES {
            let found: Vec<&serde_json::Value> =
                packages.iter().filter(|p| p["name"] == *name).collect();
            assert!(!found.is_empty(), "{name} is not in cargo metadata");
            let mut versions: Vec<&str> =
                found.iter().filter_map(|p| p["version"].as_str()).collect();
            versions.sort();
            let licenses = taken(declared);

            let mut copyright: Vec<String> = Vec::new();
            for p in &found {
                let dir = Path::new(p["manifest_path"].as_str().unwrap())
                    .parent()
                    .unwrap()
                    .to_path_buf();
                // The people the crate names as its authors, which is who holds
                // it when its own notice does not say.
                let authors: Vec<String> = p["authors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a.as_str())
                    .map(|a| a.split('<').next().unwrap_or(a).trim().to_string())
                    .filter(|a| !a.is_empty())
                    .collect();
                for license in &licenses {
                    let Some(text) = license_file(&dir, license) else {
                        continue;
                    };
                    // A line with a year and no one in it keeps its words and
                    // gains the crate's authors after them.
                    copyright.extend(copyright_lines(&text).into_iter().map(|line| {
                        let holder = line
                            .to_lowercase()
                            .replace("copyright", "")
                            .replace("(c)", "")
                            .replace(['©', '-', ',', ' '], "");
                        if !holder.chars().all(|c| c.is_ascii_digit()) {
                            line
                        } else if authors.is_empty() {
                            format!("{line}, the {name} developers")
                        } else {
                            format!("{line}, {}", authors.join(", "))
                        }
                    }));
                    if license != "MIT" {
                        let crates = texts.entry(license.clone()).or_default();
                        let same = |t: &String| t.split_whitespace().eq(text.split_whitespace());
                        match crates.iter_mut().find(|(t, _)| same(t)) {
                            Some((_, users)) => users.push(name.to_string()),
                            None => crates.push((text.trim().to_string(), vec![name.to_string()])),
                        }
                    }
                }
                if copyright.is_empty() {
                    copyright.push(if authors.is_empty() {
                        format!("Copyright (c) the {name} developers")
                    } else {
                        format!("Copyright (c) {}", authors.join(", "))
                    });
                }
            }
            copyright.dedup();
            let mut seen = Vec::new();
            copyright.retain(|c| {
                let fresh = !seen.contains(c);
                seen.push(c.clone());
                fresh
            });

            body.push_str(&format!("{name} {}\n", versions.join(", ")));
            let taken_as = licenses.join(" and ");
            if taken_as == *declared {
                body.push_str(&format!("    license: {declared}\n"));
            } else {
                body.push_str(&format!("    license: {declared}, taken as {taken_as}\n"));
            }
            for c in &copyright {
                body.push_str(&format!("    {c}\n"));
            }
            body.push('\n');
        }

        // Lua is C, built into the binary from the source lua-src carries,
        // and its notice is in that source rather than in any crate's file.
        let lua_src = packages
            .iter()
            .find(|p| p["name"] == "lua-src")
            .expect("lua-src builds Lua");
        let lua_dir = Path::new(lua_src["manifest_path"].as_str().unwrap())
            .parent()
            .unwrap()
            .to_path_buf();
        let lua54 = std::fs::read_dir(&lua_dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("lua-5.4"))
            .max()
            .expect("lua-src carries Lua 5.4");
        let lua_h = std::fs::read_to_string(lua_dir.join(&lua54).join("lua.h")).unwrap();
        let lua_copyright = copyright_lines(&lua_h)
            .into_iter()
            .find(|l| l.contains("Lua.org"))
            .expect("lua.h carries its notice");

        let mut file = String::new();
        file.push_str(
            "THIRD-PARTY NOTICES\n\n\
             tiny carries the work below inside its binary. Each crate is listed with\n\
             its copyright and the license it is offered under; where it offers a\n\
             choice, the one tiny takes is named. The text of each license follows\n\
             the list, once: the copyright notices above it are the ones it refers to.\n\n\
             Written from each crate's own license files by\n\
             `cargo test write_third_party_notices -- --ignored`.\n\n",
        );
        file.push_str(&rule("crates"));
        file.push('\n');
        file.push_str(&body);
        file.push_str(&rule("built in from source"));
        file.push('\n');
        file.push_str(&format!(
            "Lua {} (built by lua-src)\n    license: MIT\n    {lua_copyright}\n\n",
            lua54.trim_start_matches("lua-")
        ));
        // The themes syntect compiles in. Their notices are not in the crate;
        // these are from each theme's own LICENSE upstream: base16-textmate,
        // altercation/solarized and sethlopezme/InspiredGitHub.tmtheme.
        file.push_str(
            "Color themes bundled by syntect\n    license: MIT\n    \
             base16 (ocean, eighties, mocha): Copyright (C) 2012 Chris Kempson\n    \
             Solarized: Copyright (c) 2011 Ethan Schoonover\n    \
             InspiredGitHub: Copyright (c) 2015 Seth Lopez\n\n",
        );
        file.push_str(&rule("MIT"));
        file.push('\n');
        file.push_str(mit.trim());
        file.push_str("\n\n");
        for (license, versions) in &texts {
            for (text, users) in versions {
                file.push_str(&rule(&format!("{license} ({})", users.join(", "))));
                file.push('\n');
                file.push_str(text);
                file.push_str("\n\n");
            }
        }
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("THIRD-PARTY-NOTICES.txt");
        std::fs::write(&path, file.trim_end().to_string() + "\n").unwrap();
    }
}
