# spm — Simple Package Manager

**Large-file-aware Linux package builder for RPM and DEB**

[![CI](https://github.com/mjmjelde/spm/actions/workflows/ci.yml/badge.svg)](https://github.com/mjmjelde/spm/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

spm (Simple Package Manager) is a Rust CLI tool that builds RPM and DEB packages from a declarative YAML config and a directory tree. Designed for teams packaging large vendor software (MATLAB, CUDA, Intel compilers, EDA tools) for enterprise Linux, where installations routinely reach 20-50+ GiB and break existing packaging tools.

spm is pure Rust with no runtime dependency on rpmbuild, dpkg-deb, or system packaging tools.

## Why spm?

Vendor software installations hit hard limits in both RPM and DEB package formats that existing tools handle poorly or not at all:

| Format | Limit | Root Cause |
|--------|-------|------------|
| RPM (standard CPIO) | 4 GiB per file | 8-digit hex `c_filesize` in SVR4 newc (070701) header |
| DEB | ~9.3 GiB per ar member | 10-digit ASCII decimal size field in ar headers |
| DEB | ~8 GiB per tar entry | 11-digit ASCII octal size field in ustar tar |

Tools like fpm and nfpm either silently produce corrupt packages or fail outright when these limits are hit. spm was built to handle these cases correctly.

| | spm | fpm | nfpm | rpmbuild |
|-|-----|-----|------|----------|
| Files > 4 GiB (RPM) | Extended CPIO (07070X) | Not handled | Not handled | Supported (4.12+) |
| Packages > 9.3 GiB (DEB) | Auto-split | Not handled | Not handled | N/A |
| Streaming I/O | Yes | No | No | Yes |
| Multi-threaded compression | zstd, xz | No | No | No |
| Pure implementation | Rust | Ruby + system tools | Go + system tools | C + system tools |
| Config format | YAML | CLI flags | YAML | specfile |
| update-alternatives | Declarative YAML | Manual scripts | Manual scripts | Manual scripts |

## Features

- **RPM v4 and DEB 2.0** package formats from a single YAML config
- **Large file support** via RPM extended CPIO format (07070X) for files exceeding 4 GiB
- **Auto-splitting** when packages exceed format limits, with a meta-package that depends on all parts
- **Multi-threaded compression** with zstd (default), xz, gzip, or no compression
- **Declarative update-alternatives** with follower support, auto-generated scriptlets
- **Distro compatibility database** for EL 8/9, Fedora, Ubuntu 20.04/22.04/24.04 with warnings about unsupported features
- **Reproducible builds** via `SOURCE_DATE_EPOCH`
- **Streaming I/O** throughout the pipeline, constant memory usage regardless of package size
- **Package inspection** to read metadata from existing .rpm and .deb files
- **Pure Rust** with no runtime dependency on rpmbuild, dpkg-deb, or cpio

## Installation

### From GitHub Releases

Download the latest binary, RPM, or DEB from the [Releases](https://github.com/mjmjelde/spm/releases) page:

```bash
# Binary
curl -LO https://github.com/mjmjelde/spm/releases/latest/download/spm-0.1.0-x86_64-linux
chmod +x spm-0.1.0-x86_64-linux
sudo mv spm-0.1.0-x86_64-linux /usr/local/bin/spm

# Or install from the release RPM/DEB
sudo rpm -i spm-0.1.0-1.x86_64.rpm      # RPM-based systems
sudo dpkg -i spm_0.1.0-1_amd64.deb      # DEB-based systems
```

### From Source

Requires the Rust stable toolchain (2021 edition).

```bash
git clone https://github.com/mjmjelde/spm.git
cd spm
cargo build --release
# Binary at target/release/spm
```

## Quick Start

```bash
# 1. Create a template config
spm init --name myapp --version 2.0.0

# 2. Edit spm.yaml to configure your file mappings, dependencies, etc.

# 3. Validate the config
spm validate --config spm.yaml

# 4. Preview what would be built (dry run)
spm plan --config spm.yaml --format all

# 5. Build RPM and DEB packages
spm build --config spm.yaml --format all -o ./dist/

# 6. Inspect the results
spm inspect dist/myapp-2.0.0-1.x86_64.rpm
spm inspect dist/myapp_2.0.0-1_amd64.deb
```

## CLI Reference

### Global Flags

| Flag | Default | Description |
|------|---------|-------------|
| `-c, --config <path>` | `spm.yaml` | Config file path |
| `-q, --quiet` | | Suppress non-error output |

### `spm init`

Create a commented template `spm.yaml` in the current directory.

```bash
spm init --name myapp --version 1.0.0
```

### `spm validate`

Validate configuration without building. Checks YAML schema, field constraints, and that script files exist.

```bash
spm validate --config spm.yaml
```

### `spm plan`

Dry run. Shows package sizes, compression estimates, split plan, minimum tool versions, and distro compatibility warnings.

```bash
spm plan --config spm.yaml --format rpm
spm plan --config spm.yaml --format deb --target-distro ubuntu2404
```

### `spm build`

Build packages. Shows per-package progress spinners and a build summary with compression ratios.

```bash
spm build --config spm.yaml --format all -o ./dist/
spm build --config spm.yaml --format rpm --compression xz --threads 4
```

### `spm inspect`

Read and display metadata from an existing package file. Auto-detects format by file extension.

```bash
spm inspect package.rpm
spm inspect package.deb
```

### Build/Plan Flags

These flags are shared by both `plan` and `build`:

| Flag | Description |
|------|-------------|
| `-f, --format <fmt>` | Output format: `rpm`, `deb`, or `all` (default: `all`) |
| `-o, --output <dir>` | Output directory (default: `./out`, `build` only) |
| `--no-split` | Disable package splitting |
| `--compression <alg>` | Override compression: `zstd`, `gzip`, `xz`, `none` |
| `--compression-level <n>` | Override compression level |
| `--threads <n>` | Override thread count (0 = auto) |
| `--source-date-epoch <ts>` | Fixed timestamp for reproducible builds |
| `--target-distro <distro>` | Target distribution for compatibility warnings |

Target distros: `el8`, `el9`, `fedora`, `ubuntu2004`, `ubuntu2204`, `ubuntu2404`.

## Configuration

spm uses a single YAML file to define package metadata, file mappings, compression, splitting, and format-specific settings.

```yaml
package:     # Name, version, arch, license, dependencies
content:     # File mappings, symlinks, directories, alternatives
scripts:     # Pre/post install/remove hook scripts
compression: # Algorithm, level, threads
splitting:   # Auto-split strategy and thresholds
signing:     # PGP key (not yet implemented)
rpm:         # RPM-specific overrides
deb:         # DEB-specific overrides
build:       # Reproducibility settings (source_date_epoch)
```

For the complete schema reference with all fields, types, defaults, and validation rules, see [docs/CONFIGURATION.md](docs/CONFIGURATION.md).

## Examples

### Minimal

```yaml
package:
  name: myapp
  version: "1.0.0"
  arch: x86_64
  license: MIT
  maintainer: "Your Name <you@example.com>"
  description: "My application"

content:
  files:
    - src: "/path/to/staged/files/**"
      dst: /opt/myapp/
```

### Real-World: Large Vendor Software

```yaml
package:
  name: matlab
  version: "2025a"
  release: "1"
  arch: x86_64
  license: Proprietary
  maintainer: "HPC Team <hpc-help@tamu.edu>"
  description: "MATLAB R2025a - Technical Computing Environment"
  url: https://www.mathworks.com/products/matlab.html
  vendor: MathWorks

  dependencies:
    requires:
      - libX11
      - libXext
      - "libasound2 >= 1.1"
    requires_rpm:
      - mesa-libGL
    requires_deb:
      - libgl1-mesa-glx

content:
  defaults:
    user: root
    group: root
    file_mode: "0644"
    dir_mode: "0755"

  files:
    - src: "/opt/matlab-staging/R2025a/**"
      dst: /opt/matlab/R2025a/
    - src: matlab.sh
      dst: /etc/profile.d/matlab-2025a.sh
      type: config

  alternatives:
    - name: matlab
      link: /usr/bin/matlab
      path: /opt/matlab/R2025a/bin/matlab
      priority: 2025
      followers:
        - name: mex
          link: /usr/bin/mex
          path: /opt/matlab/R2025a/bin/mex

compression:
  algorithm: zstd
  level: 19
  threads: 0

splitting:
  enabled: true
  strategy: auto
```

## Architecture

spm is organized as a Rust workspace with 6 crates:

```
spm-cli ──> spm-core
        ├─> spm-compress
        ├─> spm-rpm ──> spm-core, spm-cpio, spm-compress
        └─> spm-deb ──> spm-core, spm-compress
```

| Crate | Description |
|-------|-------------|
| `spm-cli` | Binary crate. CLI frontend with clap, progress spinners. |
| `spm-core` | Config parsing, file tree walking, package planning, split strategies, distro compat. |
| `spm-compress` | Streaming compression/decompression for zstd, gzip, xz. |
| `spm-cpio` | CPIO archive writer supporting SVR4 newc (070701) and RPM extended (07070X) formats. |
| `spm-rpm` | RPM v4 package builder and metadata reader. |
| `spm-deb` | DEB package builder and metadata reader. |

For detailed type documentation and design decisions, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Supported Platforms

spm runs on any Linux x86_64 host. The `--target-distro` flag checks compatibility with the target distribution's packaging tools.

| Format | Tested Distributions | Minimum Tool Version |
|--------|---------------------|---------------------|
| RPM | EL 8, EL 9, Fedora | rpm 4.6.0+ (4.14.0+ for zstd) |
| DEB | Ubuntu 20.04, 22.04, 24.04 | dpkg 1.0.0+ (1.21.18+ for zstd) |

## Contributing

```bash
cargo test --workspace       # 244 tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Issues and pull requests are welcome at [github.com/mjmjelde/spm](https://github.com/mjmjelde/spm).

## License

[MIT](LICENSE)
