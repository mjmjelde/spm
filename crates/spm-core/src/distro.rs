//! Target distribution compatibility database.
//!
//! Provides a compile-time database of known Linux distributions and their
//! packaging capabilities, used for compatibility warnings and auto-dependency
//! injection.

/// A known target distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Distro {
    El8,
    El9,
    Ubuntu2004,
    Ubuntu2204,
    Ubuntu2404,
    Fedora,
}

/// Capabilities of an RPM-based distribution.
#[derive(Debug, Clone)]
pub struct RpmDistroInfo {
    pub name: &'static str,
    pub rpm_version: &'static str,
    pub supports_zstd: bool,
    pub supports_large_files: bool,
    /// Package name for the alternatives tool dependency.
    pub alternatives_dep: &'static str,
}

/// Capabilities of a DEB-based distribution.
#[derive(Debug, Clone)]
pub struct DebDistroInfo {
    pub name: &'static str,
    pub dpkg_version: &'static str,
    pub supports_zstd: bool,
}

/// Distribution info classified by package format family.
pub enum DistroInfo {
    Rpm(RpmDistroInfo),
    Deb(DebDistroInfo),
}

impl Distro {
    /// Parse a distro identifier from a CLI string.
    ///
    /// Accepts: `"el8"`, `"el9"`, `"ubuntu2004"`, `"ubuntu2204"`, `"ubuntu2404"`, `"fedora"`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "el8" | "rhel8" => Some(Self::El8),
            "el9" | "rhel9" => Some(Self::El9),
            "ubuntu2004" => Some(Self::Ubuntu2004),
            "ubuntu2204" => Some(Self::Ubuntu2204),
            "ubuntu2404" => Some(Self::Ubuntu2404),
            "fedora" => Some(Self::Fedora),
            _ => None,
        }
    }

    /// Return the capability info for this distribution.
    pub fn info(&self) -> DistroInfo {
        match self {
            Self::El8 => DistroInfo::Rpm(RpmDistroInfo {
                name: "RHEL 8 / CentOS Stream 8",
                rpm_version: "4.14.3",
                supports_zstd: true,
                supports_large_files: true,
                alternatives_dep: "chkconfig",
            }),
            Self::El9 => DistroInfo::Rpm(RpmDistroInfo {
                name: "RHEL 9 / CentOS Stream 9",
                rpm_version: "4.16.1",
                supports_zstd: true,
                supports_large_files: true,
                alternatives_dep: "alternatives",
            }),
            Self::Fedora => DistroInfo::Rpm(RpmDistroInfo {
                name: "Fedora",
                rpm_version: "4.19",
                supports_zstd: true,
                supports_large_files: true,
                alternatives_dep: "alternatives",
            }),
            Self::Ubuntu2004 => DistroInfo::Deb(DebDistroInfo {
                name: "Ubuntu 20.04 LTS",
                dpkg_version: "1.19.7",
                supports_zstd: true,
            }),
            Self::Ubuntu2204 => DistroInfo::Deb(DebDistroInfo {
                name: "Ubuntu 22.04 LTS",
                dpkg_version: "1.21.1",
                supports_zstd: true,
            }),
            Self::Ubuntu2404 => DistroInfo::Deb(DebDistroInfo {
                name: "Ubuntu 24.04 LTS",
                dpkg_version: "1.22.6",
                supports_zstd: true,
            }),
        }
    }
}

/// Check compatibility between a configuration and a target distro.
///
/// Returns a list of warning messages. An empty list means fully compatible.
pub fn check_compatibility(
    distro: &Distro,
    compression_algo: &str,
    has_large_files: bool,
    format: &str,
) -> Vec<String> {
    let mut warnings = Vec::new();

    match distro.info() {
        DistroInfo::Rpm(info) => {
            if format == "deb" {
                warnings.push(format!(
                    "target distro '{}' is RPM-based but format is 'deb'",
                    info.name
                ));
            }
            if compression_algo == "zstd" && !info.supports_zstd {
                warnings.push(format!(
                    "zstd compression not supported on {} (rpm {}); use gzip or xz",
                    info.name, info.rpm_version
                ));
            }
            if has_large_files && !info.supports_large_files {
                warnings.push(format!(
                    "files > 4 GiB not supported on {} (rpm {}); requires rpm >= 4.12",
                    info.name, info.rpm_version
                ));
            }
        }
        DistroInfo::Deb(info) => {
            if format == "rpm" {
                warnings.push(format!(
                    "target distro '{}' is DEB-based but format is 'rpm'",
                    info.name
                ));
            }
            if compression_algo == "zstd" && !info.supports_zstd {
                warnings.push(format!(
                    "zstd compression not supported on {} (dpkg {})",
                    info.name, info.dpkg_version
                ));
            }
        }
    }

    warnings
}

/// Return the minimum RPM version required for the given configuration.
///
/// Returns `(version, reason)` for display in `spm plan` output.
pub fn minimum_rpm_version(
    algo: &str,
    has_large_files: bool,
    needs_extended_cpio: bool,
) -> (&'static str, &'static str) {
    // The highest minimum wins.
    // zstd requires rpm >= 4.14.0
    // xz requires rpm >= 4.7.0
    // large files (>4GiB) require rpm >= 4.12
    // extended cpio requires rpm >= 4.12
    // gzip/none: rpm >= 4.6.0 (baseline)
    match (algo, has_large_files || needs_extended_cpio) {
        ("zstd", true) => ("4.14.0", "zstd compression + large files"),
        ("zstd", false) => ("4.14.0", "zstd compression"),
        ("xz", true) => ("4.12.0", "xz compression + large files"),
        ("xz", false) => ("4.7.0", "xz compression"),
        (_, true) => ("4.12.0", "large files"),
        _ => ("4.6.0", "baseline"),
    }
}

/// Return the minimum dpkg version required for the given compression algorithm.
///
/// Returns `(version, reason)` for display in `spm plan` output.
pub fn minimum_dpkg_version(algo: &str) -> (&'static str, &'static str) {
    match algo {
        "zstd" => ("1.21.18", "zstd compression"),
        "xz" => ("1.15.0", "xz compression"),
        _ => ("1.0.0", "baseline"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distro_from_str_known() {
        assert_eq!(Distro::from_str("el8"), Some(Distro::El8));
        assert_eq!(Distro::from_str("rhel8"), Some(Distro::El8));
        assert_eq!(Distro::from_str("el9"), Some(Distro::El9));
        assert_eq!(Distro::from_str("rhel9"), Some(Distro::El9));
        assert_eq!(Distro::from_str("ubuntu2004"), Some(Distro::Ubuntu2004));
        assert_eq!(Distro::from_str("ubuntu2204"), Some(Distro::Ubuntu2204));
        assert_eq!(Distro::from_str("ubuntu2404"), Some(Distro::Ubuntu2404));
        assert_eq!(Distro::from_str("fedora"), Some(Distro::Fedora));
    }

    #[test]
    fn test_distro_from_str_unknown() {
        assert_eq!(Distro::from_str("archlinux"), None);
        assert_eq!(Distro::from_str(""), None);
        assert_eq!(Distro::from_str("EL8"), None);
    }

    #[test]
    fn test_el8_info() {
        let info = Distro::El8.info();
        match info {
            DistroInfo::Rpm(rpm) => {
                assert_eq!(rpm.rpm_version, "4.14.3");
                assert!(rpm.supports_zstd);
                assert!(rpm.supports_large_files);
                assert_eq!(rpm.alternatives_dep, "chkconfig");
            }
            _ => unreachable!("expected RPM info for el8"),
        }
    }

    #[test]
    fn test_el9_info() {
        let info = Distro::El9.info();
        match info {
            DistroInfo::Rpm(rpm) => {
                assert_eq!(rpm.rpm_version, "4.16.1");
                assert_eq!(rpm.alternatives_dep, "alternatives");
            }
            _ => unreachable!("expected RPM info for el9"),
        }
    }

    #[test]
    fn test_ubuntu_info() {
        let info = Distro::Ubuntu2204.info();
        match info {
            DistroInfo::Deb(deb) => {
                assert_eq!(deb.dpkg_version, "1.21.1");
                assert!(deb.supports_zstd);
            }
            _ => unreachable!("expected DEB info for ubuntu2204"),
        }
    }

    #[test]
    fn test_compat_zstd_el8_ok() {
        let warnings = check_compatibility(&Distro::El8, "zstd", false, "rpm");
        assert!(
            warnings.is_empty(),
            "expected no warnings, got: {warnings:?}"
        );
    }

    #[test]
    fn test_compat_format_mismatch_rpm_on_deb_distro() {
        let warnings = check_compatibility(&Distro::Ubuntu2204, "zstd", false, "rpm");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("DEB-based but format is 'rpm'"));
    }

    #[test]
    fn test_compat_format_mismatch_deb_on_rpm_distro() {
        let warnings = check_compatibility(&Distro::El9, "gzip", false, "deb");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("RPM-based but format is 'deb'"));
    }

    #[test]
    fn test_minimum_rpm_version_zstd_large() {
        let (ver, reason) = minimum_rpm_version("zstd", true, false);
        assert_eq!(ver, "4.14.0");
        assert!(reason.contains("zstd"));
        assert!(reason.contains("large"));
    }

    #[test]
    fn test_minimum_rpm_version_gzip_only() {
        let (ver, _) = minimum_rpm_version("gzip", false, false);
        assert_eq!(ver, "4.6.0");
    }

    #[test]
    fn test_minimum_rpm_version_xz_large() {
        let (ver, _) = minimum_rpm_version("xz", true, false);
        assert_eq!(ver, "4.12.0");
    }

    #[test]
    fn test_minimum_dpkg_version_zstd() {
        let (ver, reason) = minimum_dpkg_version("zstd");
        assert_eq!(ver, "1.21.18");
        assert!(reason.contains("zstd"));
    }

    #[test]
    fn test_minimum_dpkg_version_gzip() {
        let (ver, _) = minimum_dpkg_version("gzip");
        assert_eq!(ver, "1.0.0");
    }
}
