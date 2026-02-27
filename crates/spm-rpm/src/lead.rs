//! RPM Lead (96-byte fixed header) writer.
//!
//! The lead is a legacy structure at the very beginning of every RPM file.
//! Modern RPM tools mostly ignore it (metadata comes from the Header section),
//! but it must be present and structurally valid.

use std::io::Write;

use crate::error::RpmError;

/// RPM lead magic number.
const RPM_MAGIC: [u8; 4] = [0xED, 0xAB, 0xEE, 0xDB];

/// Lead size in bytes.
const LEAD_SIZE: usize = 96;

/// Maximum length of the name field in the lead (NUL-terminated).
const MAX_NAME_LEN: usize = 65;

/// Write the RPM lead (96 bytes) to the output.
///
/// # Arguments
/// - `writer`: destination for the lead bytes.
/// - `name`: package name string (truncated to 65 chars, NUL-padded to 66).
/// - `arch`: target architecture string (mapped to RPM arch number internally).
pub fn write_lead<W: Write>(writer: &mut W, name: &str, arch: &str) -> Result<(), RpmError> {
    let mut lead = [0u8; LEAD_SIZE];

    // Bytes 0-3: magic
    lead[0..4].copy_from_slice(&RPM_MAGIC);

    // Bytes 4-5: RPM format version (3.0)
    lead[4] = 3;
    lead[5] = 0;

    // Bytes 6-7: type (0 = binary package)
    lead[6..8].copy_from_slice(&0u16.to_be_bytes());

    // Bytes 8-9: architecture number
    let arch_num = arch_to_num(arch);
    lead[8..10].copy_from_slice(&arch_num.to_be_bytes());

    // Bytes 10-75: name (NUL-padded, max 65 chars + NUL)
    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(MAX_NAME_LEN);
    lead[10..10 + copy_len].copy_from_slice(&name_bytes[..copy_len]);
    // Remaining bytes 10+copy_len..76 are already zero (NUL padding)

    // Bytes 76-77: OS number (1 = Linux)
    lead[76..78].copy_from_slice(&1u16.to_be_bytes());

    // Bytes 78-79: signature type (5 = header-style signatures)
    lead[78..80].copy_from_slice(&5u16.to_be_bytes());

    // Bytes 80-95: reserved (already zero)

    writer.write_all(&lead)?;
    Ok(())
}

/// Map an architecture string to the RPM architecture number.
pub fn arch_to_num(arch: &str) -> u16 {
    match arch {
        "x86_64" | "i686" | "i386" | "i486" | "i586" => 1,
        "aarch64" => 12,
        "armv7hl" | "armv7l" => 12,
        "ppc64le" => 16,
        "s390x" => 15,
        "noarch" => 0,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lead_magic() {
        let mut buf = Vec::new();
        write_lead(&mut buf, "testpkg-1.0-1", "x86_64").unwrap();
        assert_eq!(buf.len(), 96);
        assert_eq!(&buf[0..4], &[0xED, 0xAB, 0xEE, 0xDB]);
    }

    #[test]
    fn test_lead_version() {
        let mut buf = Vec::new();
        write_lead(&mut buf, "testpkg", "x86_64").unwrap();
        assert_eq!(buf[4], 3); // major
        assert_eq!(buf[5], 0); // minor
    }

    #[test]
    fn test_lead_type_binary() {
        let mut buf = Vec::new();
        write_lead(&mut buf, "testpkg", "x86_64").unwrap();
        assert_eq!(u16::from_be_bytes([buf[6], buf[7]]), 0);
    }

    #[test]
    fn test_lead_arch() {
        let mut buf = Vec::new();
        write_lead(&mut buf, "testpkg", "x86_64").unwrap();
        let arch = u16::from_be_bytes([buf[8], buf[9]]);
        assert_eq!(arch, 1);
    }

    #[test]
    fn test_lead_name_embedded() {
        let mut buf = Vec::new();
        write_lead(&mut buf, "testpkg-1.0-1", "x86_64").unwrap();
        let name = &buf[10..23]; // "testpkg-1.0-1" is 13 bytes
        assert_eq!(name, b"testpkg-1.0-1");
        assert_eq!(buf[23], 0); // NUL terminated
    }

    #[test]
    fn test_lead_name_truncated() {
        let long_name = "a".repeat(100);
        let mut buf = Vec::new();
        write_lead(&mut buf, &long_name, "x86_64").unwrap();
        // Only first 65 chars should be written
        assert_eq!(&buf[10..75], "a".repeat(65).as_bytes());
        assert_eq!(buf[75], 0); // byte 76 (index 75) is part of name field, zeroed
    }

    #[test]
    fn test_lead_os_linux() {
        let mut buf = Vec::new();
        write_lead(&mut buf, "testpkg", "x86_64").unwrap();
        let os = u16::from_be_bytes([buf[76], buf[77]]);
        assert_eq!(os, 1);
    }

    #[test]
    fn test_lead_sigtype() {
        let mut buf = Vec::new();
        write_lead(&mut buf, "testpkg", "x86_64").unwrap();
        let sigtype = u16::from_be_bytes([buf[78], buf[79]]);
        assert_eq!(sigtype, 5);
    }

    #[test]
    fn test_arch_to_num() {
        assert_eq!(arch_to_num("x86_64"), 1);
        assert_eq!(arch_to_num("aarch64"), 12);
        assert_eq!(arch_to_num("noarch"), 0);
        assert_eq!(arch_to_num("unknown"), 0);
    }
}
