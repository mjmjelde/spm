//! Minimal ar archive writer for DEB package assembly.
//!
//! Implements the ar archive format as used by Debian packages:
//! - `!<arch>\n` global magic header (8 bytes)
//! - Member headers: 60 bytes each
//! - Even-byte padding between members

use std::io::{self, Write};

/// ar global magic header.
const AR_MAGIC: &[u8; 8] = b"!<arch>\n";

/// ar member header terminator (file magic).
const AR_FMAG: &[u8; 2] = b"`\n";

/// Writer for ar archives (DEB container format).
///
/// DEB files are ar archives with a specific member ordering:
/// 1. `debian-binary` — format version
/// 2. `control.tar.{zst,gz}` — control metadata
/// 3. `data.tar.{zst,gz}` — package files
pub struct ArWriter<W: Write> {
    writer: W,
    wrote_magic: bool,
    /// Whether the current streaming member needs a padding byte on finish.
    needs_pad: bool,
}

impl<W: Write> ArWriter<W> {
    /// Create a new ar archive writer.
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            wrote_magic: false,
            needs_pad: false,
        }
    }

    /// Write the ar global magic header if not already written.
    fn ensure_magic(&mut self) -> io::Result<()> {
        if !self.wrote_magic {
            self.writer.write_all(AR_MAGIC)?;
            self.wrote_magic = true;
        }
        Ok(())
    }

    /// Write a complete member with data already in memory.
    pub fn write_member(
        &mut self,
        name: &str,
        data: &[u8],
        mtime: u64,
        mode: u32,
    ) -> io::Result<()> {
        self.ensure_magic()?;
        self.write_header(name, data.len() as u64, mtime, mode)?;
        self.writer.write_all(data)?;
        if data.len() % 2 != 0 {
            self.writer.write_all(b"\n")?;
        }
        Ok(())
    }

    /// Begin a streaming member. The caller must write exactly `size` bytes
    /// via [`writer_mut()`](Self::writer_mut), then call
    /// [`finish_member()`](Self::finish_member).
    pub fn begin_member(&mut self, name: &str, size: u64, mtime: u64, mode: u32) -> io::Result<()> {
        self.ensure_magic()?;
        self.write_header(name, size, mtime, mode)?;
        self.needs_pad = size % 2 != 0;
        Ok(())
    }

    /// Get a mutable reference to the inner writer for streaming data
    /// after [`begin_member()`](Self::begin_member).
    pub fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    /// Finish a streaming member (adds padding byte if size was odd).
    pub fn finish_member(&mut self) -> io::Result<()> {
        if self.needs_pad {
            self.writer.write_all(b"\n")?;
            self.needs_pad = false;
        }
        Ok(())
    }

    /// Consume the writer and return the inner writer.
    pub fn finish(self) -> io::Result<W> {
        Ok(self.writer)
    }

    /// Write a 60-byte ar member header.
    ///
    /// Format (all ASCII, space-padded to field width):
    /// - name: 16 bytes (terminated with `/`)
    /// - mtime: 12 bytes (decimal)
    /// - uid: 6 bytes (decimal, always "0")
    /// - gid: 6 bytes (decimal, always "0")
    /// - mode: 8 bytes (octal)
    /// - size: 10 bytes (decimal)
    /// - fmag: 2 bytes (`` `\n ``)
    fn write_header(&mut self, name: &str, size: u64, mtime: u64, mode: u32) -> io::Result<()> {
        let name_field = format!("{name}/");
        write!(
            self.writer,
            "{:<16}{:<12}{:<6}{:<6}{:<8o}{:<10}",
            name_field, mtime, 0, 0, mode, size,
        )?;
        self.writer.write_all(AR_FMAG)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ar_magic() {
        let mut buf = Vec::new();
        let mut ar = ArWriter::new(&mut buf);
        ar.write_member("test", b"", 0, 0o100644).unwrap();
        assert_eq!(&buf[..8], b"!<arch>\n");
    }

    #[test]
    fn test_ar_magic_written_once() {
        let mut buf = Vec::new();
        let mut ar = ArWriter::new(&mut buf);
        ar.write_member("a", b"x", 0, 0o100644).unwrap();
        ar.write_member("b", b"y", 0, 0o100644).unwrap();
        // Magic appears only at the start
        let magic_count = buf.windows(8).filter(|w| *w == b"!<arch>\n").count();
        assert_eq!(magic_count, 1);
    }

    #[test]
    fn test_ar_header_size() {
        let mut buf = Vec::new();
        let mut ar = ArWriter::new(&mut buf);
        ar.write_member("test", b"", 0, 0o100644).unwrap();
        // 8 (magic) + 60 (header) = 68 bytes for empty member
        assert_eq!(buf.len(), 68);
    }

    #[test]
    fn test_ar_header_fields() {
        let mut buf = Vec::new();
        let mut ar = ArWriter::new(&mut buf);
        ar.write_member("debian-binary", b"2.0\n", 1700000000, 0o100644)
            .unwrap();
        let header = &buf[8..68];
        let header_str = std::str::from_utf8(header).unwrap();
        // name (16 chars): "debian-binary/  "
        assert_eq!(&header_str[..16], "debian-binary/  ");
        // mtime (12 chars): "1700000000  "
        assert_eq!(&header_str[16..28], "1700000000  ");
        // uid (6 chars): "0     "
        assert_eq!(&header_str[28..34], "0     ");
        // gid (6 chars): "0     "
        assert_eq!(&header_str[34..40], "0     ");
        // mode (8 chars): "100644  "
        assert_eq!(&header_str[40..48], "100644  ");
        // size (10 chars): "4         "
        assert_eq!(&header_str[48..58], "4         ");
        // fmag (2 chars): "`\n"
        assert_eq!(&header[58..60], b"`\n");
    }

    #[test]
    fn test_ar_member_data() {
        let mut buf = Vec::new();
        let mut ar = ArWriter::new(&mut buf);
        ar.write_member("test", b"hello", 0, 0o100644).unwrap();
        // Data starts after magic (8) + header (60) = 68
        assert_eq!(&buf[68..73], b"hello");
    }

    #[test]
    fn test_ar_odd_size_padding() {
        let mut buf = Vec::new();
        let mut ar = ArWriter::new(&mut buf);
        ar.write_member("test", b"hello", 0, 0o100644).unwrap();
        // 5 bytes is odd → 1 padding byte
        // Total: 8 + 60 + 5 + 1 = 74
        assert_eq!(buf.len(), 74);
        assert_eq!(buf[73], b'\n');
    }

    #[test]
    fn test_ar_even_size_no_padding() {
        let mut buf = Vec::new();
        let mut ar = ArWriter::new(&mut buf);
        ar.write_member("test", b"hi", 0, 0o100644).unwrap();
        // 2 bytes is even → no padding
        // Total: 8 + 60 + 2 = 70
        assert_eq!(buf.len(), 70);
    }

    #[test]
    fn test_ar_streaming_matches_in_memory() {
        let data = b"test data content";

        // In-memory
        let mut buf1 = Vec::new();
        let mut ar1 = ArWriter::new(&mut buf1);
        ar1.write_member("file.txt", data, 12345, 0o100644).unwrap();

        // Streaming
        let mut buf2 = Vec::new();
        let mut ar2 = ArWriter::new(&mut buf2);
        ar2.begin_member("file.txt", data.len() as u64, 12345, 0o100644)
            .unwrap();
        ar2.writer_mut().write_all(data).unwrap();
        ar2.finish_member().unwrap();

        assert_eq!(buf1, buf2);
    }

    #[test]
    fn test_ar_streaming_odd_size_padding() {
        let data = b"odd";
        let mut buf = Vec::new();
        let mut ar = ArWriter::new(&mut buf);
        ar.begin_member("f", data.len() as u64, 0, 0o100644)
            .unwrap();
        ar.writer_mut().write_all(data).unwrap();
        ar.finish_member().unwrap();
        // 8 + 60 + 3 + 1 = 72
        assert_eq!(buf.len(), 72);
    }

    #[test]
    fn test_ar_multiple_members() {
        let mut buf = Vec::new();
        let mut ar = ArWriter::new(&mut buf);
        ar.write_member("debian-binary", b"2.0\n", 0, 0o100644)
            .unwrap();
        ar.write_member("control.tar.zst", b"ctrl", 0, 0o100644)
            .unwrap();
        ar.write_member("data.tar.zst", b"data!", 0, 0o100644)
            .unwrap();

        // Verify structure:
        // magic (8) + header1 (60) + "2.0\n" (4) = 72
        // + header2 (60) + "ctrl" (4) = 136
        // + header3 (60) + "data!" (5) + pad (1) = 202
        assert_eq!(buf.len(), 202);

        // Verify first member data
        assert_eq!(&buf[68..72], b"2.0\n");
        // Verify second member data
        assert_eq!(&buf[132..136], b"ctrl");
        // Verify third member data
        assert_eq!(&buf[196..201], b"data!");
    }

    #[test]
    fn test_ar_empty_member() {
        let mut buf = Vec::new();
        let mut ar = ArWriter::new(&mut buf);
        ar.write_member("empty", b"", 0, 0o100644).unwrap();
        // 8 + 60 = 68 (no data, no padding)
        assert_eq!(buf.len(), 68);
    }

    #[test]
    fn test_ar_finish_returns_writer() {
        let buf = Vec::new();
        let ar = ArWriter::new(buf);
        let inner = ar.finish().unwrap();
        // Writer was empty, nothing written
        assert!(inner.is_empty());
    }
}
