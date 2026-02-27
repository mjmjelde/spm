# Architecture

## Workspace Layout

```
spm/
├── Cargo.toml                  # Workspace root
├── crates/
│   ├── spm-cli/                # Binary crate — CLI frontend
│   │   └── src/main.rs         # clap CLI with validate/init subcommands
│   └── spm-core/               # Config parsing, planning, shared types
│       └── src/
│           ├── lib.rs           # Re-exports modules
│           ├── config.rs        # YAML deserialization & validation
│           ├── error.rs         # Error types (thiserror)
│           └── types.rs         # Shared types (placeholder)
└── tests/
    └── fixtures/               # Test YAML configs
```

## Crate Dependency Graph

```
spm-cli ──► spm-core
```

## Key Types

### spm-core

- `Config` — Top-level config struct, deserializable from YAML. Entry point: `Config::load(path)`.
- `PackageConfig` — Package identity (name, version, arch, etc.).
- `ContentConfig` — File mappings, symlinks, directories, alternatives.
- `CompressionConfig` — Algorithm, level, thread count.
- `SplittingConfig` — Auto-split strategy and parameters.
- `ConfigError` — Typed error enum for config loading/validation failures.

### spm-cli

- `Cli` / `Commands` — clap-derived CLI structure with `validate` and `init` subcommands.
