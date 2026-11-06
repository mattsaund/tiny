//! The icon tiny.exe carries, so Explorer, the Start menu and "Open with"
//! show tiny's own rather than a blank program's.
//!
//! A Windows program keeps its icon as a resource, and Microsoft's linker
//! takes resources as a `.res` file. That is usually a resource compiler's
//! job, which is one more tool to have installed; the format is simple
//! enough to write here instead, from the `.ico` that
//! `packaging/desktop/icon.py` draws. Only for Microsoft's linker, which the
//! releases are built with: anything else builds as it always did.

use std::env;
use std::fs;
use std::path::PathBuf;

const ICO: &str = "packaging/desktop/tiny.ico";

/// What a `.res` file calls an icon's picture, and the directory of them.
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={ICO}");
    let windows = env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    let msvc = env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|e| e == "msvc");
    if !(windows && msvc) {
        return;
    }
    let ico = fs::read(ICO).unwrap_or_else(|e| panic!("cannot read {ICO}: {e}"));
    let res = resources(&ico).unwrap_or_else(|| panic!("{ICO} is not an icon file"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("tiny.res");
    fs::write(&out, res).unwrap_or_else(|e| panic!("cannot write {}: {e}", out.display()));
    // The linker takes a `.res` among its inputs as it would an object.
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

/// One resource as a `.res` file holds it: a header giving its type and its
/// number, then its bytes, padded out to a multiple of four.
fn resource(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    let size = u32::try_from(data.len()).expect("an icon is smaller than 4 GB");
    out.extend_from_slice(&size.to_le_bytes());
    // The header's own length: these fields, with the type and the name
    // given as numbers rather than spelled out.
    out.extend_from_slice(&32u32.to_le_bytes());
    for number in [kind, id] {
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
        out.extend_from_slice(&number.to_le_bytes());
    }
    // The data's version, the memory flags, the language (neutral), the
    // version and the characteristics, all as a resource compiler writes
    // them.
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// The `.res` for an `.ico`: an empty resource first, which is how a `.res`
/// says it is the 32-bit kind, then each picture as an icon of its own, then
/// the group of them that Explorer takes for the program's icon.
fn resources(ico: &[u8]) -> Option<Vec<u8>> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(ico.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(ico.get(at..at + 4)?.try_into().ok()?));
    if u16_at(0)? != 0 || u16_at(2)? != 1 {
        return None;
    }
    let count = u16_at(4)?;
    let mut out = Vec::new();
    resource(&mut out, 0, 0, 0, &[]);
    let mut group = Vec::new();
    for word in [0, 1, count] {
        group.extend_from_slice(&u16::to_le_bytes(word));
    }
    for i in 0..count {
        let entry = 6 + 16 * usize::from(i);
        let size = usize::try_from(u32_at(entry + 8)?).ok()?;
        let at = usize::try_from(u32_at(entry + 12)?).ok()?;
        let picture = ico.get(at..at.checked_add(size)?)?;
        let id = i + 1;
        resource(&mut out, RT_ICON, id, 0x1010, picture);
        // The group's entry is the icon file's, with where the picture was
        // in the file swapped for the number of the resource it is now.
        group.extend_from_slice(ico.get(entry..entry + 12)?);
        group.extend_from_slice(&id.to_le_bytes());
    }
    resource(&mut out, RT_GROUP_ICON, 1, 0x1030, &group);
    Some(out)
}
