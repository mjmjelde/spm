//! Shared types used across spm crates.

/// Format-specific size limits used by the planner.
#[derive(Debug, Clone)]
pub struct FormatLimits {
    /// Max compressed payload size per package (for auto-split decisions).
    pub max_compressed_payload: u64,
    /// Max individual file size before extended format is needed.
    pub max_file_size_standard: u64,
    /// Name of the format (for display messages).
    pub format_name: &'static str,
}

impl FormatLimits {
    /// RPM format limits.
    pub fn rpm() -> Self {
        Self {
            // RPM doesn't have a practical package size limit (64-bit tags since 4.6)
            max_compressed_payload: u64::MAX,
            // Standard cpio limit: 4 GiB (8 hex digits)
            max_file_size_standard: 0xFFFF_FFFF,
            format_name: "rpm",
        }
    }

    /// DEB format limits.
    pub fn deb() -> Self {
        Self {
            // ar member size: 10 ASCII decimal digits
            max_compressed_payload: 9_999_999_999,
            // GNU tar: effectively unlimited per entry
            max_file_size_standard: u64::MAX,
            format_name: "deb",
        }
    }
}

/// Package output naming for the plan display.
#[derive(Debug, Clone)]
pub struct PackageFileName {
    pub name: String,
    pub version: String,
    pub release: String,
    pub arch: String,
    pub format: String,
}

impl PackageFileName {
    /// Generate the output filename for RPM format.
    pub fn rpm(name: &str, version: &str, release: &str, arch: &str) -> String {
        format!("{name}-{version}-{release}.{arch}.rpm")
    }

    /// Generate the output filename for DEB format.
    /// DEB uses amd64 instead of x86_64, arm64 instead of aarch64.
    pub fn deb(name: &str, version: &str, release: &str, arch: &str) -> String {
        let deb_arch = match arch {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            "i686" => "i386",
            "armv7hl" => "armhf",
            "noarch" | "all" => "all",
            other => other,
        };
        format!("{name}_{version}-{release}_{deb_arch}.deb")
    }
}

/// Parse a human-readable size string into bytes.
///
/// Supports: bare numbers (bytes), B, KiB, MiB, GiB, TiB suffixes.
/// Decimal values like "4.5GiB" are supported.
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty size string".to_string());
    }

    // Find the boundary between the numeric part and the suffix
    let num_end = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());

    let num_str = &s[..num_end];
    let suffix = s[num_end..].trim();

    let num: f64 = num_str
        .parse()
        .map_err(|_| format!("invalid number: '{num_str}'"))?;

    let multiplier: u64 = match suffix {
        "" | "B" => 1,
        "KiB" => 1024,
        "MiB" => 1024 * 1024,
        "GiB" => 1024 * 1024 * 1024,
        "TiB" => 1024u64 * 1024 * 1024 * 1024,
        other => return Err(format!("unknown size suffix: '{other}'")),
    };

    Ok((num * multiplier as f64) as u64)
}

/// Format a byte count as a human-readable size string.
pub fn format_size(bytes: u64) -> String {
    const TIB: u64 = 1024 * 1024 * 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;
    const KIB: u64 = 1024;

    if bytes >= TIB {
        format!("{:.1} TiB", bytes as f64 / TIB as f64)
    } else if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Estimated compression ratio for a given algorithm.
/// Returns a value between 0.0 and 1.0 representing compressed/uncompressed.
pub fn estimated_compression_ratio(algorithm: &str) -> f64 {
    match algorithm {
        "zstd" => 0.35,
        "gzip" => 0.40,
        "xz" => 0.30,
        "none" => 1.0,
        _ => 0.35, // default to zstd-like
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size_bare_number() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("0").unwrap(), 0);
    }

    #[test]
    fn test_parse_size_bytes() {
        assert_eq!(parse_size("512B").unwrap(), 512);
    }

    #[test]
    fn test_parse_size_kib() {
        assert_eq!(parse_size("1KiB").unwrap(), 1024);
        assert_eq!(parse_size("10KiB").unwrap(), 10240);
    }

    #[test]
    fn test_parse_size_mib() {
        assert_eq!(parse_size("100MiB").unwrap(), 100 * 1024 * 1024);
    }

    #[test]
    fn test_parse_size_gib() {
        assert_eq!(parse_size("8GiB").unwrap(), 8 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_parse_size_tib() {
        assert_eq!(parse_size("1TiB").unwrap(), 1024u64 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_parse_size_decimal() {
        let expected = (4.5 * 1024.0 * 1024.0 * 1024.0) as u64;
        assert_eq!(parse_size("4.5GiB").unwrap(), expected);
    }

    #[test]
    fn test_parse_size_invalid_suffix() {
        assert!(parse_size("8ZB").is_err());
    }

    #[test]
    fn test_parse_size_empty() {
        assert!(parse_size("").is_err());
    }

    #[test]
    fn test_parse_size_invalid_number() {
        assert!(parse_size("abcGiB").is_err());
    }

    #[test]
    fn test_format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn test_format_size_kib() {
        assert_eq!(format_size(1024), "1.0 KiB");
        assert_eq!(format_size(1536), "1.5 KiB");
    }

    #[test]
    fn test_format_size_mib() {
        assert_eq!(format_size(100 * 1024 * 1024), "100.0 MiB");
    }

    #[test]
    fn test_format_size_gib() {
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GiB");
    }

    #[test]
    fn test_format_size_tib() {
        assert_eq!(format_size(1024u64 * 1024 * 1024 * 1024), "1.0 TiB");
    }

    #[test]
    fn test_format_limits_rpm() {
        let limits = FormatLimits::rpm();
        assert_eq!(limits.max_compressed_payload, u64::MAX);
        assert_eq!(limits.max_file_size_standard, 0xFFFF_FFFF);
        assert_eq!(limits.format_name, "rpm");
    }

    #[test]
    fn test_format_limits_deb() {
        let limits = FormatLimits::deb();
        assert_eq!(limits.max_compressed_payload, 9_999_999_999);
        assert_eq!(limits.max_file_size_standard, u64::MAX);
        assert_eq!(limits.format_name, "deb");
    }

    #[test]
    fn test_rpm_filename() {
        assert_eq!(
            PackageFileName::rpm("matlab-2025a", "2025a", "1", "x86_64"),
            "matlab-2025a-2025a-1.x86_64.rpm"
        );
    }

    #[test]
    fn test_deb_filename_arch_translation() {
        assert_eq!(
            PackageFileName::deb("matlab-2025a", "2025a", "1", "x86_64"),
            "matlab-2025a_2025a-1_amd64.deb"
        );
        assert_eq!(
            PackageFileName::deb("pkg", "1.0", "1", "aarch64"),
            "pkg_1.0-1_arm64.deb"
        );
        assert_eq!(
            PackageFileName::deb("pkg", "1.0", "1", "noarch"),
            "pkg_1.0-1_all.deb"
        );
    }
}
