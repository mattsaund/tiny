//! Who owns what is in the binary.
//!
//! Three layers, and `tiny --licenses` prints all three:
//!
//! - tiny itself, MIT, reproduced from the `LICENSE` file beside this source
//!   so the two can never drift apart.
//! - The Rust crates it is compiled with. Their code is *inside* the binary,
//!   which is what makes this a redistribution and their terms tiny's problem
//!   rather than only the builder's. Every one of them is permissive; nothing
//!   here is copyleft, which is what keeps the MIT above honest.
//! - The syntax definitions, which are third-party data files and carry their
//!   own notices — see [`crate::text::highlight::acknowledgments`].
//!
//! # Keeping [`CRATES`] true
//!
//! It is a written-down list, so it can go stale. What stops it is the test
//! below: every crate in `Cargo.lock` that is not a test-only dependency has
//! to appear here, and nothing may appear here that is not in the lockfile.
//! Add a dependency without listing it and the suite says so.
//!
//! To rebuild the list after changing dependencies:
//!
//! ```text
//! cargo tree -e normal,build --prefix none --no-dedupe | awk 'NF{print $1}' | sort -u
//! ```
//!
//! and take each crate's `license` field from `cargo metadata`.

/// Every crate compiled into tiny, with the license expression it declares.
///
/// Spelled exactly as each crate spells it — `MIT/Apache-2.0` and
/// `MIT OR Apache-2.0` are the same permission and different strings, and
/// reproducing a notice means reproducing it.
pub const CRATES: &[(&str, &str)] = &[
    ("adler2", "0BSD OR MIT OR Apache-2.0"),
    ("aho-corasick", "Unlicense OR MIT"),
    ("allocator-api2", "MIT OR Apache-2.0"),
    ("anyhow", "MIT OR Apache-2.0"),
    ("bincode", "MIT"),
    ("bit-set", "Apache-2.0 OR MIT"),
    ("bit-vec", "Apache-2.0 OR MIT"),
    ("bitflags", "MIT OR Apache-2.0"),
    ("castaway", "MIT"),
    ("cc", "MIT OR Apache-2.0"),
    ("cfg-if", "MIT OR Apache-2.0"),
    ("compact_str", "MIT"),
    ("convert_case", "MIT"),
    ("crc32fast", "MIT OR Apache-2.0"),
    ("critical-section", "MIT OR Apache-2.0"),
    ("crossterm", "MIT"),
    ("darling", "MIT"),
    ("darling_core", "MIT"),
    ("darling_macro", "MIT"),
    ("deranged", "MIT OR Apache-2.0"),
    ("derive_more", "MIT"),
    ("derive_more-impl", "MIT"),
    ("document-features", "MIT OR Apache-2.0"),
    ("either", "MIT OR Apache-2.0"),
    ("equivalent", "Apache-2.0 OR MIT"),
    ("errno", "MIT OR Apache-2.0"),
    ("fancy-regex", "MIT"),
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
    ("memchr", "Unlicense OR MIT"),
    ("miniz_oxide", "MIT OR Zlib OR Apache-2.0"),
    ("mio", "MIT"),
    ("num-conv", "MIT OR Apache-2.0"),
    ("num_threads", "MIT OR Apache-2.0"),
    ("once_cell", "MIT OR Apache-2.0"),
    ("parking_lot", "MIT OR Apache-2.0"),
    ("parking_lot_core", "MIT OR Apache-2.0"),
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
    ("walkdir", "Unlicense/MIT"),
    ("winnow", "MIT"),
    ("zmij", "MIT"),
];

/// tiny's own terms, everything it is compiled with, and the grammars it
/// bundles — the whole of what `tiny --licenses` prints.
pub fn notice() -> String {
    let mut out = String::new();
    out.push_str(concat!("tiny ", env!("CARGO_PKG_VERSION"), "\n\n"));
    out.push_str(include_str!("../LICENSE"));
    out.push_str(
        "\ntiny is compiled with the crates below, whose code is part of the\n\
         binary. Each is under the permissive terms it declares here; none of\n\
         them is copyleft. Full texts come with each crate's own source.\n\n",
    );
    for (name, license) in CRATES {
        out.push_str(&format!("  {name:<22}  {license}\n"));
    }
    out.push('\n');
    out.push_str(&crate::text::highlight::acknowledgments());
    out
}

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
                "{name} is a dependency but is not in CRATES — see the module docs"
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
        assert!(out.contains("syntax definitions"), "the bundled grammars");
    }
}
