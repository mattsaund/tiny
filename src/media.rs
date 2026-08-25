//! Pictures and video in the preview pane.
//!
//! Images are drawn as colored half-blocks: each terminal cell holds a `▀`
//! whose foreground is the upper pixel and background the lower one, so one
//! cell carries two pixels and the result comes out roughly square. This
//! works in any terminal that does 24-bit color — no graphics protocol, no
//! sixel, nothing to detect.
//!
//! Video shows a poster frame, pulled with ffmpeg when it is installed. When
//! it is not, the pane says so and falls back to file details rather than
//! pretending the feature is missing.
//!
//! # The half-block trick
//!
//! Terminal cells are about twice as tall as they are wide, so one pixel per
//! cell gives a picture stretched to twice its height. The fix is the upper
//! half-block `▀`: set its *foreground* to the top pixel and its *background*
//! to the bottom one, and a single cell carries a 1x2 column of pixels. The
//! result is roughly square and doubles the vertical resolution for free.
//!
//! The only requirement is 24-bit color, which every modern terminal has.
//! Nothing here detects or negotiates a graphics protocol — no sixel, no
//! kitty, no iTerm2 — which is why it works over ssh and inside tmux.
//!
//! # Nothing is cached here
//!
//! [`render`] decodes and scales from scratch every time. `App` holds the
//! result in a `MediaCache` keyed by path *and* pane size, so a redraw at the
//! same size is free and a resize re-decodes. Keep it that way: caching inside
//! this module would need its own invalidation, and the app already knows when
//! the answer can change.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, Rgba};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// What kind of preview a file wants, decided from its extension alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
    /// Anything else — text, binaries, files with no extension.
    Other,
}

/// Classify by extension, case-insensitively.
///
/// Extension-only on purpose: this runs on every cursor move in the tree, and
/// sniffing file contents would mean a read per keystroke. The actual decode
/// in [`load`] does check the real format, so a mislabelled file fails with a
/// clear message rather than drawing garbage.
pub fn classify(path: &Path) -> Kind {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico") => {
            Kind::Image
        }
        Some("mp4" | "mkv" | "mov" | "webm" | "avi" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg") => {
            Kind::Video
        }
        _ => Kind::Other,
    }
}

/// A drawn preview: the block of cells, plus a line describing what it is.
#[derive(Debug, Clone)]
pub struct Preview {
    /// Ready-to-render rows of half-block cells.
    pub lines: Vec<Line<'static>>,
    /// Caption, e.g. `1920x1080 PNG`. Drawn under the picture.
    pub note: String,
}

/// Draw `path` into at most `cols` x `rows` cells.
///
/// The size is a ceiling, not a target: aspect ratio is preserved, so the
/// result fills one axis and is centred on the other. Errors are ordinary
/// values here — a file that will not decode produces a message the pane
/// shows in place of the picture.
pub fn render(path: &Path, kind: Kind, cols: usize, rows: usize) -> Result<Preview> {
    if cols < 2 || rows < 2 {
        return Err(anyhow!("pane too small"));
    }
    match kind {
        Kind::Image => {
            let img = load(path)?;
            let (w, h) = img.dimensions();
            Ok(Preview {
                lines: to_half_blocks(&img, cols, rows),
                note: format!("{w}x{h} {}", format_name(path)),
            })
        }
        Kind::Video => {
            let frame = poster_frame(path)?;
            let img = load(&frame.path)?;
            let (w, h) = img.dimensions();
            Ok(Preview {
                lines: to_half_blocks(&img, cols, rows),
                note: format!("{w}x{h} poster frame · {}", format_name(path)),
            })
        }
        Kind::Other => Err(anyhow!("not a picture or a video")),
    }
}

/// The extension, upper-cased, for the caption. `"file"` when there is none.
fn format_name(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_uppercase())
        .unwrap_or_else(|| "file".into())
}

/// Decode an image file, identifying the format from its contents.
///
/// Every error names the file, because this message is what the user sees in
/// the pane instead of a picture.
fn load(path: &Path) -> Result<DynamicImage> {
    // Sniff the contents rather than trusting the extension.
    let reader = image::ImageReader::open(path)
        .with_context(|| format!("cannot read {}", path.display()))?
        .with_guessed_format()
        .with_context(|| format!("cannot identify {}", path.display()))?;
    reader
        .decode()
        .with_context(|| format!("cannot decode {}", path.display()))
}

/// A temp file that deletes itself when the preview is done with it.
///
/// The `Drop` impl is the whole point: [`render`] decodes the frame and then
/// lets this fall out of scope, so a long session browsing videos does not
/// leave a trail of PNGs in the temp directory.
struct TempFrame {
    path: PathBuf,
}

impl Drop for TempFrame {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Pull a single frame out of a video with ffmpeg.
///
/// Two attempts, because seeking is the fast path and it fails on very short
/// clips: first at the one-second mark (`-ss` before `-i`, so ffmpeg seeks
/// rather than decoding from the start), then from frame 0 if that produced
/// nothing.
///
/// ffmpeg is an optional runtime dependency, never a build one. When it is
/// missing the error says so by name, so the pane can tell the user what to
/// install instead of just failing.
fn poster_frame(path: &Path) -> Result<TempFrame> {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return Err(anyhow!(
            "video previews need ffmpeg on PATH — install it to see a frame here"
        ));
    }
    let out = std::env::temp_dir().join(format!(
        "tiny-frame-{}-{}.png",
        std::process::id(),
        // Distinguish frames within one run without pulling in a rng.
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y"])
        // Seek before -i so it does not decode from the start of the file.
        // Falls back to frame 0 for clips shorter than a second.
        .args(["-ss", "1"])
        .arg("-i")
        .arg(path)
        .args(["-frames:v", "1"])
        .arg(&out)
        .status()
        .context("could not run ffmpeg")?;
    if !status.success() || !out.exists() {
        // Very short clip: try again from the very beginning.
        let status = Command::new("ffmpeg")
            .args(["-v", "error", "-y"])
            .arg("-i")
            .arg(path)
            .args(["-frames:v", "1"])
            .arg(&out)
            .status()
            .context("could not run ffmpeg")?;
        if !status.success() || !out.exists() {
            return Err(anyhow!("ffmpeg could not read a frame from this file"));
        }
    }
    Ok(TempFrame { path: out })
}

/// Scale to fit and pack two pixel rows into each line of cells.
///
/// The pixel budget is `cols` wide by `rows * 2` tall, since each cell holds
/// two stacked pixels. One scale factor is chosen for both axes so the aspect
/// ratio survives, then the image is centred horizontally.
///
/// The row loop steps by two: `row` becomes the foreground and `row + 1` the
/// background of the same `▀`. An odd final row has no partner, so its lower
/// half is left transparent.
fn to_half_blocks(img: &DynamicImage, cols: usize, rows: usize) -> Vec<Line<'static>> {
    let (iw, ih) = img.dimensions();
    if iw == 0 || ih == 0 {
        return Vec::new();
    }
    // Two pixels stack in one cell, so the pixel grid is twice as tall.
    let max_w = cols as f32;
    let max_h = (rows * 2) as f32;
    let scale = (max_w / iw as f32).min(max_h / ih as f32);
    let tw = ((iw as f32 * scale).round() as u32).max(1);
    let th = ((ih as f32 * scale).round() as u32).max(1);

    let scaled = img.resize_exact(tw, th, FilterType::CatmullRom).to_rgba8();
    // Centre it, so a portrait image does not hug the left edge.
    let pad = (cols.saturating_sub(tw as usize)) / 2;

    let mut lines = Vec::with_capacity(th.div_ceil(2) as usize);
    for row in (0..th).step_by(2) {
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(tw as usize + 1);
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        for x in 0..tw {
            let top = px(scaled.get_pixel(x, row));
            // An odd final row has no lower pixel; leave that half blank.
            let bottom = if row + 1 < th {
                px(scaled.get_pixel(x, row + 1))
            } else {
                None
            };
            let mut style = Style::default();
            style = match top {
                Some(c) => style.fg(c),
                None => style.fg(Color::Reset),
            };
            style = match bottom {
                Some(c) => style.bg(c),
                None => style.bg(Color::Reset),
            };
            spans.push(Span::styled("▀", style));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// A pixel's color, or `None` where it is transparent enough to show the
/// terminal's own background through.
///
/// The threshold is a hard cutoff rather than a blend: there is nothing to
/// blend *with*, since the terminal background is whatever the user's theme
/// says and is not knowable from here. `None` becomes `Color::Reset`, which
/// leaves that half-cell painted in the terminal's own colors.
fn px(p: &Rgba<u8>) -> Option<Color> {
    if p.0[3] < 32 {
        None
    } else {
        Some(Color::Rgb(p.0[0], p.0[1], p.0[2]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn write_png(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let mut img = RgbaImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            // A gradient, so scaling and orientation are visible in the output.
            *p = Rgba([
                (x * 255 / w.max(1)) as u8,
                (y * 255 / h.max(1)) as u8,
                128,
                255,
            ]);
        }
        let path = dir.join(name);
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn classifies_by_extension() {
        assert_eq!(classify(Path::new("a.png")), Kind::Image);
        assert_eq!(classify(Path::new("a.JPG")), Kind::Image);
        assert_eq!(classify(Path::new("a.mp4")), Kind::Video);
        assert_eq!(classify(Path::new("a.mkv")), Kind::Video);
        assert_eq!(classify(Path::new("a.rs")), Kind::Other);
        assert_eq!(classify(Path::new("noext")), Kind::Other);
    }

    #[test]
    fn an_image_fills_the_pane_without_overflowing_it() {
        let td = tempfile::tempdir().unwrap();
        let p = write_png(td.path(), "grad.png", 64, 64);
        let out = render(&p, Kind::Image, 40, 20).unwrap();

        assert!(!out.lines.is_empty());
        assert!(out.lines.len() <= 20, "fits the row budget");
        for l in &out.lines {
            let cells: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(cells <= 40, "line is {cells} cells wide, budget was 40");
        }
        assert!(out.note.contains("64x64"), "{}", out.note);
        assert!(out.note.contains("PNG"), "{}", out.note);
    }

    #[test]
    fn aspect_ratio_is_preserved_for_a_wide_image() {
        let td = tempfile::tempdir().unwrap();
        let p = write_png(td.path(), "wide.png", 100, 25);
        let out = render(&p, Kind::Image, 40, 40).unwrap();
        // 4:1 source in a square pane: it should be width-bound and short.
        let widest = out
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.chars().count())
                    .sum::<usize>()
            })
            .max()
            .unwrap();
        assert_eq!(widest, 40, "uses the full width");
        assert!(
            out.lines.len() <= 6,
            "a 4:1 image should be about 5 rows tall, got {}",
            out.lines.len()
        );
    }

    #[test]
    fn a_tall_image_is_centred_rather_than_hugging_the_edge() {
        let td = tempfile::tempdir().unwrap();
        let p = write_png(td.path(), "tall.png", 20, 200);
        let out = render(&p, Kind::Image, 60, 20).unwrap();
        let first = &out.lines[0];
        assert!(
            first.spans[0].content.starts_with(' '),
            "expected leading padding to centre it"
        );
    }

    #[test]
    fn transparent_pixels_show_the_terminal_through() {
        let td = tempfile::tempdir().unwrap();
        let mut img = RgbaImage::new(4, 4);
        for p in img.pixels_mut() {
            *p = Rgba([255, 0, 0, 0]); // fully transparent
        }
        let path = td.path().join("clear.png");
        img.save(&path).unwrap();

        let out = render(&path, Kind::Image, 10, 6).unwrap();
        let style = out.lines[0].spans[0].style;
        assert_eq!(style.fg, Some(Color::Reset));
        assert_eq!(style.bg, Some(Color::Reset));
    }

    #[test]
    fn every_cell_is_a_single_half_block() {
        let td = tempfile::tempdir().unwrap();
        let p = write_png(td.path(), "g.png", 8, 8);
        let out = render(&p, Kind::Image, 8, 8).unwrap();
        for line in &out.lines {
            for span in &line.spans {
                assert!(
                    span.content.chars().all(|c| c == '▀' || c == ' '),
                    "unexpected glyph {:?}",
                    span.content
                );
            }
        }
    }

    #[test]
    fn the_picture_is_in_the_colors_not_the_glyphs() {
        let td = tempfile::tempdir().unwrap();
        // A dark disc on a light ground: every cell is the same character, so
        // the shape can only survive if the colors do.
        let (w, h) = (96u32, 96u32);
        let mut img = RgbaImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let (dx, dy) = (x as f32 - w as f32 / 2.0, y as f32 - h as f32 / 2.0);
            let inside = dx * dx + dy * dy < (w as f32 * 0.38).powi(2);
            *p = if inside {
                Rgba([20, 30, 40, 255])
            } else {
                Rgba([230, 230, 225, 255])
            };
        }
        let path = td.path().join("disc.png");
        img.save(&path).unwrap();

        let out = render(&path, Kind::Image, 40, 20).unwrap();
        let mid_row = &out.lines[out.lines.len() / 2];
        let cells: Vec<&Span> = mid_row
            .spans
            .iter()
            .filter(|s| s.content.contains('▀'))
            .collect();
        assert!(cells.len() > 10);

        let centre = cells[cells.len() / 2].style.fg.unwrap();
        let edge = cells[0].style.fg.unwrap();
        assert_ne!(centre, edge, "the disc and the ground must differ");
        // And the disc really is the dark one.
        let brightness = |c: Color| match c {
            Color::Rgb(r, g, b) => r as u32 + g as u32 + b as u32,
            other => panic!("expected an rgb pixel, got {other:?}"),
        };
        assert!(
            brightness(centre) < brightness(edge),
            "the middle of the image should be the dark disc"
        );
    }

    #[test]
    fn a_pane_too_small_to_draw_in_says_so() {
        let td = tempfile::tempdir().unwrap();
        let p = write_png(td.path(), "g.png", 8, 8);
        assert!(render(&p, Kind::Image, 1, 10).is_err());
        assert!(render(&p, Kind::Image, 10, 1).is_err());
    }

    #[test]
    fn a_file_that_is_not_an_image_fails_cleanly() {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("fake.png");
        std::fs::write(&p, b"this is not a png").unwrap();
        let err = render(&p, Kind::Image, 20, 10).unwrap_err();
        assert!(
            format!("{err:#}").contains("fake.png"),
            "the message should name the file: {err:#}"
        );
    }

    #[test]
    fn video_without_ffmpeg_explains_itself() {
        // Only meaningful where ffmpeg is absent; where it exists the call
        // fails on the bogus contents instead, which is equally fine.
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("clip.mp4");
        std::fs::write(&p, b"not really a video").unwrap();
        let err = render(&p, Kind::Video, 40, 20).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("ffmpeg"),
            "the message should mention ffmpeg: {msg}"
        );
    }
}
