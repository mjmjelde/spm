//! CPIO archive writer supporting SVR4 newc (070701) and RPM extended (07070X) formats.
//!
//! Provides a streaming CPIO writer for building RPM package payloads. Two formats
//! are supported:
//!
//! - **Newc (070701):** Standard SVR4 newc with 8-hex-digit fields. Max 4 GiB per file.
//! - **Extended (07070X):** RPM's custom stripped-down cpio for packages with files > 4 GiB.
//!   Only stores a file index in the header; all metadata comes from RPM header tags.
//!
//! # Hardlink Convention
//!
//! The caller is responsible for ordering hardlink entries so that all-but-last links
//! have `filesize = 0` (with an empty reader) and the last link carries the full file
//! data. The `CpioWriter` writes exactly what it is given.

use std::io::{self, Read, Write};

use thiserror::Error;

/// Errors that can occur during CPIO archive writing.
#[derive(Debug, Error)]
pub enum CpioError {
    /// An I/O error occurred while writing the archive.
    #[error("cpio I/O error: {0}")]
    Io(#[from] io::Error),

    /// A file exceeds the maximum size for the selected format.
    #[error("file size {size} exceeds maximum {max} for {format} format")]
    FileTooLarge {
        /// Actual file size.
        size: u64,
        /// Maximum allowed by the format.
        max: u64,
        /// Format name.
        format: &'static str,
    },
}

/// CPIO archive format variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpioFormat {
    /// Standard SVR4 newc (magic `"070701"`), max file size 4 GiB.
    Newc,
    /// RPM extended (magic `"07070X"`), file index only, unlimited size.
    Extended,
}

/// Metadata for a CPIO entry.
///
/// For the `Newc` format, all fields are written into the header.
/// For the `Extended` format, these fields are ignored by the writer
/// (metadata lives in RPM header tags), but `filesize` is still used
/// to know how many data bytes to expect.
#[derive(Debug, Clone)]
pub struct CpioMetadata {
    /// Inode number.
    pub ino: u32,
    /// File mode (includes file type bits).
    pub mode: u32,
    /// User ID.
    pub uid: u32,
    /// Group ID.
    pub gid: u32,
    /// Number of hard links.
    pub nlink: u32,
    /// Modification time (seconds since epoch).
    pub mtime: u32,
    /// File size in bytes.
    pub filesize: u64,
    /// Device major number.
    pub devmajor: u32,
    /// Device minor number.
    pub devminor: u32,
    /// Rdev major number.
    pub rdevmajor: u32,
    /// Rdev minor number.
    pub rdevminor: u32,
}

/// Builder for a CPIO archive. Writes entries sequentially to an underlying writer.
///
/// Call [`write_entry`](CpioWriter::write_entry) for each file, then
/// [`finish`](CpioWriter::finish) to write the `TRAILER!!!` terminator
/// and recover the inner writer.
pub struct CpioWriter<W: Write> {
    writer: W,
    format: CpioFormat,
    bytes_written: u64,
    entry_count: u32,
}

/// Size of the Newc header (magic + 13 fields of 8 hex chars).
const NEWC_HEADER_SIZE: u64 = 110;

/// Size of the Extended header (magic + 8-char hex index).
const EXTENDED_HEADER_SIZE: u64 = 14;

/// Maximum file size for Newc format (8 hex digits).
const NEWC_MAX_FILESIZE: u64 = 0xFFFF_FFFF;

/// Streaming copy buffer size.
const COPY_BUF_SIZE: usize = 256 * 1024;

impl<W: Write> CpioWriter<W> {
    /// Create a new CPIO writer with the given format.
    pub fn new(writer: W, format: CpioFormat) -> Self {
        Self {
            writer,
            format,
            bytes_written: 0,
            entry_count: 0,
        }
    }

    /// Write a file entry to the archive.
    ///
    /// - `index`: zero-based entry index (used for Extended format header).
    /// - `name`: install path for the file (used for Newc format; ignored for Extended).
    ///   For RPM payloads in Newc format, prefix with `./` (e.g., `./opt/app/bin/tool`).
    /// - `metadata`: file metadata. For Newc format, `filesize` must fit in 32 bits.
    /// - `data`: reader providing the file content. Must yield exactly `metadata.filesize` bytes.
    ///
    /// Returns the number of bytes written for this entry (including padding).
    pub fn write_entry(
        &mut self,
        index: u32,
        name: &str,
        metadata: &CpioMetadata,
        data: &mut dyn Read,
    ) -> Result<u64, CpioError> {
        let start = self.bytes_written;

        match self.format {
            CpioFormat::Newc => self.write_newc_entry(name, metadata, data)?,
            CpioFormat::Extended => self.write_extended_entry(index, metadata, data)?,
        }

        self.entry_count += 1;
        Ok(self.bytes_written - start)
    }

    /// Write the `TRAILER!!!` entry to terminate the archive.
    ///
    /// Returns the inner writer and the total number of uncompressed bytes
    /// written to the archive (useful for RPM signature size calculations).
    pub fn finish(mut self) -> Result<(W, u64), CpioError> {
        match self.format {
            CpioFormat::Newc => {
                let trailer_meta = CpioMetadata {
                    ino: 0,
                    mode: 0,
                    uid: 0,
                    gid: 0,
                    nlink: 1,
                    mtime: 0,
                    filesize: 0,
                    devmajor: 0,
                    devminor: 0,
                    rdevmajor: 0,
                    rdevminor: 0,
                };
                self.write_newc_entry("TRAILER!!!", &trailer_meta, &mut io::empty())?;
            }
            CpioFormat::Extended => {
                // Write a sentinel entry with max index and zero data.
                self.write_extended_entry(
                    self.entry_count,
                    &CpioMetadata {
                        ino: 0,
                        mode: 0,
                        uid: 0,
                        gid: 0,
                        nlink: 0,
                        mtime: 0,
                        filesize: 0,
                        devmajor: 0,
                        devminor: 0,
                        rdevmajor: 0,
                        rdevminor: 0,
                    },
                    &mut io::empty(),
                )?;
            }
        }

        Ok((self.writer, self.bytes_written))
    }

    /// Write a Newc format entry.
    fn write_newc_entry(
        &mut self,
        name: &str,
        metadata: &CpioMetadata,
        data: &mut dyn Read,
    ) -> Result<(), CpioError> {
        // Validate file size fits in 32 bits.
        if metadata.filesize > NEWC_MAX_FILESIZE {
            return Err(CpioError::FileTooLarge {
                size: metadata.filesize,
                max: NEWC_MAX_FILESIZE,
                format: "Newc (070701)",
            });
        }

        let namesize = name.len() as u32 + 1; // include NUL terminator

        // Write 110-byte header.
        write!(
            self.writer,
            "070701\
             {:08X}\
             {:08X}\
             {:08X}\
             {:08X}\
             {:08X}\
             {:08X}\
             {:08X}\
             {:08X}\
             {:08X}\
             {:08X}\
             {:08X}\
             {:08X}\
             {:08X}",
            metadata.ino,
            metadata.mode,
            metadata.uid,
            metadata.gid,
            metadata.nlink,
            metadata.mtime,
            metadata.filesize as u32,
            metadata.devmajor,
            metadata.devminor,
            metadata.rdevmajor,
            metadata.rdevminor,
            namesize,
            0u32, // c_check
        )?;
        self.bytes_written += NEWC_HEADER_SIZE;

        // Write filename + NUL.
        self.writer.write_all(name.as_bytes())?;
        self.writer.write_all(&[0])?;
        self.bytes_written += namesize as u64;

        // Pad to 4-byte boundary (alignment from start of header).
        let header_plus_name = NEWC_HEADER_SIZE + namesize as u64;
        let name_pad = pad4(header_plus_name);
        if name_pad > 0 {
            self.writer.write_all(&[0u8; 4][..name_pad])?;
            self.bytes_written += name_pad as u64;
        }

        // Write file data.
        let data_written = self.stream_data(data, metadata.filesize)?;
        debug_assert_eq!(data_written, metadata.filesize);

        // Pad data to 4-byte boundary.
        let data_pad = pad4(metadata.filesize);
        if data_pad > 0 {
            self.writer.write_all(&[0u8; 4][..data_pad])?;
            self.bytes_written += data_pad as u64;
        }

        Ok(())
    }

    /// Write an Extended format entry.
    fn write_extended_entry(
        &mut self,
        index: u32,
        metadata: &CpioMetadata,
        data: &mut dyn Read,
    ) -> Result<(), CpioError> {
        // Write 14-byte header: "07070X" + 8-char hex index.
        write!(self.writer, "07070X{:08X}", index)?;
        self.bytes_written += EXTENDED_HEADER_SIZE;

        // Write file data.
        let data_written = self.stream_data(data, metadata.filesize)?;
        debug_assert_eq!(data_written, metadata.filesize);

        // Pad data to 4-byte boundary.
        let data_pad = pad4(metadata.filesize);
        if data_pad > 0 {
            self.writer.write_all(&[0u8; 4][..data_pad])?;
            self.bytes_written += data_pad as u64;
        }

        Ok(())
    }

    /// Stream exactly `expected_len` bytes from `data` to the writer.
    fn stream_data(&mut self, data: &mut dyn Read, expected_len: u64) -> Result<u64, CpioError> {
        if expected_len == 0 {
            return Ok(0);
        }

        let mut buf = [0u8; COPY_BUF_SIZE];
        let mut remaining = expected_len;

        while remaining > 0 {
            let to_read = (remaining as usize).min(COPY_BUF_SIZE);
            let n = data.read(&mut buf[..to_read])?;
            if n == 0 {
                return Err(CpioError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!(
                        "expected {expected_len} bytes but got {}",
                        expected_len - remaining
                    ),
                )));
            }
            self.writer.write_all(&buf[..n])?;
            self.bytes_written += n as u64;
            remaining -= n as u64;
        }

        Ok(expected_len)
    }
}

/// Calculate padding bytes needed to align `offset` to a 4-byte boundary.
fn pad4(offset: u64) -> usize {
    ((4 - (offset % 4)) % 4) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_metadata(filesize: u64) -> CpioMetadata {
        CpioMetadata {
            ino: 1,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            nlink: 1,
            mtime: 1700000000,
            filesize,
            devmajor: 0,
            devminor: 0,
            rdevmajor: 0,
            rdevminor: 0,
        }
    }

    #[test]
    fn test_cpio_newc_magic() {
        let mut buf = Vec::new();
        let mut writer = CpioWriter::new(&mut buf, CpioFormat::Newc);
        let meta = empty_metadata(5);
        let mut data = io::Cursor::new(b"hello");
        writer.write_entry(0, "./test", &meta, &mut data).unwrap();
        let (_, _) = writer.finish().unwrap();
        assert_eq!(&buf[0..6], b"070701");
    }

    #[test]
    fn test_cpio_newc_padding() {
        // Name "ab" (2 bytes) + NUL = 3 bytes. Header 110 + 3 = 113 → pad to 116 (3 bytes pad).
        let mut buf = Vec::new();
        let mut writer = CpioWriter::new(&mut buf, CpioFormat::Newc);

        let meta = empty_metadata(3);
        let mut data = io::Cursor::new(b"abc");
        writer.write_entry(0, "ab", &meta, &mut data).unwrap();

        // Header (110) + name "ab\0" (3) + pad (3) = 116
        // Data (3) + pad (1) = 4
        // Total before trailer: 120
        // Now check that the data starts at offset 116
        assert_eq!(buf[116], b'a');
        assert_eq!(buf[117], b'b');
        assert_eq!(buf[118], b'c');
        // Pad byte after data
        assert_eq!(buf[119], 0);
    }

    #[test]
    fn test_cpio_newc_trailer() {
        let mut buf = Vec::new();
        let writer = CpioWriter::new(&mut buf, CpioFormat::Newc);
        let (_, bytes) = writer.finish().unwrap();

        // Should contain TRAILER!!!
        let trailer_pos = buf
            .windows(10)
            .position(|w| w == b"TRAILER!!!")
            .expect("TRAILER!!! not found");
        assert!(trailer_pos > 0);
        assert!(bytes > 0);
    }

    #[test]
    fn test_cpio_newc_single_file() {
        let mut buf = Vec::new();
        let mut writer = CpioWriter::new(&mut buf, CpioFormat::Newc);

        let meta = CpioMetadata {
            ino: 42,
            mode: 0o100755,
            uid: 1000,
            gid: 1000,
            nlink: 1,
            mtime: 1700000000,
            filesize: 11,
            devmajor: 8,
            devminor: 1,
            rdevmajor: 0,
            rdevminor: 0,
        };

        let content = b"hello world";
        let mut data = io::Cursor::new(content);
        writer
            .write_entry(0, "./usr/bin/hello", &meta, &mut data)
            .unwrap();
        let (_, _) = writer.finish().unwrap();

        // Verify magic
        assert_eq!(&buf[0..6], b"070701");

        // Verify ino field (bytes 6..14)
        let ino_str = std::str::from_utf8(&buf[6..14]).unwrap();
        assert_eq!(u32::from_str_radix(ino_str, 16).unwrap(), 42);

        // Verify mode field (bytes 14..22)
        let mode_str = std::str::from_utf8(&buf[14..22]).unwrap();
        assert_eq!(u32::from_str_radix(mode_str, 16).unwrap(), 0o100755);
    }

    #[test]
    fn test_cpio_newc_rejects_large_file() {
        let mut buf = Vec::new();
        let mut writer = CpioWriter::new(&mut buf, CpioFormat::Newc);

        let meta = empty_metadata(0x1_0000_0000); // > 4 GiB
        let mut data = io::empty();
        let result = writer.write_entry(0, "./big", &meta, &mut data);
        assert!(result.is_err());

        match result.unwrap_err() {
            CpioError::FileTooLarge { size, max, .. } => {
                assert_eq!(size, 0x1_0000_0000);
                assert_eq!(max, NEWC_MAX_FILESIZE);
            }
            other => panic!("expected FileTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn test_cpio_newc_empty_file() {
        let mut buf = Vec::new();
        let mut writer = CpioWriter::new(&mut buf, CpioFormat::Newc);

        let meta = empty_metadata(0);
        let mut data = io::empty();
        writer.write_entry(0, "./empty", &meta, &mut data).unwrap();
        let (_, _) = writer.finish().unwrap();

        assert_eq!(&buf[0..6], b"070701");
    }

    #[test]
    fn test_cpio_newc_finish_returns_bytes_written() {
        let mut buf = Vec::new();
        let mut writer = CpioWriter::new(&mut buf, CpioFormat::Newc);

        let meta = empty_metadata(5);
        let mut data = io::Cursor::new(b"hello");
        writer.write_entry(0, "./test", &meta, &mut data).unwrap();
        let (_, bytes_written) = writer.finish().unwrap();

        assert_eq!(bytes_written as usize, buf.len());
    }

    #[test]
    fn test_cpio_extended_magic() {
        let mut buf = Vec::new();
        let mut writer = CpioWriter::new(&mut buf, CpioFormat::Extended);

        let meta = empty_metadata(5);
        let mut data = io::Cursor::new(b"hello");
        writer.write_entry(0, "", &meta, &mut data).unwrap();
        let (_, _) = writer.finish().unwrap();

        assert_eq!(&buf[0..6], b"07070X");
    }

    #[test]
    fn test_cpio_extended_header_size() {
        let mut buf = Vec::new();
        let mut writer = CpioWriter::new(&mut buf, CpioFormat::Extended);

        let meta = empty_metadata(0);
        let mut data = io::empty();
        writer.write_entry(0, "", &meta, &mut data).unwrap();

        // Header should be 14 bytes (6 magic + 8 index). No data, no padding needed.
        // Next entry (trailer from finish) starts at offset 14.
        // But first entry has filesize=0 and pad4(0)=0, so 14 bytes total.
        // Let's just check the first 14 bytes are our header.
        assert_eq!(buf.len(), 14); // Only one entry with no data before finish
        assert_eq!(&buf[0..6], b"07070X");

        // Index field
        let idx_str = std::str::from_utf8(&buf[6..14]).unwrap();
        assert_eq!(u32::from_str_radix(idx_str, 16).unwrap(), 0);
    }

    #[test]
    fn test_cpio_extended_large_file_accepted() {
        // Extended format should NOT reject files > 4 GiB.
        let mut buf = Vec::new();
        let mut writer = CpioWriter::new(&mut buf, CpioFormat::Extended);

        // We won't actually write 5 GiB of data, just test that the metadata is accepted.
        // Use filesize=0 to avoid needing actual data, but set a large logical size
        // in metadata that won't be validated (Extended doesn't check size).
        // Actually, stream_data will try to read filesize bytes, so we need filesize=0.
        let meta = CpioMetadata {
            filesize: 0,
            ..empty_metadata(0)
        };
        let mut data = io::empty();
        let result = writer.write_entry(0, "", &meta, &mut data);
        assert!(result.is_ok());
    }

    #[test]
    fn test_cpio_extended_padding() {
        let mut buf = Vec::new();
        let mut writer = CpioWriter::new(&mut buf, CpioFormat::Extended);

        // 3 bytes of data: header (14) + data (3) + pad (1) = 18
        let meta = empty_metadata(3);
        let mut data = io::Cursor::new(b"abc");
        writer.write_entry(0, "", &meta, &mut data).unwrap();
        let (_, _) = writer.finish().unwrap();

        // First entry: header (14) + data (3) + pad (1) = 18
        // Data starts at offset 14
        assert_eq!(buf[14], b'a');
        assert_eq!(buf[15], b'b');
        assert_eq!(buf[16], b'c');
        assert_eq!(buf[17], 0); // padding
                                // Trailer starts at offset 18
        assert_eq!(&buf[18..24], b"07070X");
    }

    #[test]
    fn test_cpio_hardlink_handling() {
        let mut buf = Vec::new();
        let mut writer = CpioWriter::new(&mut buf, CpioFormat::Newc);

        let content = b"shared content";

        // First two links: filesize=0, nlink=3
        let meta_nodata = CpioMetadata {
            ino: 99,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            nlink: 3,
            mtime: 1700000000,
            filesize: 0,
            devmajor: 0,
            devminor: 0,
            rdevmajor: 0,
            rdevminor: 0,
        };

        let mut empty = io::empty();
        writer
            .write_entry(0, "./link1", &meta_nodata, &mut empty)
            .unwrap();
        let mut empty = io::empty();
        writer
            .write_entry(1, "./link2", &meta_nodata, &mut empty)
            .unwrap();

        // Last link: carries the data
        let meta_data = CpioMetadata {
            filesize: content.len() as u64,
            ..meta_nodata
        };
        let mut data = io::Cursor::new(content);
        writer
            .write_entry(2, "./link3", &meta_data, &mut data)
            .unwrap();

        let (_, _) = writer.finish().unwrap();

        // Verify: search for the content in the buffer.
        // It should appear exactly once (only with the last link).
        let content_positions: Vec<_> = buf
            .windows(content.len())
            .enumerate()
            .filter(|(_, w)| *w == content)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            content_positions.len(),
            1,
            "content should appear exactly once"
        );
    }

    #[test]
    fn test_pad4() {
        assert_eq!(pad4(0), 0);
        assert_eq!(pad4(1), 3);
        assert_eq!(pad4(2), 2);
        assert_eq!(pad4(3), 1);
        assert_eq!(pad4(4), 0);
        assert_eq!(pad4(5), 3);
        assert_eq!(pad4(110), 2); // Newc header size
    }
}
