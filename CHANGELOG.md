# Changelog

## 0.1.0 (unreleased)

### Phase 0: Workspace Scaffolding & Config Parsing

- Created workspace with `spm-core` and `spm-cli` crates
- Implemented YAML config parsing with full schema from spec.md
- Implemented environment variable expansion (`${VAR}` syntax)
- Implemented config validation (arch, compression, splitting strategy)
- Added `spm validate` subcommand
- Added `spm init` subcommand to generate template configs
- Added test fixtures (minimal, full MATLAB example, invalid configs)
