//! RPM signature header builder.
//!
//! The signature header uses the same binary format as the metadata header
//! but contains digest and size tags that allow `rpm -K` to verify package
//! integrity without GPG signatures.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use md5::{Digest as Md5Digest, Md5};
use sha1::Sha1;
use sha2::Sha256;

use crate::error::RpmError;
use crate::header::HeaderBuilder;
use crate::tags::*;

/// Build the RPM signature header.
///
/// # Arguments
/// - `header_bytes`: the serialized metadata header.
/// - `payload_path`: path to the compressed payload temp file.
/// - `uncompressed_payload_size`: total uncompressed cpio archive size.
///
/// Returns the serialized signature header bytes.
pub fn build_signature(
    header_bytes: &[u8],
    payload_path: &Path,
    uncompressed_payload_size: u64,
) -> Result<Vec<u8>, RpmError> {
    let payload_file_size = std::fs::metadata(payload_path)?.len();
    let header_plus_payload = header_bytes.len() as u64 + payload_file_size;

    // Compute MD5 of header + payload.
    let md5_digest = compute_md5(header_bytes, payload_path)?;

    // Compute SHA-1 of header only (hex string).
    let sha1_hex = compute_sha1_hex(header_bytes);

    // Compute SHA-256 of header only (hex string).
    let sha256_hex = compute_sha256_hex(header_bytes);

    let mut hdr = HeaderBuilder::new();

    // SHA-1 hex digest of header only.
    hdr.add_string(RPMSIGTAG_SHA1, &sha1_hex);

    // SHA-256 hex digest of header only.
    hdr.add_string(RPMSIGTAG_SHA256, &sha256_hex);

    // Size tags: use both 32-bit and 64-bit when value fits.
    if header_plus_payload <= i32::MAX as u64 {
        hdr.add_int32(RPMSIGTAG_SIZE, vec![header_plus_payload as i32]);
    }
    hdr.add_int64(RPMSIGTAG_LONGSIZE, vec![header_plus_payload as i64]);

    // MD5 digest of header + payload.
    hdr.add_bin(RPMSIGTAG_MD5, md5_digest);

    // Uncompressed payload size tags.
    if uncompressed_payload_size <= i32::MAX as u64 {
        hdr.add_int32(
            RPMSIGTAG_PAYLOADSIZE,
            vec![uncompressed_payload_size as i32],
        );
    }
    hdr.add_int64(
        RPMSIGTAG_LONGARCHIVESIZE,
        vec![uncompressed_payload_size as i64],
    );

    // Region tag (must be added last — its data goes at end of data section).
    hdr.add_region_tag(RPMTAG_HEADERSIGNATURES);

    hdr.build()
}

/// Compute MD5 digest of header bytes followed by payload file bytes.
fn compute_md5(header_bytes: &[u8], payload_path: &Path) -> Result<Vec<u8>, RpmError> {
    let mut hasher = Md5::new();
    hasher.update(header_bytes);

    let mut file = File::open(payload_path)?;
    let mut buf = [0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(hasher.finalize().to_vec())
}

/// Compute SHA-1 hex digest of the given bytes.
fn compute_sha1_hex(data: &[u8]) -> String {
    use sha1::Digest;
    let mut hasher = Sha1::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Compute SHA-256 hex digest of the given bytes.
fn compute_sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_signature_sha256_is_hex() {
        let sha = compute_sha256_hex(b"test data");
        assert_eq!(sha.len(), 64);
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_signature_md5_length() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"payload data").unwrap();

        let md5 = compute_md5(b"header data", tmp.path()).unwrap();
        assert_eq!(md5.len(), 16); // MD5 is 128 bits = 16 bytes
    }

    #[test]
    fn test_build_signature_produces_valid_header() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"compressed payload").unwrap();

        let header_bytes = b"fake header bytes";
        let sig = build_signature(header_bytes, tmp.path(), 1024).unwrap();

        // Should start with header magic.
        assert_eq!(&sig[0..4], &[0x8E, 0xAD, 0xE8, 0x01]);
    }
}
