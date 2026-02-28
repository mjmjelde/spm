use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};

use spm_core::config::{BuildConfig, Config};
use spm_core::distro::{self, Distro};
use spm_core::planner::{count_files, Planner, SubPackageRole};
use spm_core::types::{format_size, FormatLimits, PackageFileName};

/// spm — Large-file-aware Linux package builder
#[derive(Parser)]
#[command(name = "spm", version, about)]
struct Cli {
    /// Config file path
    #[arg(short, long, global = true, default_value = "spm.yaml")]
    config: PathBuf,

    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Suppress non-error output
    #[arg(short, long, global = true)]
    quiet: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Validate a spm.yaml without building
    Validate,

    /// Show what would be built (dry run)
    Plan {
        /// Output format: rpm, deb, all
        #[arg(short, long, default_value = "all")]
        format: String,

        /// Disable package splitting
        #[arg(long)]
        no_split: bool,

        /// Override source_date_epoch for reproducible builds
        #[arg(long)]
        source_date_epoch: Option<String>,

        /// Target distribution for compatibility checks
        #[arg(long)]
        target_distro: Option<String>,

        /// Override compression algorithm
        #[arg(long)]
        compression: Option<String>,

        /// Override compression level
        #[arg(long)]
        compression_level: Option<i32>,

        /// Override thread count for compression
        #[arg(long)]
        threads: Option<usize>,
    },

    /// Build package(s) from config
    Build {
        /// Output format: rpm, deb, all
        #[arg(short, long, default_value = "all")]
        format: String,

        /// Output directory
        #[arg(short, long, default_value = "./out")]
        output: PathBuf,

        /// Disable package splitting
        #[arg(long)]
        no_split: bool,

        /// Override source_date_epoch for reproducible builds
        #[arg(long)]
        source_date_epoch: Option<String>,

        /// Target distribution for compatibility checks
        #[arg(long)]
        target_distro: Option<String>,

        /// Override compression algorithm
        #[arg(long)]
        compression: Option<String>,

        /// Override compression level
        #[arg(long)]
        compression_level: Option<i32>,

        /// Override thread count for compression
        #[arg(long)]
        threads: Option<usize>,
    },

    /// Show metadata for an existing package
    Inspect {
        /// Path to .rpm or .deb file
        path: PathBuf,
    },

    /// Create a template spm.yaml
    Init {
        /// Package name
        #[arg(long)]
        name: String,

        /// Package version
        #[arg(long)]
        version: String,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Validate => cmd_validate(&cli.config),
        Commands::Plan {
            ref format,
            no_split,
            ref source_date_epoch,
            ref target_distro,
            ref compression,
            compression_level,
            threads,
        } => cmd_plan(
            &cli.config,
            format,
            no_split,
            source_date_epoch.as_deref(),
            target_distro.as_deref(),
            compression.as_deref(),
            compression_level,
            threads,
        ),
        Commands::Build {
            ref format,
            ref output,
            no_split,
            ref source_date_epoch,
            ref target_distro,
            ref compression,
            compression_level,
            threads,
        } => cmd_build(
            &cli.config,
            format,
            output,
            no_split,
            source_date_epoch.as_deref(),
            target_distro.as_deref(),
            compression.as_deref(),
            compression_level,
            threads,
            cli.quiet,
        ),
        Commands::Inspect { ref path } => cmd_inspect(path),
        Commands::Init { name, version } => cmd_init(&name, &version),
    };

    if let Err(e) = result {
        eprintln!("Error: {e:#}");
        process::exit(1);
    }
}

/// Apply CLI flag overrides to a config (mutated in place).
fn apply_overrides(
    config: &mut Config,
    no_split: bool,
    source_date_epoch: Option<&str>,
    compression: Option<&str>,
    compression_level: Option<i32>,
    threads: Option<usize>,
) {
    if no_split {
        config.splitting.enabled = false;
    }

    // Priority: CLI flag > env var > config file
    let epoch = source_date_epoch
        .map(|s| s.to_string())
        .or_else(|| std::env::var("SOURCE_DATE_EPOCH").ok());
    if let Some(epoch_val) = epoch {
        let build = config.build.get_or_insert(BuildConfig {
            source_date_epoch: None,
        });
        build.source_date_epoch = Some(epoch_val);
    }

    if let Some(algo) = compression {
        config.compression.algorithm = algo.to_string();
    }
    if let Some(level) = compression_level {
        config.compression.level = Some(level);
    }
    if let Some(t) = threads {
        config.compression.threads = Some(t);
    }
}

/// Parse --target-distro flag value into a Distro.
fn parse_target_distro(target_distro: Option<&str>) -> Result<Option<Distro>> {
    match target_distro {
        Some(s) => match Distro::from_str(s) {
            Some(d) => Ok(Some(d)),
            None => anyhow::bail!(
                "unknown target distro '{s}'; expected one of: el8, el9, ubuntu2004, ubuntu2204, ubuntu2404, fedora"
            ),
        },
        None => Ok(None),
    }
}

/// Print target distro compatibility warnings to stderr.
fn print_distro_warnings(distro: &Distro, config: &Config, format: &str, plan_total_size: u64) {
    let has_large_files = plan_total_size > FormatLimits::rpm().max_file_size_standard;
    let warnings = distro::check_compatibility(
        distro,
        &config.compression.algorithm,
        has_large_files,
        format,
    );
    for w in &warnings {
        eprintln!("Warning: {w}");
    }
}

/// Return the formats to iterate over, given the --format flag value.
fn resolve_formats(format: &str) -> Result<Vec<&str>> {
    match format {
        "all" => Ok(vec!["rpm", "deb"]),
        "rpm" => Ok(vec!["rpm"]),
        "deb" => Ok(vec!["deb"]),
        other => anyhow::bail!("unsupported format '{other}', expected 'rpm', 'deb', or 'all'"),
    }
}

/// Load and validate the config, printing a summary on success.
fn cmd_validate(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)
        .with_context(|| format!("failed to load config from '{}'", config_path.display()))?;

    println!(
        "Config valid: {} {}-{} ({})",
        config.package.name, config.package.version, config.package.release, config.package.arch,
    );

    Ok(())
}

/// Run the planner and print a summary of what would be built.
#[allow(clippy::too_many_arguments)]
fn cmd_plan(
    config_path: &Path,
    format: &str,
    no_split: bool,
    source_date_epoch: Option<&str>,
    target_distro: Option<&str>,
    compression: Option<&str>,
    compression_level: Option<i32>,
    threads: Option<usize>,
) -> Result<()> {
    let mut config = Config::load(config_path)
        .with_context(|| format!("failed to load config from '{}'", config_path.display()))?;

    apply_overrides(
        &mut config,
        no_split,
        source_date_epoch,
        compression,
        compression_level,
        threads,
    );

    let distro = parse_target_distro(target_distro)?;
    let formats = resolve_formats(format)?;
    let config_dir = config_path.parent().unwrap_or(Path::new("."));

    for (idx, fmt) in formats.iter().enumerate() {
        if formats.len() > 1 && idx > 0 {
            println!();
        }

        let limits = match *fmt {
            "rpm" => FormatLimits::rpm(),
            "deb" => FormatLimits::deb(),
            _ => unreachable!(),
        };

        let plan = Planner::plan(&config, &limits, config_dir)
            .with_context(|| format!("failed to create {fmt} package plan"))?;

        let output_filename = match *fmt {
            "rpm" => PackageFileName::rpm(&plan.name, &plan.version, &plan.release, &plan.arch),
            "deb" => PackageFileName::deb(&plan.name, &plan.version, &plan.release, &plan.arch),
            _ => unreachable!(),
        };

        let total_files: usize = plan
            .sub_packages
            .iter()
            .map(|sp| count_files(&sp.files))
            .sum();

        println!("Package: {output_filename}");
        println!("  Source: {}", config.content.source_dir.display());
        println!("  Files: {}", format_file_count(total_files));
        println!("  Uncompressed: {}", format_size(plan.total_size));

        let ratio = spm_core::types::estimated_compression_ratio(&config.compression.algorithm);
        let estimated_compressed = (plan.total_size as f64 * ratio) as u64;
        let level_str = config
            .compression
            .level
            .map(|l| format!(" -{l}"))
            .unwrap_or_default();
        println!(
            "  Estimated compressed ({}{level_str}): ~{}",
            config.compression.algorithm,
            format_size(estimated_compressed)
        );
        println!();

        // RPM-specific: cpio format.
        if *fmt == "rpm" {
            if plan.needs_extended_cpio {
                println!("  RPM payload format: 07070X (extended cpio, files > 4 GiB detected)");
            } else {
                println!("  RPM payload format: 070701 (standard cpio)");
            }
        }

        // Splitting info.
        if plan.is_split {
            println!("  Splitting: REQUIRED");
            println!("  Split plan:");
            for sp in &plan.sub_packages {
                let sp_filename = match *fmt {
                    "rpm" => {
                        PackageFileName::rpm(&sp.name, &plan.version, &plan.release, &plan.arch)
                    }
                    "deb" => {
                        PackageFileName::deb(&sp.name, &plan.version, &plan.release, &plan.arch)
                    }
                    _ => unreachable!(),
                };
                match &sp.role {
                    SubPackageRole::Meta => {
                        println!("    {sp_filename}  (meta-package)");
                    }
                    SubPackageRole::Part(_) => {
                        let file_count = count_files(&sp.files);
                        println!(
                            "    {sp_filename}  (~{}, {} files)",
                            format_size(sp.total_size),
                            format_file_count(file_count)
                        );
                    }
                    SubPackageRole::Standalone => {}
                }
            }
        } else if *fmt == "rpm" {
            println!("  Splitting: NOT REQUIRED (RPM supports packages > 4 GiB with rpm >= 4.6)");
        } else {
            println!("  Splitting: NOT REQUIRED");
        }

        // Minimum version info.
        match *fmt {
            "rpm" => {
                let (min_ver, reason) = distro::minimum_rpm_version(
                    &config.compression.algorithm,
                    plan.total_size > limits.max_file_size_standard,
                    plan.needs_extended_cpio,
                );
                println!("  Minimum rpm version: {min_ver} ({reason})");
            }
            "deb" => {
                let (min_ver, reason) = distro::minimum_dpkg_version(&config.compression.algorithm);
                println!("  Minimum dpkg version: {min_ver} ({reason})");
            }
            _ => unreachable!(),
        }

        // Target distro warnings.
        if let Some(ref d) = distro {
            print_distro_warnings(d, &config, fmt, plan.total_size);
        }

        println!();
        println!("  Output: out/{output_filename}");
    }

    Ok(())
}

/// Build packages from config.
#[allow(clippy::too_many_arguments)]
fn cmd_build(
    config_path: &Path,
    format: &str,
    output_dir: &Path,
    no_split: bool,
    source_date_epoch: Option<&str>,
    target_distro: Option<&str>,
    compression: Option<&str>,
    compression_level: Option<i32>,
    threads: Option<usize>,
    quiet: bool,
) -> Result<()> {
    let mut config = Config::load(config_path)
        .with_context(|| format!("failed to load config from '{}'", config_path.display()))?;

    apply_overrides(
        &mut config,
        no_split,
        source_date_epoch,
        compression,
        compression_level,
        threads,
    );

    let distro = parse_target_distro(target_distro)?;
    let formats = resolve_formats(format)?;
    let config_dir = config_path.parent().unwrap_or(Path::new("."));

    std::fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "failed to create output directory '{}'",
            output_dir.display()
        )
    })?;

    for fmt in &formats {
        let limits = match *fmt {
            "rpm" => FormatLimits::rpm(),
            "deb" => FormatLimits::deb(),
            _ => unreachable!(),
        };

        let plan = Planner::plan(&config, &limits, config_dir)
            .with_context(|| format!("failed to create {fmt} package plan"))?;

        if let Some(ref d) = distro {
            print_distro_warnings(d, &config, fmt, plan.total_size);
        }

        let total_sub = plan.sub_packages.len();
        match *fmt {
            "rpm" => {
                for (i, sub_pkg) in plan.sub_packages.iter().enumerate() {
                    let filename = PackageFileName::rpm(
                        &sub_pkg.name,
                        &plan.version,
                        &plan.release,
                        &plan.arch,
                    );
                    let output_path = output_dir.join(&filename);

                    let spinner = make_spinner(quiet);
                    spinner.set_message(format!("Building {filename}..."));

                    spm_rpm::builder::RpmBuilder::build(
                        sub_pkg,
                        &plan,
                        &config,
                        &output_path,
                        distro.as_ref(),
                    )
                    .with_context(|| {
                        format!(
                            "failed to build RPM '{filename}' (sub-package {} of {total_sub})",
                            i + 1
                        )
                    })?;

                    spinner.finish_with_message(format!(
                        "{filename} ({})",
                        format_size(sub_pkg.total_size)
                    ));
                }
            }
            "deb" => {
                let spinner = make_spinner(quiet);
                let deb_label = format!("{} {}-{}", plan.name, plan.version, plan.release);
                spinner.set_message(format!("Building DEB packages for {deb_label}..."));

                let output_paths = spm_deb::builder::DebBuilder::build(&plan, &config, output_dir)
                    .with_context(|| format!("failed to build DEB packages for '{deb_label}'"))?;

                spinner.finish_and_clear();
                for path in &output_paths {
                    let filename = path.file_name().unwrap_or_default().to_string_lossy();
                    if !quiet {
                        println!("{filename}");
                    }
                }
            }
            _ => unreachable!(),
        }
    }

    if !quiet {
        println!("Done.");
    }
    Ok(())
}

/// Show metadata for an existing .rpm or .deb package.
fn cmd_inspect(path: &Path) -> Result<()> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    match ext {
        "rpm" => {
            let meta = spm_rpm::reader::read_rpm_metadata(path)
                .with_context(|| format!("failed to read RPM '{}'", path.display()))?;

            println!("Package: {}", meta.name);
            println!("Version: {}", meta.version);
            println!("Release: {}", meta.release);
            println!("Architecture: {}", meta.arch);
            println!("Installed Size: {}", format_size(meta.size));
            if !meta.description.is_empty() {
                println!("Description: {}", meta.description);
            }
            println!("License: {}", meta.license);
            if let Some(url) = &meta.url {
                println!("URL: {url}");
            }
            if let Some(vendor) = &meta.vendor {
                println!("Vendor: {vendor}");
            }
            if let Some(packager) = &meta.packager {
                println!("Packager: {packager}");
            }
            if let Some(comp) = &meta.compressor {
                println!("Compression: {comp}");
            }
            println!("Files: {}", format_file_count(meta.file_count));
            if !meta.requires.is_empty() {
                println!("Requires:");
                for req in &meta.requires {
                    println!("  - {req}");
                }
            }
        }
        "deb" => {
            let meta = spm_deb::reader::read_deb_metadata(path)
                .with_context(|| format!("failed to read DEB '{}'", path.display()))?;

            for (key, value) in &meta.fields {
                println!("{key}: {value}");
            }
        }
        _ => {
            anyhow::bail!("unsupported file extension '.{ext}', expected '.rpm' or '.deb'");
        }
    }

    Ok(())
}

/// Create a progress spinner, or a hidden one if quiet mode is on.
fn make_spinner(quiet: bool) -> ProgressBar {
    if quiet {
        return ProgressBar::hidden();
    }
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );
    spinner.enable_steady_tick(Duration::from_millis(100));
    spinner
}

/// Format a file count with comma separators.
fn format_file_count(count: usize) -> String {
    let s = count.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Write a template spm.yaml to the current directory.
fn cmd_init(name: &str, version: &str) -> Result<()> {
    let output_path = PathBuf::from("spm.yaml");
    if output_path.exists() {
        anyhow::bail!("spm.yaml already exists in the current directory");
    }

    let template = format!(
        r#"# spm.yaml — Package configuration for {name}
# See spec.md for full documentation of all fields.

package:
  name: {name}
  version: "{version}"
  release: "1"
  arch: x86_64
  license: Proprietary
  maintainer: "Your Name <you@example.com>"
  description: "{name} {version}"
  # url: https://example.com
  # vendor: YourOrg

  dependencies:
    requires: []
    # requires_rpm: []
    # requires_deb: []
    # conflicts: []
    # provides: []
    # replaces: []

content:
  # Directory containing the files to package
  source_dir: /path/to/staged/files

  files:
    - src: "/path/to/staged/files/**"
      dst: /opt/{name}/

  # symlinks: []

  # alternatives:
  #   - name: {name}
  #     link: /usr/bin/{name}
  #     path: /opt/{name}/bin/{name}
  #     priority: 100

  directories: []

# scripts:
#   pre_install: scripts/preinst.sh
#   post_install: scripts/postinst.sh
#   pre_remove: scripts/prerm.sh
#   post_remove: scripts/postrm.sh

compression:
  algorithm: zstd
  # level: 3
  threads: 0

splitting:
  enabled: true
  strategy: auto

# signing:
#   key_file: ${{SPM_SIGNING_KEY}}

# rpm:
#   group: Development/Tools
#   payload_format: cpio

# deb:
#   section: misc
#   priority: optional

# build:
#   source_date_epoch: "1700000000"
"#
    );

    std::fs::write(&output_path, template)
        .with_context(|| format!("failed to write '{}'", output_path.display()))?;

    println!("Created {}", output_path.display());
    Ok(())
}
