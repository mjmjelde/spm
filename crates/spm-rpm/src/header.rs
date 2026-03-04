//! RPM header structure builder.
//!
//! Builds the binary representation of an RPM header section. Used for both
//! the metadata header and the signature header. The binary format is:
//!
//! ```text
//! Magic:       0x8E 0xAD 0xE8 0x01 (4 bytes)
//! Reserved:    0x00 0x00 0x00 0x00 (4 bytes)
//! Index count: u32 big-endian       (4 bytes)
//! Data size:   u32 big-endian       (4 bytes)
//! Index entries: count × 16 bytes each (tag, type, offset, count — all u32 BE)
//! Data section: variable length
//! ```
//!
//! Alignment rules for the data section:
//! - INT16 values: 2-byte aligned
//! - INT32 values: 4-byte aligned
//! - INT64 values: 8-byte aligned
//! - Strings, StringArrays, Bin: byte-aligned (no padding required)

use crate::error::RpmError;
use crate::tags::TagType;

/// RPM header magic bytes.
const HEADER_MAGIC: [u8; 4] = [0x8E, 0xAD, 0xE8, 0x01];

/// A tag value to be written into an RPM header's data section.
#[derive(Debug, Clone)]
pub enum TagValue {
    /// Single NUL-terminated string.
    String(String),
    /// Array of NUL-terminated strings.
    StringArray(Vec<String>),
    /// Internationalized string (written identically to String).
    I18NString(String),
    /// Array of signed 32-bit integers.
    Int32(Vec<i32>),
    /// Array of signed 64-bit integers.
    Int64(Vec<i64>),
    /// Array of signed 16-bit integers.
    Int16(Vec<i16>),
    /// Binary blob (arbitrary bytes).
    Bin(Vec<u8>),
}

impl TagValue {
    /// Return the RPM tag type for this value.
    fn tag_type(&self) -> TagType {
        match self {
            TagValue::String(_) => TagType::String,
            TagValue::StringArray(_) => TagType::StringArray,
            TagValue::I18NString(_) => TagType::I18NString,
            TagValue::Int32(_) => TagType::Int32,
            TagValue::Int64(_) => TagType::Int64,
            TagValue::Int16(_) => TagType::Int16,
            TagValue::Bin(_) => TagType::Bin,
        }
    }

    /// Return the count for the index entry.
    fn count(&self) -> u32 {
        match self {
            TagValue::String(_) | TagValue::I18NString(_) => 1,
            TagValue::StringArray(v) => v.len() as u32,
            TagValue::Int32(v) => v.len() as u32,
            TagValue::Int64(v) => v.len() as u32,
            TagValue::Int16(v) => v.len() as u32,
            TagValue::Bin(v) => v.len() as u32,
        }
    }

    /// Return the required alignment for this value type in the data section.
    fn alignment(&self) -> usize {
        match self {
            TagValue::Int16(_) => 2,
            TagValue::Int32(_) => 4,
            TagValue::Int64(_) => 8,
            _ => 1,
        }
    }

    /// Write this value into a data buffer, returning bytes written.
    fn write_to(&self, data: &mut Vec<u8>) {
        match self {
            TagValue::String(s) | TagValue::I18NString(s) => {
                data.extend_from_slice(s.as_bytes());
                data.push(0); // NUL terminator
            }
            TagValue::StringArray(strings) => {
                for s in strings {
                    data.extend_from_slice(s.as_bytes());
                    data.push(0); // NUL terminator
                }
            }
            TagValue::Int16(values) => {
                for &v in values {
                    data.extend_from_slice(&v.to_be_bytes());
                }
            }
            TagValue::Int32(values) => {
                for &v in values {
                    data.extend_from_slice(&v.to_be_bytes());
                }
            }
            TagValue::Int64(values) => {
                for &v in values {
                    data.extend_from_slice(&v.to_be_bytes());
                }
            }
            TagValue::Bin(bytes) => {
                data.extend_from_slice(bytes);
            }
        }
    }
}

/// Builds an RPM header section (used for both signature and metadata headers).
///
/// Tags are added in any order; they are sorted by tag number during
/// serialization as required by the RPM format.
pub struct HeaderBuilder {
    entries: Vec<(u32, TagValue)>,
}

impl HeaderBuilder {
    /// Create a new empty header builder.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Push an entry, asserting no duplicate tags in debug builds.
    fn push_entry(&mut self, tag: u32, value: TagValue) {
        debug_assert!(
            !self.entries.iter().any(|(t, _)| *t == tag),
            "duplicate RPM header tag {tag}"
        );
        self.entries.push((tag, value));
    }

    /// Add a single string tag.
    pub fn add_string(&mut self, tag: u32, value: &str) -> &mut Self {
        self.push_entry(tag, TagValue::String(value.to_owned()));
        self
    }

    /// Add a string array tag.
    pub fn add_string_array(&mut self, tag: u32, values: Vec<String>) -> &mut Self {
        self.push_entry(tag, TagValue::StringArray(values));
        self
    }

    /// Add an internationalized string tag.
    pub fn add_i18n_string(&mut self, tag: u32, value: &str) -> &mut Self {
        self.push_entry(tag, TagValue::I18NString(value.to_owned()));
        self
    }

    /// Add an INT32 array tag.
    pub fn add_int32(&mut self, tag: u32, values: Vec<i32>) -> &mut Self {
        self.push_entry(tag, TagValue::Int32(values));
        self
    }

    /// Add an INT64 array tag.
    pub fn add_int64(&mut self, tag: u32, values: Vec<i64>) -> &mut Self {
        self.push_entry(tag, TagValue::Int64(values));
        self
    }

    /// Add an INT16 array tag.
    pub fn add_int16(&mut self, tag: u32, values: Vec<i16>) -> &mut Self {
        self.push_entry(tag, TagValue::Int16(values));
        self
    }

    /// Add a binary blob tag.
    pub fn add_bin(&mut self, tag: u32, data: Vec<u8>) -> &mut Self {
        self.push_entry(tag, TagValue::Bin(data));
        self
    }

    /// Add a region tag (62 for signature, 63 for metadata).
    ///
    /// The region tag has type BIN with 16 bytes of trailer data that
    /// encodes a pseudo-index-entry pointing back at the region's entries.
    /// Must be called AFTER all other entries have been added, because the
    /// trailer's negative offset encodes the total entry count.
    ///
    /// The trailer data (16 bytes) is:
    /// - tag (u32 BE): the region tag number
    /// - type (u32 BE): BIN (7)
    /// - offset (i32 BE): -(total_entry_count * 16)
    /// - count (u32 BE): 16
    pub fn add_region_tag(&mut self, tag: u32) -> &mut Self {
        // Total entries INCLUDING this region tag.
        let total = (self.entries.len() + 1) as i32;
        let neg_offset = -(total * 16);

        let mut trailer = Vec::with_capacity(16);
        trailer.extend_from_slice(&tag.to_be_bytes());
        trailer.extend_from_slice(&7u32.to_be_bytes()); // BIN type
        trailer.extend_from_slice(&neg_offset.to_be_bytes());
        trailer.extend_from_slice(&16u32.to_be_bytes());

        self.push_entry(tag, TagValue::Bin(trailer));
        self
    }

    /// Return the number of entries added so far.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Serialize the header to its binary representation.
    ///
    /// Returns the complete header: magic (4) + reserved (4) + index count (4) +
    /// data size (4) + index entries (16 each) + data section.
    ///
    /// RPM requires index entries to be sorted in ascending tag order.
    /// Data offsets must be monotonically increasing for regular tags.
    /// Region tags (62, 63) are special: their data goes at the end of
    /// the data section, but their index entries sort by natural tag number
    /// (first, since 62/63 are the lowest). RPM skips offset monotonicity
    /// checks for region tag entries.
    pub fn build(&self) -> Result<Vec<u8>, RpmError> {
        if self.entries.is_empty() {
            return Err(RpmError::Header("header has no entries".into()));
        }

        // Region tags (62 = HEADERSIGNATURES, 63 = HEADERIMMUTABLE) have their
        // data placed at the END of the data section, regardless of tag order.
        const REGION_TAGS: [u32; 2] = [62, 63];

        // Sort entries for DATA layout: regular tags by tag number first,
        // region tags at the end (their data must be last in the data section).
        let mut data_sorted: Vec<&(u32, TagValue)> = self.entries.iter().collect();
        data_sorted.sort_by_key(|(tag, _)| {
            if REGION_TAGS.contains(tag) {
                (1, *tag) // region tags' data goes after all regular tags
            } else {
                (0, *tag)
            }
        });

        // Build the data section in data-layout order.
        let mut data = Vec::new();
        let mut entry_offsets: Vec<(u32, TagType, i32, u32)> =
            Vec::with_capacity(self.entries.len());

        for (tag, value) in &data_sorted {
            // Insert alignment padding.
            let align = value.alignment();
            let pad = (align - (data.len() % align)) % align;
            data.extend(std::iter::repeat_n(0u8, pad));

            if data.len() > i32::MAX as usize {
                return Err(RpmError::InvalidRpm(format!(
                    "header data section too large ({} bytes, max {})",
                    data.len(),
                    i32::MAX
                )));
            }
            let offset = data.len() as i32;
            let count = value.count();
            let tag_type = value.tag_type();

            value.write_to(&mut data);

            entry_offsets.push((*tag, tag_type, offset, count));
        }

        // Build index entries sorted by NATURAL tag number (ascending).
        // Region tags (62, 63) have the lowest numbers so they sort first.
        // RPM's hdrblobVerifyInfo() requires strict ascending tag order in
        // the index but skips offset monotonicity checks for region tags.
        let mut index_entries: Vec<IndexEntry> = entry_offsets
            .iter()
            .map(|&(tag, tag_type, offset, count)| IndexEntry {
                tag,
                tag_type,
                offset,
                count,
            })
            .collect();
        index_entries.sort_by_key(|e| e.tag);

        // Assemble the full header.
        let index_count = index_entries.len() as u32;
        let data_size = data.len() as u32;

        let total_size = 16 + (index_entries.len() * 16) + data.len();
        let mut out = Vec::with_capacity(total_size);

        // Magic (4 bytes)
        out.extend_from_slice(&HEADER_MAGIC);

        // Reserved (4 bytes)
        out.extend_from_slice(&[0u8; 4]);

        // Index count (4 bytes, big-endian)
        out.extend_from_slice(&index_count.to_be_bytes());

        // Data size (4 bytes, big-endian)
        out.extend_from_slice(&data_size.to_be_bytes());

        // Index entries (16 bytes each)
        for entry in &index_entries {
            out.extend_from_slice(&entry.tag.to_be_bytes());
            out.extend_from_slice(&(entry.tag_type as u32).to_be_bytes());
            out.extend_from_slice(&entry.offset.to_be_bytes());
            out.extend_from_slice(&entry.count.to_be_bytes());
        }

        // Data section
        out.extend_from_slice(&data);

        Ok(out)
    }
}

impl Default for HeaderBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Internal representation of a header index entry.
struct IndexEntry {
    tag: u32,
    tag_type: TagType,
    offset: i32,
    count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_magic() {
        let mut hdr = HeaderBuilder::new();
        hdr.add_string(1000, "testpkg");
        let bytes = hdr.build().unwrap();
        assert_eq!(&bytes[0..4], &[0x8E, 0xAD, 0xE8, 0x01]);
    }

    #[test]
    fn test_header_reserved_zeros() {
        let mut hdr = HeaderBuilder::new();
        hdr.add_string(1000, "testpkg");
        let bytes = hdr.build().unwrap();
        assert_eq!(&bytes[4..8], &[0, 0, 0, 0]);
    }

    #[test]
    fn test_header_index_count() {
        let mut hdr = HeaderBuilder::new();
        hdr.add_string(1000, "name");
        hdr.add_string(1001, "version");
        let bytes = hdr.build().unwrap();
        let count = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
        assert_eq!(count, 2);
    }

    #[test]
    fn test_header_byte_order() {
        let mut hdr = HeaderBuilder::new();
        hdr.add_int32(1009, vec![12345]);
        let bytes = hdr.build().unwrap();
        // First index entry starts at offset 16. Tag field is bytes 16..20.
        let tag = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        assert_eq!(tag, 1009);
    }

    #[test]
    fn test_header_entries_sorted_by_tag() {
        let mut hdr = HeaderBuilder::new();
        // Add in reverse order.
        hdr.add_string(1005, "desc");
        hdr.add_string(1000, "name");
        let bytes = hdr.build().unwrap();
        // First index entry tag (offset 16..20)
        let tag1 = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        // Second index entry tag (offset 32..36)
        let tag2 = u32::from_be_bytes(bytes[32..36].try_into().unwrap());
        assert_eq!(tag1, 1000);
        assert_eq!(tag2, 1005);
    }

    #[test]
    fn test_header_string_nul_terminated() {
        let mut hdr = HeaderBuilder::new();
        hdr.add_string(1000, "test");
        let bytes = hdr.build().unwrap();
        // Data section starts after 16 (preamble) + 16 (one index entry) = 32.
        let data_start = 32;
        assert_eq!(&bytes[data_start..data_start + 4], b"test");
        assert_eq!(bytes[data_start + 4], 0); // NUL
    }

    #[test]
    fn test_header_int32_alignment() {
        let mut hdr = HeaderBuilder::new();
        // String "x" takes 2 bytes (x + NUL), offset 0.
        hdr.add_string(1000, "x");
        // INT32 must be 4-byte aligned, so padding of 2 bytes expected.
        hdr.add_int32(1009, vec![42]);
        let bytes = hdr.build().unwrap();

        // Find the INT32 entry's offset in the index.
        // Two entries: string(1000) at index 0, int32(1009) at index 1.
        // Index entry 1 starts at 16 + 16 = 32. Offset field is at 32+8..32+12.
        let offset = i32::from_be_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(offset % 4, 0, "INT32 offset must be 4-byte aligned");
    }

    #[test]
    fn test_header_int64_alignment() {
        let mut hdr = HeaderBuilder::new();
        hdr.add_string(1000, "x"); // 2 bytes in data
        hdr.add_int64(5009, vec![1_234_567_890i64]);
        let bytes = hdr.build().unwrap();

        // Index entry 1 (int64) offset field: at 16 + 16 + 8 = 40..44
        let offset = i32::from_be_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(offset % 8, 0, "INT64 offset must be 8-byte aligned");
    }

    #[test]
    fn test_header_int16_alignment() {
        let mut hdr = HeaderBuilder::new();
        hdr.add_string(1000, "x"); // 2 bytes in data (already aligned for INT16)
        hdr.add_int16(1030, vec![0o644]);
        let bytes = hdr.build().unwrap();

        let offset = i32::from_be_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(offset % 2, 0, "INT16 offset must be 2-byte aligned");
    }

    #[test]
    fn test_header_string_array() {
        let mut hdr = HeaderBuilder::new();
        hdr.add_string_array(1117, vec!["file1".into(), "file2".into(), "file3".into()]);
        let bytes = hdr.build().unwrap();

        // Check count in index entry (offset 16+12..16+16)
        let count = u32::from_be_bytes(bytes[28..32].try_into().unwrap());
        assert_eq!(count, 3);

        // Data should contain "file1\0file2\0file3\0"
        let data_start = 32;
        let expected = b"file1\0file2\0file3\0";
        assert_eq!(&bytes[data_start..data_start + expected.len()], expected);
    }

    #[test]
    fn test_header_bin() {
        let mut hdr = HeaderBuilder::new();
        let md5 = vec![0xDE, 0xAD, 0xBE, 0xEF];
        hdr.add_bin(1004, md5.clone());
        let bytes = hdr.build().unwrap();

        // Check type in index entry (offset 16+4..16+8)
        let tag_type = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        assert_eq!(tag_type, TagType::Bin as u32);

        // Data section starts at 32.
        assert_eq!(&bytes[32..36], &md5);
    }

    #[test]
    fn test_header_empty_rejected() {
        let hdr = HeaderBuilder::new();
        let result = hdr.build();
        assert!(result.is_err());
    }

    #[test]
    fn test_header_data_size_field() {
        let mut hdr = HeaderBuilder::new();
        hdr.add_string(1000, "hello"); // 6 bytes (hello + NUL)
        let bytes = hdr.build().unwrap();

        let data_size = u32::from_be_bytes(bytes[12..16].try_into().unwrap());
        assert_eq!(data_size, 6);
    }

    #[test]
    fn test_region_tag_index_first_data_last() {
        let mut hdr = HeaderBuilder::new();
        hdr.add_string(1000, "name");
        hdr.add_string(1001, "version");
        hdr.add_region_tag(62);

        let bytes = hdr.build().unwrap();

        let index_count = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
        assert_eq!(index_count, 3);

        // First index entry should be tag 62 (lowest tag number).
        let tag0 = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        assert_eq!(tag0, 62, "region tag must be first in index");

        // Second and third should be 1000, 1001.
        let tag1 = u32::from_be_bytes(bytes[32..36].try_into().unwrap());
        let tag2 = u32::from_be_bytes(bytes[48..52].try_into().unwrap());
        assert_eq!(tag1, 1000);
        assert_eq!(tag2, 1001);

        // Region tag's data offset must be the highest (data is last).
        let offset0 = i32::from_be_bytes(bytes[24..28].try_into().unwrap());
        let offset1 = i32::from_be_bytes(bytes[40..44].try_into().unwrap());
        let offset2 = i32::from_be_bytes(bytes[56..60].try_into().unwrap());
        assert!(
            offset0 > offset1 && offset0 > offset2,
            "region tag data offset ({offset0}) must be greater than regular tag offsets ({offset1}, {offset2})"
        );
    }

    #[test]
    fn test_immutable_region_tag_sorts_first() {
        let mut hdr = HeaderBuilder::new();
        hdr.add_string(1000, "name");
        hdr.add_int32(1009, vec![42]);
        hdr.add_region_tag(63);

        let bytes = hdr.build().unwrap();

        let tag0 = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        assert_eq!(tag0, 63, "immutable region tag must be first in index");
    }
}
