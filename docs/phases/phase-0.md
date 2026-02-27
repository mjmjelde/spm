# Phase 0: Workspace Scaffolding & Config Parsing

## What Was Implemented

- Rust workspace with two crates: `spm-core` (library) and `spm-cli` (binary)
- Full YAML config schema matching spec.md Section 3, including:
  - Package metadata, dependencies, content mappings, alternatives
  - Scripts, compression, splitting, signing, RPM/DEB overrides, build settings
- Environment variable expansion via `shellexpand` (`${VAR}` syntax)
- Config validation: required fields, allowed architectures, compression algorithms, split strategies
- CLI with two subcommands:
  - `spm validate` — loads and validates a config, prints summary
  - `spm init` — generates a template `spm.yaml`
- Test fixtures: minimal config, full MATLAB example, invalid variants
- Unit tests for parsing, validation, env-var expansion, default values

## Design Decisions

- **`shellexpand::env()` operates on the raw YAML string before parsing.** This means `${VAR}` is expanded in all string values, which matches the spec's intent for `signing.key_file` and `build.source_date_epoch`.
- **Validation is post-deserialization.** serde handles structural validation (required fields, types), then `Config::validate()` checks semantic constraints.
- **`thiserror` in spm-core, `anyhow` in spm-cli.** Library errors are typed; CLI wraps them with `.context()` chains for user-friendly messages.

## Known Limitations

- `Config::load()` does not check that `source_dir` exists or that script files exist. This is deferred to Phase 1 (file tree walking) and Phase 5 (validation improvements).
- No `--color` CLI flag yet (Phase 5).

## Testing

```bash
# Run unit tests
cargo test -p spm-core

# Validate the full MATLAB example
SPM_SIGNING_KEY=/tmp/key.gpg cargo run -p spm-cli -- validate --config tests/fixtures/full.yaml

# Reject invalid configs
cargo run -p spm-cli -- validate --config tests/fixtures/invalid/missing_name.yaml

# Generate a template
cargo run -p spm-cli -- init --name myapp --version 1.0.0
```
