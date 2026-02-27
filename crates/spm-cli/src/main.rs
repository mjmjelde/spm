use std::path::{Path, PathBuf};
use std::process;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use spm_core::config::Config;
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
        /// Output format: rpm, deb
        #[arg(short, long, default_value = "rpm")]
        format: String,
    },

    /// Build package(s) from config
    Build {
        /// Output format: rpm
        #[arg(short, long, default_value = "rpm")]
        format: String,

        /// Output directory
        #[arg(short, long, default_value = "./out")]
        output: PathBuf,
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
        Commands::Plan { ref format } => cmd_plan(&cli.config, format),
        Commands::Build {
            ref format,
            ref output,
        } => cmd_build(&cli.config, format, output),
        Commands::Init { name, version } => cmd_init(&name, &version),
    };

    if let Err(e) = result {
        eprintln!("Error: {e:#}");
        process::exit(1);
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
fn cmd_plan(config_path: &Path, format: &str) -> Result<()> {
    let config = Config::load(config_path)
        .with_context(|| format!("failed to load config from '{}'", config_path.display()))?;

    let limits = match format {
        "rpm" => FormatLimits::rpm(),
        "deb" => FormatLimits::deb(),
        other => anyhow::bail!("unsupported format '{}', expected 'rpm' or 'deb'", other),
    };

    let plan = Planner::plan(&config, &limits).with_context(|| "failed to create package plan")?;

    // Print plan summary.
    let output_filename = match format {
        "rpm" => PackageFileName::rpm(&plan.name, &plan.version, &plan.release, &plan.arch),
        "deb" => PackageFileName::deb(&plan.name, &plan.version, &plan.release, &plan.arch),
        _ => unreachable!(),
    };

    // Count total files (non-directory entries).
    let total_files: usize = plan
        .sub_packages
        .iter()
        .map(|sp| count_files(&sp.files))
        .sum();

    println!("Package: {output_filename}");
    println!("  Source: {}", config.content.source_dir.display());
    println!("  Files: {}", format_file_count(total_files));
    println!("  Uncompressed: {}", format_size(plan.total_size));

    // Estimated compressed size.
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
    if format == "rpm" {
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
            match &sp.role {
                SubPackageRole::Meta => {
                    let meta_filename = match format {
                        "rpm" => {
                            PackageFileName::rpm(&sp.name, &plan.version, &plan.release, &plan.arch)
                        }
                        "deb" => {
                            PackageFileName::deb(&sp.name, &plan.version, &plan.release, &plan.arch)
                        }
                        _ => unreachable!(),
                    };
                    println!("    {meta_filename}  (meta-package)");
                }
                SubPackageRole::Part(_) => {
                    let part_filename = match format {
                        "rpm" => {
                            PackageFileName::rpm(&sp.name, &plan.version, &plan.release, &plan.arch)
                        }
                        "deb" => {
                            PackageFileName::deb(&sp.name, &plan.version, &plan.release, &plan.arch)
                        }
                        _ => unreachable!(),
                    };
                    let file_count = count_files(&sp.files);
                    println!(
                        "    {part_filename}  (~{}, {} files)",
                        format_size(sp.total_size),
                        format_file_count(file_count)
                    );
                }
                SubPackageRole::Standalone => {} // shouldn't happen in split
            }
        }
    } else {
        println!("  Splitting: NOT REQUIRED");
    }

    println!();
    println!("  Output: out/{output_filename}");

    Ok(())
}

/// Build packages from config.
fn cmd_build(config_path: &Path, format: &str, output_dir: &Path) -> Result<()> {
    let config = Config::load(config_path)
        .with_context(|| format!("failed to load config from '{}'", config_path.display()))?;

    if format != "rpm" {
        anyhow::bail!("only 'rpm' format is supported in this version, got '{format}'");
    }

    let limits = FormatLimits::rpm();
    let plan = Planner::plan(&config, &limits).with_context(|| "failed to create package plan")?;

    // Create output directory if needed.
    std::fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "failed to create output directory '{}'",
            output_dir.display()
        )
    })?;

    // Build each sub-package.
    for sub_pkg in &plan.sub_packages {
        let filename =
            PackageFileName::rpm(&sub_pkg.name, &plan.version, &plan.release, &plan.arch);
        let output_path = output_dir.join(&filename);

        println!("Building {filename}...");

        spm_rpm::builder::RpmBuilder::build(sub_pkg, &plan, &config, &output_path)
            .with_context(|| format!("failed to build RPM '{filename}'"))?;

        println!("  -> {}", output_path.display());
    }

    println!("Done.");
    Ok(())
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
