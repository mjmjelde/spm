# spm — Large-File-Aware Linux Package Builder

A Rust CLI tool for building RPM and DEB packages from directory trees, with first-class support for large files (>4 GB), auto-splitting oversized packages, multi-threaded compression, and declarative `update-alternatives` integration.

**Primary use case:** Packaging large vendor software (MATLAB, CUDA, Intel compilers, EDA tools) for enterprise Linux deployment at scale.

## Building

```bash
cargo build --release
```

The binary is produced at `target/release/spm`.

## Usage

```bash
# Create a template config
spm init --name myapp --version 1.0.0

# Validate a config file
spm validate --config spm.yaml

# Dry-run: show what would be built
spm plan --config spm.yaml --format rpm
spm plan --config spm.yaml --format deb
```

## Current Status

- [x] Phase 0: Workspace scaffolding, config parsing, validation, CLI (`validate`, `init`)
- [x] Phase 1: File tree walking & package planning (`plan`)
- [x] Phase 2: Compression engine (zstd multi-threaded, gzip, passthrough)
- [x] Phase 3: CPIO writer & RPM backend
- [x] Phase 4: DEB backend & auto-split
- [x] Phase 5a: XZ compression, distro compat DB, RPM builder gaps, config validation
- [x] Phase 5b: CLI integration (`build`, `plan`, `inspect`), RPM/DEB readers, spinners
- [ ] Phase 6: Signing
