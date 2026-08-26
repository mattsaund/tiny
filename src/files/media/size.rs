//! Pixel dimensions, read straight out of a file header.
//!
//! Every image format writes its size within the first few dozen bytes, in a
//! fixed place, in a documented layout. So tiny reads those bytes and stops.
//!
//! # Why not a decoder
//!
//! This used to go through the `image` crate, which is an excellent decoder
//! and brings twenty-odd crates with it — PNG, JPEG, GIF, WebP and TIFF
//! decoders, two inflate implementations, a colour-management library. That is
//! the right dependency for a program that draws pictures. tiny stopped
//! drawing pictures (see the [`super`] module docs), and what is left is
//! reading two integers.
//!
//! # Failure is a missing line, not an error
//!
//! Every read here is bounds-checked and every parse returns `Option`, so a
//! truncated file, a format that is not really the one the extension claims,
//! or a layout this does not know about all end the same way: no resolution in
//! the pane, and a note saying so. Nothing here can panic on hostile input,
//! and nothing here needs to be right for the rest of the preview to work.
//!
//! # I/O is a few reads, not a slurp
//!
//! [`read`] seeks to what it needs. Most formats are answered by the first 32
//! bytes; JPEG walks its segment chain because a big EXIF block can sit in
//! front of the frame header, and TIFF follows one offset to its directory.
//! Nothing reads the pixels.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, anyhow};

/// A JPEG whose frame header is not within this many bytes of the start is
/// treated as unreadable. Real files put it within a few hundred; the cap is
/// what stops a corrupt or hostile file from walking us through gigabytes of
/// invented segment lengths.
const MAX_JPEG_SCAN: u64 = 4 * 1024 * 1024;

/// The dimensions of the image in `path`, from its header alone.
///
/// The format is identified by its magic bytes, not by the file's extension,
/// so a PNG saved as `.jpg` still reports its true size.
pub(super) fn read(path: &Path) -> Result<(u32, u32)> {
    let mut file = File::open(path).with_context(|| format!("cannot read {}", name(path)))?;
    let head = at(&mut file, 0, 32)?;
    let unknown = || anyhow!("{} is not an image tiny can measure", name(path));

    let size = match () {
        _ if head.starts_with(b"\x89PNG\r\n\x1a\n") => png(&head),
        _ if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") => gif(&head),
        _ if head.starts_with(b"BM") => bmp(&head),
        _ if head.starts_with(b"\xff\xd8") => jpeg(&mut file)?,
        _ if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP") => webp(&mut file)?,
        _ if head.starts_with(b"II\x2a\x00") || head.starts_with(b"MM\x00\x2a") => {
            tiff(&mut file, &head)?
        }
        _ if head.starts_with(b"\x00\x00\x01\x00") => ico(&head),
        _ => return Err(unknown()),
    };
    // A zero dimension is a corrupt header, not a zero-sized picture.
    size.filter(|&(w, h)| w > 0 && h > 0)
        .ok_or_else(|| anyhow!("the header of {} is damaged", name(path)))
}

/// `len` bytes from `off`. A short read is not an error — it comes back short,
/// and the parser that asked for it fails its own bounds check.
fn at(file: &mut File, off: u64, len: usize) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(off))?;
    let mut buf = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    buf.truncate(filled);
    Ok(buf)
}

// ---- the formats ----------------------------------------------------------

/// IHDR is always the first chunk, so the size is at a fixed offset.
fn png(b: &[u8]) -> Option<(u32, u32)> {
    (b.get(12..16)? == b"IHDR").then_some(())?;
    Some((be32(b, 16)?, be32(b, 20)?))
}

/// The logical screen descriptor, immediately after the six-byte signature.
fn gif(b: &[u8]) -> Option<(u32, u32)> {
    Some((le16(b, 6)? as u32, le16(b, 8)? as u32))
}

/// The BITMAPINFOHEADER. Height is signed: negative means the rows are stored
/// top-down, which says nothing about how tall the picture is.
fn bmp(b: &[u8]) -> Option<(u32, u32)> {
    let w = le32(b, 18)? as i32;
    let h = le32(b, 22)? as i32;
    Some((w.unsigned_abs(), h.unsigned_abs()))
}

/// The first entry of the directory. A dimension of 0 means 256 — the field is
/// one byte and 256 does not fit in it.
fn ico(b: &[u8]) -> Option<(u32, u32)> {
    let byte = |i: usize| {
        b.get(i)
            .copied()
            .map(|v| if v == 0 { 256 } else { v as u32 })
    };
    Some((byte(6)?, byte(7)?))
}

/// Walk the segment chain to the frame header.
///
/// JPEG puts the size in a start-of-frame segment, and there is no telling how
/// far in that is: EXIF, ICC profiles and embedded thumbnails all come first
/// and can be tens of kilobytes each. So the segments are stepped through by
/// their declared lengths until a start-of-frame turns up.
///
/// The `0xC4`, `0xC8` and `0xCC` exclusions matter: those three markers sit in
/// the middle of the start-of-frame range but mean something else entirely
/// (Huffman tables, an extension, arithmetic coding), and reading a size out
/// of one gives a confident wrong answer.
fn jpeg(file: &mut File) -> Result<Option<(u32, u32)>> {
    let mut pos = 2u64;
    while pos < MAX_JPEG_SCAN {
        let header = at(file, pos, 4)?;
        let [mut lead, mut marker] = [*header.first().unwrap_or(&0), *header.get(1).unwrap_or(&0)];
        if lead != 0xFF {
            return Ok(None);
        }
        // Any number of 0xFF bytes may pad the gap before a marker.
        let mut skipped = 0u64;
        while marker == 0xFF {
            skipped += 1;
            let next = at(file, pos + 1 + skipped, 2)?;
            lead = 0xFF;
            marker = *next.first().unwrap_or(&0);
        }
        let _ = lead;
        pos += 2 + skipped;
        match marker {
            // Start of frame, in every one of its flavours.
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                let seg = at(file, pos, 9)?;
                // Two bytes of length, a byte of precision, then height —
                // JPEG is the one format here that writes it that way round.
                return Ok(match (be16(&seg, 5), be16(&seg, 3)) {
                    (Some(w), Some(h)) => Some((w as u32, h as u32)),
                    _ => None,
                });
            }
            // Standalone markers: no length field to skip.
            0x01 | 0xD0..=0xD7 => {}
            // Start of scan or end of image: the header is over.
            0xDA | 0xD9 => return Ok(None),
            _ => {
                let seg = at(file, pos, 2)?;
                let len = match be16(&seg, 0) {
                    // A length below 2 does not include its own field, so it
                    // cannot be right and stepping by it would not terminate.
                    Some(n) if n >= 2 => n as u64,
                    _ => return Ok(None),
                };
                pos += len;
            }
        }
    }
    Ok(None)
}

/// One of three sub-formats, told apart by the first chunk's identifier.
///
/// All three store the dimensions minus one, because a WebP cannot be zero
/// pixels wide and spending a code point on saying so would be wasteful.
fn webp(file: &mut File) -> Result<Option<(u32, u32)>> {
    let b = at(file, 12, 18)?;
    let Some(tag) = b.get(0..4) else {
        return Ok(None);
    };
    Ok(match tag {
        // Extended: an explicit canvas size, 24 bits each.
        b"VP8X" => match (le24(&b, 12), le24(&b, 15)) {
            (Some(w), Some(h)) => Some((w + 1, h + 1)),
            _ => None,
        },
        // Lossless: 14 bits each, packed after a one-byte signature.
        b"VP8L" => match le32(&b, 9) {
            Some(bits) if b.get(8) == Some(&0x2F) => {
                Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
            }
            _ => None,
        },
        // Lossy: the VP8 keyframe header, behind its start code.
        b"VP8 " => match (le16(&b, 14), le16(&b, 16)) {
            (Some(w), Some(h)) if b.get(11..14) == Some(&[0x9D, 0x01, 0x2A][..]) => {
                Some(((w & 0x3FFF) as u32, (h & 0x3FFF) as u32))
            }
            _ => None,
        },
        _ => None,
    })
}

/// Follow the offset to the first image file directory and look for the two
/// size tags in it.
///
/// TIFF is the only one of these that is genuinely a database rather than a
/// header: the fields are tagged, unordered, and live wherever an offset says.
/// Both byte orders are in the wild, which is what the magic bytes announce.
fn tiff(file: &mut File, head: &[u8]) -> Result<Option<(u32, u32)>> {
    let big_endian = head.starts_with(b"MM");
    let u16at = |b: &[u8], i: usize| {
        if big_endian { be16(b, i) } else { le16(b, i) }
    };
    let u32at = |b: &[u8], i: usize| {
        if big_endian { be32(b, i) } else { le32(b, i) }
    };

    let Some(ifd) = u32at(head, 4) else {
        return Ok(None);
    };
    let count_bytes = at(file, ifd as u64, 2)?;
    let Some(count) = u16at(&count_bytes, 0) else {
        return Ok(None);
    };
    // Twelve bytes per entry. A directory claiming thousands of them is
    // corrupt, and reading it would be a large allocation on a hover.
    let entries = at(file, ifd as u64 + 2, count.min(512) as usize * 12)?;

    let (mut width, mut height) = (None, None);
    for e in entries.as_chunks::<12>().0 {
        let (Some(tag), Some(kind)) = (u16at(e, 0), u16at(e, 2)) else {
            continue;
        };
        // The value sits inline when it fits in four bytes, which both a SHORT
        // and a LONG always do.
        let value = match kind {
            3 => u16at(e, 8).map(u32::from), // SHORT
            4 => u32at(e, 8),                // LONG
            _ => None,
        };
        match tag {
            256 => width = value,
            257 => height = value,
            _ => {}
        }
        if let (Some(w), Some(h)) = (width, height) {
            return Ok(Some((w, h)));
        }
    }
    Ok(None)
}

// ---- reading numbers out of bytes -----------------------------------------

fn be16(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn le16(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn le32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// A 24-bit little-endian integer, which only WebP uses.
fn le24(b: &[u8], i: usize) -> Option<u32> {
    let s = b.get(i..i + 3)?;
    Some(u32::from(s[0]) | u32::from(s[1]) << 8 | u32::from(s[2]) << 16)
}

/// The file name, for messages. Falls back to the whole path.
fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Write bytes to a file and measure them. Only the header has to be
    /// real — nothing here decodes anything, so there are no pixels to
    /// provide and the fixtures can be written by hand.
    fn measure(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> Result<(u32, u32)> {
        let p: PathBuf = dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        read(&p)
    }

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend_from_slice(&13u32.to_be_bytes()); // IHDR length
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0]); // depth, colour, the rest
        v
    }

    #[test]
    fn a_png_reports_its_size() {
        let td = tempfile::tempdir().unwrap();
        assert_eq!(
            measure(&td, "a.png", &png_bytes(1920, 1080)).unwrap(),
            (1920, 1080)
        );
    }

    #[test]
    fn the_format_comes_from_the_bytes_not_the_name() {
        // A PNG wearing a .jpg suffix still measures as a PNG.
        let td = tempfile::tempdir().unwrap();
        assert_eq!(
            measure(&td, "liar.jpg", &png_bytes(33, 17)).unwrap(),
            (33, 17)
        );
    }

    #[test]
    fn a_gif_reports_its_size() {
        let td = tempfile::tempdir().unwrap();
        let mut v = b"GIF89a".to_vec();
        v.extend_from_slice(&640u16.to_le_bytes());
        v.extend_from_slice(&480u16.to_le_bytes());
        v.extend_from_slice(&[0; 8]);
        assert_eq!(measure(&td, "a.gif", &v).unwrap(), (640, 480));
    }

    #[test]
    fn a_bitmap_is_as_tall_as_it_is_however_its_rows_are_stored() {
        let td = tempfile::tempdir().unwrap();
        let bmp = |h: i32| {
            let mut v = b"BM".to_vec();
            v.extend_from_slice(&[0; 16]); // file header, then the info header size
            v.extend_from_slice(&100i32.to_le_bytes());
            v.extend_from_slice(&h.to_le_bytes());
            v.extend_from_slice(&[0; 8]);
            v
        };
        assert_eq!(measure(&td, "up.bmp", &bmp(50)).unwrap(), (100, 50));
        // A negative height means top-down rows, not a negative picture.
        assert_eq!(measure(&td, "down.bmp", &bmp(-50)).unwrap(), (100, 50));
    }

    #[test]
    fn an_icon_of_no_stated_size_is_the_one_size_that_does_not_fit() {
        let td = tempfile::tempdir().unwrap();
        let ico = |w: u8, h: u8| {
            let mut v = vec![0, 0, 1, 0, 1, 0];
            v.extend_from_slice(&[w, h, 0, 0, 1, 0, 32, 0]);
            v.extend_from_slice(&[0; 16]);
            v
        };
        assert_eq!(measure(&td, "small.ico", &ico(48, 48)).unwrap(), (48, 48));
        assert_eq!(measure(&td, "big.ico", &ico(0, 0)).unwrap(), (256, 256));
    }

    /// A JPEG with `pad` bytes of segments in front of the frame header, so
    /// the walk has something to walk.
    fn jpeg_bytes(w: u16, h: u16, pad: usize) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        if pad > 0 {
            v.extend_from_slice(&[0xFF, 0xE1]); // APP1, where EXIF lives
            v.extend_from_slice(&((pad + 2) as u16).to_be_bytes());
            v.extend(std::iter::repeat_n(0u8, pad));
        }
        v.extend_from_slice(&[0xFF, 0xC0]); // SOF0
        v.extend_from_slice(&11u16.to_be_bytes());
        v.push(8); // precision
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&[3, 1, 0x11, 0]);
        v
    }

    #[test]
    fn a_jpeg_reports_its_size_however_much_metadata_is_in_front_of_it() {
        let td = tempfile::tempdir().unwrap();
        assert_eq!(
            measure(&td, "bare.jpg", &jpeg_bytes(800, 600, 0)).unwrap(),
            (800, 600)
        );
        // A camera's EXIF block with a thumbnail in it is easily this big.
        assert_eq!(
            measure(&td, "exif.jpg", &jpeg_bytes(4032, 3024, 60_000)).unwrap(),
            (4032, 3024)
        );
    }

    #[test]
    fn a_jpeg_huffman_table_is_not_mistaken_for_a_frame_header() {
        // 0xC4 sits inside the start-of-frame marker range but is a Huffman
        // table. Reading a size out of it would give a confident wrong answer.
        let td = tempfile::tempdir().unwrap();
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xC4];
        v.extend_from_slice(&20u16.to_be_bytes());
        v.extend_from_slice(&[0xAB; 18]);
        v.extend_from_slice(&jpeg_bytes(320, 240, 0)[2..]);
        assert_eq!(measure(&td, "huff.jpg", &v).unwrap(), (320, 240));
    }

    fn webp_bytes(chunk: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&((payload.len() + 12) as u32).to_le_bytes());
        v.extend_from_slice(b"WEBP");
        v.extend_from_slice(chunk);
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn all_three_kinds_of_webp_report_their_size() {
        let td = tempfile::tempdir().unwrap();

        // Extended: an explicit canvas, stored minus one.
        let mut x = vec![0u8; 10];
        x[4..7].copy_from_slice(&(1919u32.to_le_bytes()[..3]));
        x[7..10].copy_from_slice(&(1079u32.to_le_bytes()[..3]));
        assert_eq!(
            measure(&td, "x.webp", &webp_bytes(b"VP8X", &x)).unwrap(),
            (1920, 1080)
        );

        // Lossless: 14 bits each, packed behind a signature byte.
        let bits = (639u32) | (479u32 << 14);
        let mut l = vec![0x2F];
        l.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(
            measure(&td, "l.webp", &webp_bytes(b"VP8L", &l)).unwrap(),
            (640, 480)
        );

        // Lossy: the VP8 keyframe header, behind its start code.
        let mut y = vec![0u8; 3];
        y.extend_from_slice(&[0x9D, 0x01, 0x2A]);
        y.extend_from_slice(&300u16.to_le_bytes());
        y.extend_from_slice(&200u16.to_le_bytes());
        assert_eq!(
            measure(&td, "y.webp", &webp_bytes(b"VP8 ", &y)).unwrap(),
            (300, 200)
        );
    }

    /// A one-directory TIFF holding just the two size tags.
    fn tiff_bytes(w: u32, h: u32, big_endian: bool) -> Vec<u8> {
        let u16b = |v: u16| {
            if big_endian {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let u32b = |v: u32| {
            if big_endian {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let mut v = if big_endian {
            b"MM\x00\x2a".to_vec()
        } else {
            b"II\x2a\x00".to_vec()
        };
        v.extend_from_slice(&u32b(8)); // the directory starts right after
        v.extend_from_slice(&u16b(2)); // two entries
        for (tag, value) in [(256u16, w), (257, h)] {
            v.extend_from_slice(&u16b(tag));
            v.extend_from_slice(&u16b(4)); // LONG
            v.extend_from_slice(&u32b(1)); // one of them
            v.extend_from_slice(&u32b(value));
        }
        v.extend_from_slice(&u32b(0)); // no next directory
        v
    }

    #[test]
    fn a_tiff_reports_its_size_in_either_byte_order() {
        let td = tempfile::tempdir().unwrap();
        assert_eq!(
            measure(&td, "le.tif", &tiff_bytes(2048, 1536, false)).unwrap(),
            (2048, 1536)
        );
        assert_eq!(
            measure(&td, "be.tif", &tiff_bytes(2048, 1536, true)).unwrap(),
            (2048, 1536)
        );
    }

    #[test]
    fn nothing_hostile_gets_further_than_a_message() {
        let td = tempfile::tempdir().unwrap();
        let cases: &[(&str, Vec<u8>)] = &[
            ("empty.png", Vec::new()),
            ("text.png", b"this is not a png".to_vec()),
            // A truncated header: the magic is right and the size is not there.
            ("cut.png", png_bytes(10, 10)[..18].to_vec()),
            // A zero dimension is a damaged header, not a zero-sized picture.
            ("zero.png", png_bytes(0, 100)),
            // A JPEG that never reaches a frame header.
            ("endless.jpg", vec![0xFF, 0xD8, 0xFF, 0xE0, 0xFF, 0xFF]),
            // A TIFF whose directory offset points past the end of the file.
            ("wild.tif", b"II\x2a\x00\xff\xff\xff\x7f".to_vec()),
            ("short.webp", b"RIFF\x04\x00\x00\x00WEBPVP8X".to_vec()),
        ];
        for (name, bytes) in cases {
            assert!(
                measure(&td, name, bytes).is_err(),
                "{name} should have been refused, not measured"
            );
        }
    }
}
