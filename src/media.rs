//! What kind of file an agent sends, as Matrix shows it: from its first
//! bytes, else from its name.

/// Bytes of a file's start that `kind` needs at most
pub const HEAD: usize = 256 * 1024;

/// The message type and media type of a file, and the width and height of
/// a picture when its header says them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kind {
    pub msgtype: &'static str,
    pub mimetype: &'static str,
    pub size: Option<(u32, u32)>,
}

/// The kind of the file `name`, whose first bytes are `head`.
#[must_use]
pub fn kind(name: &str, head: &[u8]) -> Kind {
    let image = |mimetype, size| Kind {
        msgtype: "m.image",
        mimetype,
        size,
    };
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return image("image/png", png(head));
    }
    if head.starts_with(&[0xff, 0xd8, 0xff]) {
        return image("image/jpeg", jpeg(head));
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return image("image/gif", pair(le16(head, 6), le16(head, 8)));
    }
    if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP") {
        return image("image/webp", webp(head));
    }
    let file = |msgtype, mimetype| Kind {
        msgtype,
        mimetype,
        size: None,
    };
    if head.starts_with(b"%PDF-") {
        return file("m.file", "application/pdf");
    }
    let extension = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "mp3" => file("m.audio", "audio/mpeg"),
        "ogg" | "oga" | "opus" => file("m.audio", "audio/ogg"),
        "m4a" => file("m.audio", "audio/mp4"),
        "wav" => file("m.audio", "audio/wav"),
        "flac" => file("m.audio", "audio/flac"),
        "mp4" | "m4v" => file("m.video", "video/mp4"),
        "webm" => file("m.video", "video/webm"),
        "mov" => file("m.video", "video/quicktime"),
        "mkv" => file("m.video", "video/x-matroska"),
        "txt" | "log" => file("m.file", "text/plain"),
        "md" => file("m.file", "text/markdown"),
        "csv" => file("m.file", "text/csv"),
        "json" => file("m.file", "application/json"),
        _ => file("m.file", "application/octet-stream"),
    }
}

fn pair(w: Option<u32>, h: Option<u32>) -> Option<(u32, u32)> {
    Some((w?, h?)).filter(|(w, h)| *w > 0 && *h > 0)
}

fn le16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes(
        b.get(at..at + 2)?.try_into().ok()?,
    )))
}

fn be16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes(
        b.get(at..at + 2)?.try_into().ok()?,
    )))
}

fn le24(b: &[u8], at: usize) -> Option<u32> {
    let x = b.get(at..at + 3)?;
    Some(u32::from_le_bytes([x[0], x[1], x[2], 0]))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The size in the IHDR chunk, which comes first.
fn png(b: &[u8]) -> Option<(u32, u32)> {
    if b.get(12..16) != Some(b"IHDR") {
        return None;
    }
    pair(be32(b, 16), be32(b, 20))
}

/// The size in the first start-of-frame segment.
fn jpeg(b: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    loop {
        if *b.get(at)? != 0xff {
            return None;
        }
        let marker = *b.get(at + 1)?;
        match marker {
            // Fill bytes before a marker
            0xff => at += 1,
            // Markers without a length
            0x01 | 0xd0..=0xd7 => at += 2,
            0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                return pair(be16(b, at + 7), be16(b, at + 5));
            }
            _ => at += 2 + usize::try_from(be16(b, at + 2)?).ok()?,
        }
    }
}

/// The size of a lossy, lossless or extended WebP.
fn webp(b: &[u8]) -> Option<(u32, u32)> {
    match b.get(12..16)? {
        b"VP8 " if b.get(23..26) == Some(&[0x9d, 0x01, 0x2a]) => pair(
            le16(b, 26).map(|w| w & 0x3fff),
            le16(b, 28).map(|h| h & 0x3fff),
        ),
        b"VP8L" if b.get(20) == Some(&0x2f) => {
            let bits = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
            pair(Some((bits & 0x3fff) + 1), Some(((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8X" => pair(le24(b, 24).map(|w| w + 1), le24(b, 27).map(|h| h + 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0]);
        b
    }

    fn webp(chunk: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = b"RIFF\0\0\0\0WEBP".to_vec();
        b.extend(chunk);
        b.extend([0; 4]);
        b.extend(body);
        b
    }

    fn image(mimetype: &'static str, size: Option<(u32, u32)>) -> Kind {
        Kind {
            msgtype: "m.image",
            mimetype,
            size,
        }
    }

    #[test]
    fn png_gif_and_jpeg_carry_their_size() {
        assert_eq!(
            kind("x", &png(640, 480)),
            image("image/png", Some((640, 480)))
        );
        assert_eq!(
            kind("x.png", &png(640, 480)[..20]),
            image("image/png", None)
        );
        let gif = b"GIF89a\x80\x02\xe0\x01\0\0";
        assert_eq!(kind("x", gif), image("image/gif", Some((640, 480))));
        // An APP0 segment, a fill byte, then a baseline frame of 640 by 480
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0, 4, 1, 2, 0xff];
        jpeg.extend([0xff, 0xc0, 0, 17, 8, 0x01, 0xe0, 0x02, 0x80, 3]);
        assert_eq!(kind("x", &jpeg), image("image/jpeg", Some((640, 480))));
        // A table before the frame is skipped
        let mut table = vec![0xff, 0xd8, 0xff, 0xc4, 0, 3, 0];
        table.extend(&jpeg[2..]);
        assert_eq!(kind("x", &table), image("image/jpeg", Some((640, 480))));
        assert_eq!(kind("x", &jpeg[..12]), image("image/jpeg", None));
    }

    #[test]
    fn webp_has_three_forms() {
        let lossy = webp(
            *b"VP8 ",
            &[0, 0, 0, 0x9d, 0x01, 0x2a, 0x80, 0x02, 0xe0, 0x01],
        );
        assert_eq!(kind("x", &lossy), image("image/webp", Some((640, 480))));
        // 640 - 1 and 480 - 1 in two fields of 14 bits
        let bits: u32 = 0x27f | (0x1df << 14);
        let mut lossless = vec![0x2f];
        lossless.extend(bits.to_le_bytes());
        let lossless = webp(*b"VP8L", &lossless);
        assert_eq!(kind("x", &lossless), image("image/webp", Some((640, 480))));
        let extended = webp(*b"VP8X", &[0, 0, 0, 0, 0x7f, 0x02, 0, 0xdf, 0x01, 0]);
        assert_eq!(kind("x", &extended), image("image/webp", Some((640, 480))));
        assert_eq!(
            kind("x", &webp(*b"VP8 ", &[0; 10])),
            image("image/webp", None)
        );
        assert_eq!(kind("x", &webp(*b"ALPH", &[])), image("image/webp", None));
    }

    #[test]
    fn others_go_by_their_start_or_their_name() {
        let file = |msgtype, mimetype| Kind {
            msgtype,
            mimetype,
            size: None,
        };
        for (name, head, want) in [
            (
                "report",
                &b"%PDF-1.7"[..],
                file("m.file", "application/pdf"),
            ),
            ("song.MP3", b"ID3", file("m.audio", "audio/mpeg")),
            ("note.ogg", b"OggS", file("m.audio", "audio/ogg")),
            ("clip.mp4", b"\0\0\0\x18ftyp", file("m.video", "video/mp4")),
            ("clip.webm", b"\x1a\x45", file("m.video", "video/webm")),
            ("notes.txt", b"hello", file("m.file", "text/plain")),
            (
                "fake.png",
                b"hello",
                file("m.file", "application/octet-stream"),
            ),
            ("noext", b"", file("m.file", "application/octet-stream")),
        ] {
            assert_eq!(kind(name, head), want, "{name}");
        }
    }
}
