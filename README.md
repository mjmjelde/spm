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
```

## Current Status

- [x] Phase 0: Workspace scaffolding, config parsing, validation, CLI (`validate`, `init`)
- [ ] Phase 1: File tree walking & package planning
- [ ] Phase 2: Compression engine
- [ ] Phase 3: CPIO writer & RPM backend
- [ ] Phase 4: DEB backend & auto-split
- [ ] Phase 5: Full CLI, distro compat, polish
- [ ] Phase 6: Signing
