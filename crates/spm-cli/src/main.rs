use std::path::{Path, PathBuf};
use std::process;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use spm_core::config::Config;

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
