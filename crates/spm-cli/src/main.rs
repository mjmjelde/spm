use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use spm_core::config::{BuildConfig, Config};
use spm_core::deps::{self, DepFormat};
use spm_core::distro::{self, Distro};
use spm_core::planner::{count_files, Planner, SubPackageRole};
use spm_core::progress::{BuildProgress, BuildStage};
use spm_core::types::{format_size, FormatLimits, PackageFileName};

/// spm — Large-file-aware Linux package builder
#[derive(Parser)]
#[command(name = "spm", version, about)]
struct Cli {
    /// Config file path
    #[arg(short, long, global = true, default_value = "spm.yaml")]
    config: PathBuf,

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
) -> Result<()> {
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
        // Validate early so `spm plan` catches invalid algorithms too.
        spm_compress::Algorithm::from_str(algo)
            .map_err(|_| anyhow::anyhow!("unsupported compression algorithm '{algo}'; expected 'zstd', 'gzip', 'xz', or 'none'"))?;
        config.compression.algorithm = algo.to_string();
    }
    if let Some(level) = compression_level {
        config.compression.level = Some(level);
    }
    if let Some(t) = threads {
        config.compression.threads = Some(t);
    }

    Ok(())
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

    let dep_errors = deps::validate_all_deps_lenient(&config.package.dependencies);
    if !dep_errors.is_empty() {
        anyhow::bail!("invalid dependencies:\n  {}", dep_errors.join("\n  "));
    }

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
    )?;

    let distro = parse_target_distro(target_distro)?;
    let formats = resolve_formats(format)?;
    let config_dir = config_path.parent().unwrap_or(Path::new("."));

    for (idx, fmt) in formats.iter().enumerate() {
        if formats.len() > 1 && idx > 0 {
            println!();
        }

        let dep_format = match *fmt {
            "rpm" => DepFormat::Rpm,
            "deb" => DepFormat::Deb,
            _ => unreachable!(),
        };
        let dep_errors = deps::validate_all_deps(&config.package.dependencies, dep_format);
        if !dep_errors.is_empty() {
            anyhow::bail!("invalid {fmt} dependencies:\n  {}", dep_errors.join("\n  "));
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
        if plan.deferred_split {
            println!(
                "  Splitting: AUTO (actual split point determined at build time \
                 based on compressed size)"
            );
        } else if plan.is_split {
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

        // Plan warnings (e.g. near format limits).
        for warning in &plan.warnings {
            println!();
            println!("  Warning: {warning}");
        }

        println!();
        println!("  Output: ./out/{output_filename}");
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
    )?;

    let distro = parse_target_distro(target_distro)?;
    let formats = resolve_formats(format)?;
    let config_dir = config_path.parent().unwrap_or(Path::new("."));

    std::fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "failed to create output directory '{}'",
            output_dir.display()
        )
    })?;

    let build_start = Instant::now();
    let multi = MultiProgress::new();
    let mut build_results: Vec<BuildResult> = Vec::new();

    for fmt in &formats {
        let dep_format = match *fmt {
            "rpm" => DepFormat::Rpm,
            "deb" => DepFormat::Deb,
            _ => unreachable!(),
        };
        let dep_errors = deps::validate_all_deps(&config.package.dependencies, dep_format);
        if !dep_errors.is_empty() {
            anyhow::bail!("invalid {fmt} dependencies:\n  {}", dep_errors.join("\n  "));
        }

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

        if !quiet {
            multi.println(format!("[{fmt}]")).ok();
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

                    let progress = if quiet {
                        None
                    } else {
                        Some(IndicatifProgress::new(&multi, &filename))
                    };

                    let pkg_start = Instant::now();

                    spm_rpm::builder::RpmBuilder::build(
                        sub_pkg,
                        &plan,
                        &config,
                        &output_path,
                        distro.as_ref(),
                        progress.as_ref().map(|p| p as &dyn BuildProgress),
                    )
                    .with_context(|| {
                        format!(
                            "failed to build RPM '{filename}' (sub-package {} of {total_sub})",
                            i + 1
                        )
                    })?;

                    let elapsed = pkg_start.elapsed();
                    let output_size = std::fs::metadata(&output_path)
                        .map(|m| m.len())
                        .unwrap_or(0);

                    if let Some(ref p) = progress {
                        p.finish(output_size, elapsed);
                    }

                    build_results.push(BuildResult {
                        filename,
                        file_count: count_files(&sub_pkg.files),
                        uncompressed_size: sub_pkg.total_size,
                        compressed_size: output_size,
                        duration: elapsed,
                    });
                }
            }
            "deb" => {
                if plan.deferred_split {
                    // Streaming split: builder monitors actual compressed
                    // sizes and partitions automatically.
                    let label = format!(
                        "{} (auto-split)",
                        PackageFileName::deb(&plan.name, &plan.version, &plan.release, &plan.arch,),
                    );
                    let progress = if quiet {
                        None
                    } else {
                        Some(IndicatifProgress::new(&multi, &label))
                    };

                    let pkg_start = Instant::now();

                    let paths = spm_deb::builder::DebBuilder::build(
                        &plan,
                        &config,
                        output_dir,
                        progress.as_ref().map(|p| p as &dyn BuildProgress),
                    )
                    .context("failed to build DEB with streaming split")?;

                    let elapsed = pkg_start.elapsed();
                    if let Some(ref p) = progress {
                        let total_output: u64 = paths
                            .iter()
                            .filter_map(|p| std::fs::metadata(p).ok())
                            .map(|m| m.len())
                            .sum();
                        p.finish(total_output, elapsed);
                    }

                    let total_files = count_files(&plan.sub_packages[0].files);
                    let total_compressed: u64 = paths
                        .iter()
                        .filter_map(|p| std::fs::metadata(p).ok())
                        .map(|m| m.len())
                        .sum();

                    let summary_name = if paths.len() > 1 {
                        // Multiple files: meta-package + N parts.
                        let part_count = paths.len() - 1;
                        format!(
                            "{} ({part_count} parts)",
                            PackageFileName::deb(
                                &plan.name,
                                &plan.version,
                                &plan.release,
                                &plan.arch,
                            ),
                        )
                    } else {
                        paths[0]
                            .file_name()
                            .unwrap_or_else(|| paths[0].as_os_str())
                            .to_string_lossy()
                            .to_string()
                    };

                    build_results.push(BuildResult {
                        filename: summary_name,
                        file_count: total_files,
                        uncompressed_size: plan.total_size,
                        compressed_size: total_compressed,
                        duration: elapsed,
                    });
                } else {
                    for (i, sub_pkg) in plan.sub_packages.iter().enumerate() {
                        let filename = PackageFileName::deb(
                            &sub_pkg.name,
                            &plan.version,
                            &plan.release,
                            &plan.arch,
                        );
                        let output_path = output_dir.join(&filename);

                        let progress = if quiet {
                            None
                        } else {
                            Some(IndicatifProgress::new(&multi, &filename))
                        };

                        let pkg_start = Instant::now();

                        // For meta-packages, compute Depends on all parts.
                        let extra_depends: Vec<String> = if sub_pkg.role == SubPackageRole::Meta {
                            plan.sub_packages
                                .iter()
                                .filter(|sp| matches!(sp.role, SubPackageRole::Part(_)))
                                .map(|sp| {
                                    format!("{} (= {}-{})", sp.name, plan.version, plan.release)
                                })
                                .collect()
                        } else {
                            Vec::new()
                        };

                        spm_deb::builder::build_single_deb(
                            sub_pkg,
                            &plan,
                            &config,
                            &output_path,
                            &extra_depends,
                            progress.as_ref().map(|p| p as &dyn BuildProgress),
                        )
                        .with_context(|| {
                            format!(
                                "failed to build DEB '{filename}' \
                                 (sub-package {} of {total_sub})",
                                i + 1
                            )
                        })?;

                        let elapsed = pkg_start.elapsed();
                        let output_size = std::fs::metadata(&output_path)
                            .map(|m| m.len())
                            .unwrap_or(0);

                        if let Some(ref p) = progress {
                            p.finish(output_size, elapsed);
                        }

                        build_results.push(BuildResult {
                            filename,
                            file_count: count_files(&sub_pkg.files),
                            uncompressed_size: sub_pkg.total_size,
                            compressed_size: output_size,
                            duration: elapsed,
                        });
                    }
                }
            }
            _ => unreachable!(),
        }
    }

    if !quiet {
        print_build_summary(&build_results, build_start.elapsed());
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

// ---------------------------------------------------------------------------
// Progress reporting
// ---------------------------------------------------------------------------

/// Per-package build result, collected for the final summary.
struct BuildResult {
    filename: String,
    file_count: usize,
    uncompressed_size: u64,
    compressed_size: u64,
    duration: Duration,
}

/// indicatif-backed progress reporter for a single sub-package build.
///
/// Shows a spinner with the current stage name and filename, plus a
/// byte-level progress bar during data-intensive stages.
struct IndicatifProgress {
    multi: MultiProgress,
    overall: ProgressBar,
    filename: String,
    detail: RefCell<Option<ProgressBar>>,
    stage_start_time: RefCell<Option<Instant>>,
}

impl IndicatifProgress {
    fn new(multi: &MultiProgress, filename: &str) -> Self {
        let overall = multi.add(ProgressBar::new_spinner());
        overall.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap(),
        );
        overall.enable_steady_tick(Duration::from_millis(100));
        overall.set_message(format!("{filename}: starting..."));

        Self {
            multi: multi.clone(),
            overall,
            filename: filename.to_string(),
            detail: RefCell::new(None),
            stage_start_time: RefCell::new(None),
        }
    }

    /// Finish this package's progress display with a checkmark.
    fn finish(&self, compressed_size: u64, duration: Duration) {
        self.overall
            .set_style(ProgressStyle::default_spinner().template("{msg}").unwrap());
        self.overall.finish_with_message(format!(
            "  {} ({}, {:.1}s)",
            self.filename,
            format_size(compressed_size),
            duration.as_secs_f64(),
        ));
    }
}

impl BuildProgress for IndicatifProgress {
    fn stage_start(&self, stage: BuildStage, total_items: u64, total_bytes: u64) {
        self.overall
            .set_message(format!("{}: {}...", self.filename, stage.label()));
        *self.stage_start_time.borrow_mut() = Some(Instant::now());

        if total_items > 0 && total_bytes > 0 {
            let bar = self.multi.add(ProgressBar::new(total_bytes));
            bar.set_style(
                ProgressStyle::default_bar()
                    .template("  {bar:40.cyan/blue} {bytes}/{total_bytes} ({eta})")
                    .unwrap()
                    .progress_chars("##-"),
            );
            *self.detail.borrow_mut() = Some(bar);
        }
    }

    fn item_completed(&self, bytes: u64) {
        if let Some(ref bar) = *self.detail.borrow() {
            bar.inc(bytes);
        }
    }

    fn stage_finish(&self, _stage: BuildStage) {
        if let Some(bar) = self.detail.borrow_mut().take() {
            bar.finish_and_clear();
            self.multi.remove(&bar);
        }
        *self.stage_start_time.borrow_mut() = None;
    }

    fn part_completed(&self, part: u32, compressed_size: u64) {
        self.multi
            .println(format!("  Part {part}: {}", format_size(compressed_size)))
            .ok();
    }
}

/// Print a summary table after all packages are built.
fn print_build_summary(results: &[BuildResult], total_duration: Duration) {
    println!();
    println!("Build Summary");
    println!("{}", "-".repeat(72));

    for r in results {
        let ratio = if r.uncompressed_size > 0 {
            format!(
                "{:.1}%",
                (r.compressed_size as f64 / r.uncompressed_size as f64) * 100.0
            )
        } else {
            "n/a".to_string()
        };
        println!(
            "  {} ({} files, {} -> {} [{}], {:.1}s)",
            r.filename,
            format_file_count(r.file_count),
            format_size(r.uncompressed_size),
            format_size(r.compressed_size),
            ratio,
            r.duration.as_secs_f64(),
        );
    }

    println!("{}", "-".repeat(72));
    println!("Total: {:.1}s", total_duration.as_secs_f64());
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
