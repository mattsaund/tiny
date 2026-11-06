//! What the installers put on the desktop, and taking it away again.
//!
//! `install.sh` and `install.ps1` put tiny where the system keeps its
//! programs, with its icon, among the programs offered for text files:
//!
//! - Linux: a menu entry and the icon, in `share/applications` and
//!   `share/icons` beside the folder the binary is in, which is
//!   `~/.local/share` for `~/.local/bin`.
//! - macOS: `tiny.app` in `~/Applications`, which runs tiny in a window of a
//!   terminal.
//! - Windows: a Start menu shortcut, and the registry entries behind
//!   Explorer's "Open with".
//!
//! This finds them for `tiny --uninstall`, which lists them with everything
//! else it removes. Only what was made for the tiny being removed is taken: a
//! menu entry or an app that starts some other tiny is that one's. A tiny a
//! package manager installed has its own entry, which the package manager
//! removes along with it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::terminal::Os;

/// The icon's sizes on Linux, besides the drawing itself: what
/// `packaging/desktop/icons/hicolor` holds, and so what install.sh copies.
const ICON_SIZES: [&str; 9] = [
    "16x16", "22x22", "24x24", "32x32", "48x48", "64x64", "128x128", "256x256", "512x512",
];

/// What tiny.app's Info.plist calls it, and the key in which install.sh
/// writes the binary it starts.
const BUNDLE_ID: &str = "dev.msaunders.tiny";
const BINARY_KEY: &str = "TinyBinary";

/// Where Windows keeps tiny's part in "Open with", under HKEY_CURRENT_USER:
/// the program, the kind of file it opens, and the program among those
/// Settings offers. install.ps1 writes them.
const WINDOWS_APP: &str = r"Software\Classes\Applications\tiny.exe";

/// Where to look, passed in so the tests can point it at a folder of their
/// own.
#[derive(Debug, Clone, Default)]
pub struct Places {
    /// The home folder, which holds `Applications` on a Mac.
    pub home: Option<PathBuf>,
    /// The Start menu's folder of programs, on Windows.
    pub programs: Option<PathBuf>,
}

impl Places {
    /// The ones this machine has.
    pub fn here() -> Self {
        Self {
            home: std::env::var_os("HOME").map(PathBuf::from),
            programs: std::env::var_os("APPDATA")
                .map(|d| PathBuf::from(d).join(r"Microsoft\Windows\Start Menu\Programs")),
        }
    }
}

/// tiny's place on the desktop, as found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Menu {
    /// Files: the menu entry and its icons, or the Start menu's shortcut.
    pub files: Vec<PathBuf>,
    /// tiny.app, which is a folder.
    pub app: Option<PathBuf>,
    /// Whether Windows has tiny in its registry, for "Open with".
    pub registered: bool,
}

impl Menu {
    /// What the installer put on the desktop for the tiny at `bin`.
    pub fn plan(os: Os, bin: &Path, places: &Places) -> Self {
        match os {
            Os::Unix => Self {
                files: menu_entry(bin),
                ..Self::default()
            },
            Os::Mac => Self {
                app: mac_app(bin, places.home.as_deref()),
                ..Self::default()
            },
            Os::Windows => Self {
                files: places
                    .programs
                    .as_ref()
                    .map(|p| p.join("tiny.lnk"))
                    .filter(|p| p.is_file())
                    .into_iter()
                    .collect(),
                registered: registered(),
                ..Self::default()
            },
        }
    }

    /// The lines `tiny --uninstall` lists, in the order they go.
    pub fn lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .files
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect();
        if let Some(app) = &self.app {
            out.push(format!("  {}/", app.display()));
        }
        if self.registered {
            out.push(format!(
                "  tiny in Explorer's \"Open with\" (HKEY_CURRENT_USER\\{WINDOWS_APP} and the rest)"
            ));
        }
        out
    }

    /// Take it all away, saying what would not go.
    pub fn perform(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for file in &self.files {
            if let Err(e) = std::fs::remove_file(file) {
                errors.push(format!("{}: {e}", file.display()));
            }
        }
        if !self.files.is_empty() {
            // GNOME keeps a list of which programs open which files, and
            // would go on offering one that is gone.
            if let Some(apps) = self.files[0].parent() {
                quietly("update-desktop-database", &[apps.as_os_str()]);
            }
        }
        if let Some(app) = &self.app {
            // Told first, so the Mac stops offering it the moment it goes.
            let lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
            quietly(lsregister, &["-u".as_ref(), app.as_os_str()]);
            if let Err(e) = std::fs::remove_dir_all(app) {
                errors.push(format!("{}: {e}", app.display()));
            }
        }
        if self.registered && !unregister() {
            errors.push(format!(
                "HKEY_CURRENT_USER\\{WINDOWS_APP}: PowerShell could not remove it"
            ));
        }
        errors
    }
}

/// The menu entry install.sh made for `bin`, and the icon it put beside it:
/// `share/applications/tiny.desktop` next to the binary's folder, when its
/// Exec line starts this binary.
fn menu_entry(bin: &Path) -> Vec<PathBuf> {
    let Some(share) = bin.parent().and_then(Path::parent).map(|p| p.join("share")) else {
        return Vec::new();
    };
    let entry = share.join("applications").join("tiny.desktop");
    let path = bin.to_string_lossy();
    let ours = std::fs::read_to_string(&entry).is_ok_and(|text| {
        text.lines()
            .any(|line| line.starts_with("Exec=") && line.contains(path.as_ref()))
    });
    if !ours {
        return Vec::new();
    }
    let icons = share.join("icons").join("hicolor");
    let mut files = vec![entry];
    files.extend(
        ICON_SIZES
            .iter()
            .map(|size| icons.join(size).join("apps").join("tiny.png")),
    );
    files.push(icons.join("scalable").join("apps").join("tiny.svg"));
    files.retain(|f| f.is_file());
    files
}

/// The tiny.app install.sh made for `bin`, in `~/Applications`, or in
/// `/Applications` for an install as root: the one whose Info.plist names
/// this binary.
fn mac_app(bin: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let names = format!("<string>{}</string>", xml_text(&bin.to_string_lossy()));
    home.map(|h| h.join("Applications"))
        .into_iter()
        .chain([PathBuf::from("/Applications")])
        .map(|apps| apps.join("tiny.app"))
        .find(|app| {
            std::fs::read_to_string(app.join("Contents").join("Info.plist")).is_ok_and(|plist| {
                plist.contains(&format!("<string>{BUNDLE_ID}</string>"))
                    && plist
                        .split(&format!("<key>{BINARY_KEY}</key>"))
                        .nth(1)
                        .is_some_and(|after| after.trim_start().starts_with(&names))
            })
        })
}

/// `text` as XML writes it between tags.
fn xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Whether Windows has tiny in "Open with": the key install.ps1 writes
/// first. Only ever asked on Windows.
fn registered() -> bool {
    Command::new("reg")
        .args(["query", &format!(r"HKCU\{WINDOWS_APP}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// What takes tiny out of the registry: everything install.ps1 put there.
/// The extensions it was offered for are found rather than listed, so an
/// extension a later installer added, or one taken off its list since, goes
/// too. Keys other programs share are left, with only tiny's value gone.
const UNREGISTER: &str = r"
$user = [Microsoft.Win32.Registry]::CurrentUser
$classes = $user.OpenSubKey('Software\Classes', $true)
if ($classes) {
    foreach ($name in $classes.GetSubKeyNames()) {
        if ($name.StartsWith('.')) {
            $with = $classes.OpenSubKey($name + '\OpenWithProgids', $true)
            if ($with) { $with.DeleteValue('tiny.text', $false); $with.Close() }
        }
    }
    $classes.DeleteSubKeyTree('Applications\tiny.exe', $false)
    $classes.DeleteSubKeyTree('tiny.text', $false)
    $classes.Close()
}
$software = $user.OpenSubKey('Software', $true)
$software.DeleteSubKeyTree('tiny\Capabilities', $false)
$ours = $software.OpenSubKey('tiny')
if ($ours) {
    $empty = $ours.SubKeyCount -eq 0 -and $ours.ValueCount -eq 0
    $ours.Close()
    if ($empty) { $software.DeleteSubKey('tiny', $false) }
}
$registered = $software.OpenSubKey('RegisteredApplications', $true)
if ($registered) { $registered.DeleteValue('tiny', $false); $registered.Close() }
$software.Close()
";

/// Run [`UNREGISTER`]. Handed to PowerShell encoded, as its documentation
/// says to for a script with quotes in it, so nothing on the way can read
/// them as anything else.
fn unregister() -> bool {
    let utf16: Vec<u8> = UNREGISTER
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &base64(&utf16),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Run a program that tidies up after something removed, if it is here.
/// Nothing depends on it, so it is not asked how it went.
fn quietly(program: &str, args: &[&std::ffi::OsStr]) {
    let _ = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Standard base64, for [`unregister`].
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn base64_is_the_standard_one() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    /// `~/.local/bin/tiny` and the menu entry install.sh would make for it.
    fn linux_install(home: &Path, exec: &str) -> PathBuf {
        let bin = home.join(".local").join("bin").join("tiny");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, "").unwrap();
        let share = home.join(".local").join("share");
        fs::create_dir_all(share.join("applications")).unwrap();
        fs::write(
            share.join("applications").join("tiny.desktop"),
            format!("[Desktop Entry]\nName=tiny\nExec={exec} %f\nIcon=tiny\n"),
        )
        .unwrap();
        for size in ["48x48", "scalable"] {
            let dir = share.join("icons/hicolor").join(size).join("apps");
            fs::create_dir_all(&dir).unwrap();
            let name = if size == "scalable" {
                "tiny.svg"
            } else {
                "tiny.png"
            };
            fs::write(dir.join(name), "").unwrap();
        }
        bin
    }

    #[test]
    fn the_menu_entry_for_this_tiny_and_its_icons_go() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path().join("me");
        let bin = linux_install(&home, "");
        let entry = home.join(".local/share/applications/tiny.desktop");
        fs::write(
            &entry,
            format!("[Desktop Entry]\nExec=\"{}\" %f\n", bin.display()),
        )
        .unwrap();
        let menu = Menu::plan(Os::Unix, &bin, &Places::default());
        assert_eq!(menu.files.len(), 3, "{:?}", menu.files);
        assert_eq!(menu.files[0], entry);
        assert!(menu.perform().is_empty());
        assert!(!entry.exists());
        assert!(
            !home
                .join(".local/share/icons/hicolor/48x48/apps/tiny.png")
                .exists()
        );
        assert!(bin.exists(), "the binary is the uninstaller's to remove");
    }

    #[test]
    fn a_menu_entry_that_starts_another_tiny_is_left_alone() {
        let td = tempfile::tempdir().unwrap();
        let bin = linux_install(td.path(), "\"/opt/elsewhere/bin/tiny\"");
        assert_eq!(
            Menu::plan(Os::Unix, &bin, &Places::default()),
            Menu::default()
        );
    }

    /// tiny.app as install.sh leaves it, starting `starts`.
    fn mac_install(home: &Path, starts: &str) {
        let contents = home.join("Applications/tiny.app/Contents");
        fs::create_dir_all(&contents).unwrap();
        fs::write(
            contents.join("Info.plist"),
            format!(
                "<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict>\n\
                 \t<key>CFBundleIdentifier</key>\n\t<string>{BUNDLE_ID}</string>\n\
                 \t<key>{BINARY_KEY}</key>\n\t<string>{}</string>\n</dict>\n</plist>\n",
                xml_text(starts)
            ),
        )
        .unwrap();
    }

    #[test]
    fn the_app_that_starts_this_tiny_goes() {
        let td = tempfile::tempdir().unwrap();
        let bin = td.path().join("R&D <tools>/bin/tiny");
        mac_install(td.path(), &bin.to_string_lossy());
        let places = Places {
            home: Some(td.path().to_path_buf()),
            ..Places::default()
        };
        let menu = Menu::plan(Os::Mac, &bin, &places);
        let app = td.path().join("Applications").join("tiny.app");
        assert_eq!(menu.app.as_deref(), Some(app.as_path()));
        assert_eq!(menu.lines(), [format!("  {}/", app.display())]);
        assert!(menu.perform().is_empty());
        assert!(!app.exists());
    }

    #[test]
    fn an_app_that_starts_another_tiny_is_left_alone() {
        let td = tempfile::tempdir().unwrap();
        mac_install(td.path(), "/usr/local/bin/tiny");
        let places = Places {
            home: Some(td.path().to_path_buf()),
            ..Places::default()
        };
        let menu = Menu::plan(Os::Mac, &td.path().join("bin/tiny"), &places);
        assert_eq!(menu.app, None);
    }

    #[test]
    fn the_start_menu_shortcut_goes() {
        let td = tempfile::tempdir().unwrap();
        let link = td.path().join("tiny.lnk");
        fs::write(&link, "").unwrap();
        let places = Places {
            programs: Some(td.path().to_path_buf()),
            ..Places::default()
        };
        let menu = Menu::plan(Os::Windows, Path::new(r"C:\x\tiny.exe"), &places);
        assert_eq!(menu.files, [link]);
    }

    /// The installers and this module name the same things, which nothing
    /// else would notice drifting apart until an uninstall left something.
    #[test]
    fn the_installers_write_what_this_takes_away() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let sh = fs::read_to_string(root.join("install.sh")).unwrap();
        assert!(sh.contains("apps=\"$share/applications\""));
        assert!(sh.contains("\"$apps/tiny.desktop\""));
        assert!(sh.contains(&format!("CFBundleIdentifier -string {BUNDLE_ID}")));
        assert!(sh.contains(&format!("{BINARY_KEY} -string \"$PREFIX/tiny\"")));
        assert!(sh.contains("apps=\"$HOME/Applications\""));
        let ps = fs::read_to_string(root.join("install.ps1")).unwrap();
        assert!(ps.contains(&format!("$app = '{WINDOWS_APP}'")));
        for name in [
            "tiny.text",
            r"Software\tiny\Capabilities",
            "RegisteredApplications",
            "tiny.lnk",
        ] {
            assert!(ps.contains(name), "install.ps1 has no {name}");
            assert!(
                UNREGISTER.contains(name.trim_start_matches(r"Software\")) || name == "tiny.lnk",
                "the uninstaller has no {name}"
            );
        }
        // Every size of the icon there is, and no other.
        let hicolor = root.join("packaging/desktop/icons/hicolor");
        let mut sizes: Vec<String> = fs::read_dir(&hicolor)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|s| s != "scalable")
            .collect();
        sizes.sort_by_key(|s| s.split('x').next().unwrap().parse::<u32>().unwrap());
        assert_eq!(sizes, ICON_SIZES);
    }
}
