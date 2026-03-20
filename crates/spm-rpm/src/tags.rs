//! RPM tag constants, data types, and flag definitions.
//!
//! All tag numbers and type codes follow the RPM v4 specification.
//! Constants are organized by category: package metadata, file metadata,
//! dependencies, scripts, and signature.

/// RPM header tag data types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum TagType {
    /// Unused/null.
    Null = 0,
    /// Single 8-bit character.
    Char = 1,
    /// Array of unsigned 8-bit integers.
    Int8 = 2,
    /// Array of unsigned 16-bit integers.
    Int16 = 3,
    /// Array of signed 32-bit integers.
    Int32 = 4,
    /// Array of signed 64-bit integers.
    Int64 = 5,
    /// Single NUL-terminated string.
    String = 6,
    /// Binary blob (arbitrary bytes).
    Bin = 7,
    /// Array of NUL-terminated strings.
    StringArray = 8,
    /// Internationalized string (treated like String).
    I18NString = 9,
}

// ── Package metadata tags ──────────────────────────────────────────

/// Package name.
pub const RPMTAG_NAME: u32 = 1000;
/// Package vendor.
pub const RPMTAG_VENDOR: u32 = 1011;
/// Package version.
pub const RPMTAG_VERSION: u32 = 1001;
/// Package release.
pub const RPMTAG_RELEASE: u32 = 1002;
/// One-line summary.
pub const RPMTAG_SUMMARY: u32 = 1004;
/// Multi-line description.
pub const RPMTAG_DESCRIPTION: u32 = 1005;
/// Build timestamp (seconds since epoch).
pub const RPMTAG_BUILDTIME: u32 = 1006;
/// Build hostname.
pub const RPMTAG_BUILDHOST: u32 = 1007;
/// Installed size in bytes (32-bit).
pub const RPMTAG_SIZE: u32 = 1009;
/// License string.
pub const RPMTAG_LICENSE: u32 = 1014;
/// Packager name/email.
pub const RPMTAG_PACKAGER: u32 = 1015;
/// Package group.
pub const RPMTAG_GROUP: u32 = 1016;
/// Project URL.
pub const RPMTAG_URL: u32 = 1020;
/// Operating system (e.g., "linux").
pub const RPMTAG_OS: u32 = 1021;
/// Architecture (e.g., "x86_64").
pub const RPMTAG_ARCH: u32 = 1022;
/// Source RPM filename (empty for binary-only).
pub const RPMTAG_SOURCERPM: u32 = 1044;
/// RPM tool version that built this package.
pub const RPMTAG_RPMVERSION: u32 = 1064;
/// Compiler optimization flags.
pub const RPMTAG_OPTFLAGS: u32 = 1122;
/// Payload archive format (e.g., "cpio").
pub const RPMTAG_PAYLOADFORMAT: u32 = 1124;
/// Payload compression algorithm (e.g., "zstd").
pub const RPMTAG_PAYLOADCOMPRESSOR: u32 = 1125;
/// Payload compression flags (e.g., compression level).
pub const RPMTAG_PAYLOADFLAGS: u32 = 1126;

// ── File metadata tags ─────────────────────────────────────────────

/// Per-file sizes in bytes (32-bit).
pub const RPMTAG_FILESIZES: u32 = 1028;
/// Per-file modes (16-bit).
pub const RPMTAG_FILEMODES: u32 = 1030;
/// Per-file rdev values.
pub const RPMTAG_FILERDEVS: u32 = 1033;
/// Per-file modification times.
pub const RPMTAG_FILEMTIMES: u32 = 1034;
/// Per-file digest strings (hex SHA-256).
pub const RPMTAG_FILEDIGESTS: u32 = 1035;
/// Per-file symlink targets.
pub const RPMTAG_FILELINKTOS: u32 = 1036;
/// Per-file flags (config, noreplace, etc.).
pub const RPMTAG_FILEFLAGS: u32 = 1037;
/// Per-file owner usernames.
pub const RPMTAG_FILEUSERNAME: u32 = 1039;
/// Per-file group names.
pub const RPMTAG_FILEGROUPNAME: u32 = 1040;
/// Per-file device numbers.
pub const RPMTAG_FILEDEVICES: u32 = 1095;
/// Per-file inode numbers.
pub const RPMTAG_FILEINODES: u32 = 1096;
/// Per-file language strings.
pub const RPMTAG_FILELANGS: u32 = 1097;
/// Directory index for each file.
pub const RPMTAG_DIRINDEXES: u32 = 1116;
/// File basenames.
pub const RPMTAG_BASENAMES: u32 = 1117;
/// Unique directory paths (each ending with `/`).
pub const RPMTAG_DIRNAMES: u32 = 1118;
/// Per-file color (ELF classification).
pub const RPMTAG_FILECOLORS: u32 = 1140;
/// Per-file class index.
pub const RPMTAG_FILECLASS: u32 = 1141;
/// File verification flags.
pub const RPMTAG_FILEVERIFYFLAGS: u32 = 1045;

// ── Large file tags (64-bit) ───────────────────────────────────────

/// Per-file sizes in bytes (64-bit).
pub const RPMTAG_LONGFILESIZES: u32 = 5008;
/// Total installed size (64-bit).
pub const RPMTAG_LONGSIZE: u32 = 5009;
/// Digest algorithm identifier.
pub const RPMTAG_FILEDIGESTALGO: u32 = 5011;
/// Header string encoding declaration (RPM 4.14+).
pub const RPMTAG_ENCODING: u32 = 5062;
/// Per-payload SHA-256 digest (RPM 4.14+).
pub const RPMTAG_PAYLOADDIGEST: u32 = 5092;
/// Payload digest algorithm identifier (RPM 4.14+).
pub const RPMTAG_PAYLOADDIGESTALGO: u32 = 5093;

// ── Dependency tags ────────────────────────────────────────────────

/// Provided capability names.
pub const RPMTAG_PROVIDENAME: u32 = 1047;
/// Require dependency flags.
pub const RPMTAG_REQUIREFLAGS: u32 = 1048;
/// Required capability names.
pub const RPMTAG_REQUIRENAME: u32 = 1049;
/// Required capability versions.
pub const RPMTAG_REQUIREVERSION: u32 = 1050;
/// Provide dependency flags.
pub const RPMTAG_PROVIDEFLAGS: u32 = 1112;
/// Provided capability versions.
pub const RPMTAG_PROVIDEVERSION: u32 = 1113;

// ── Conflict/obsolete dependency tags ─────────────────────────────

/// Conflict dependency flags.
pub const RPMTAG_CONFLICTFLAGS: u32 = 1053;
/// Conflict capability names.
pub const RPMTAG_CONFLICTNAME: u32 = 1054;
/// Conflict capability versions.
pub const RPMTAG_CONFLICTVERSION: u32 = 1055;
/// Obsolete dependency flags.
pub const RPMTAG_OBSOLETEFLAGS: u32 = 1114;
/// Obsolete capability versions.
pub const RPMTAG_OBSOLETEVERSION: u32 = 1115;
/// Obsolete capability names.
pub const RPMTAG_OBSOLETENAME: u32 = 1090;

// ── Script tags ────────────────────────────────────────────────────

/// Pre-install script body.
pub const RPMTAG_PREIN: u32 = 1023;
/// Post-install script body.
pub const RPMTAG_POSTIN: u32 = 1024;
/// Pre-uninstall script body.
pub const RPMTAG_PREUN: u32 = 1025;
/// Post-uninstall script body.
pub const RPMTAG_POSTUN: u32 = 1026;
/// Pre-install script interpreter.
pub const RPMTAG_PREINPROG: u32 = 1085;
/// Post-install script interpreter.
pub const RPMTAG_POSTINPROG: u32 = 1086;
/// Pre-uninstall script interpreter.
pub const RPMTAG_PREUNPROG: u32 = 1087;
/// Post-uninstall script interpreter.
pub const RPMTAG_POSTUNPROG: u32 = 1088;
/// Pre-transaction script body.
pub const RPMTAG_PRETRANS: u32 = 1151;
/// Post-transaction script body.
pub const RPMTAG_POSTTRANS: u32 = 1152;
/// Pre-transaction script interpreter.
pub const RPMTAG_PRETRANSPROG: u32 = 1153;
/// Post-transaction script interpreter.
pub const RPMTAG_POSTTRANSPROG: u32 = 1154;

// ── Region tags ───────────────────────────────────────────────────

/// Header region tag for signature header.
pub const RPMTAG_HEADERSIGNATURES: u32 = 62;
/// Header region tag for immutable metadata header.
pub const RPMTAG_HEADERIMMUTABLE: u32 = 63;

// ── Signature header tags ──────────────────────────────────────────

/// Header + payload size in bytes (32-bit, in signature header).
pub const RPMSIGTAG_SIZE: u32 = 1000;
/// MD5 digest of header + payload (in signature header).
pub const RPMSIGTAG_MD5: u32 = 1004;
/// Uncompressed payload size (32-bit, in signature header).
pub const RPMSIGTAG_PAYLOADSIZE: u32 = 1007;
/// Header + payload size (64-bit, in signature header).
pub const RPMSIGTAG_LONGSIZE: u32 = 270;
/// Uncompressed payload size (64-bit, in signature header).
pub const RPMSIGTAG_LONGARCHIVESIZE: u32 = 271;
/// SHA-1 hex digest of header only (in signature header).
pub const RPMSIGTAG_SHA1: u32 = 269;
/// SHA-256 hex digest of header only (in signature header).
pub const RPMSIGTAG_SHA256: u32 = 273;

// ── File flag constants ────────────────────────────────────────────

/// File is a configuration file.
pub const RPMFILE_CONFIG: u32 = 1;
/// Config file should not be replaced on upgrade.
pub const RPMFILE_NOREPLACE: u32 = 1 << 7;

// ── Dependency sense flags ─────────────────────────────────────────

/// No specific version requirement.
pub const RPMSENSE_ANY: u32 = 0;
/// Version must be less than.
pub const RPMSENSE_LESS: u32 = 0x02;
/// Version must be greater than.
pub const RPMSENSE_GREATER: u32 = 0x04;
/// Version must be equal to.
pub const RPMSENSE_EQUAL: u32 = 0x08;
/// Dependency is on an rpmlib feature.
pub const RPMSENSE_RPMLIB: u32 = 0x0100_0000;

// ── Digest algorithm constants ─────────────────────────────────────

/// SHA-256 digest algorithm identifier.
pub const PGPHASHALGO_SHA256: u32 = 8;

// ── Default file verification flags ────────────────────────────────

/// Default verification flags (verify everything).
pub const RPMVERIFY_ALL: u32 = 0xFFFF_FFFF;
