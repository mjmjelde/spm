# Claude.md — Project Instructions for spm

## Project Overview

spm is a Rust CLI tool for building RPM and DEB packages from directory trees, with first-class support for large files (>4 GB), auto-splitting oversized packages, multi-threaded compression, and declarative `update-alternatives` integration.

**Primary use case:** Packaging large vendor software (MATLAB, CUDA, Intel compilers, EDA tools) for enterprise Linux deployment at scale.

## Key Files

- **`spec.md`** — The specification. Format constraints, YAML config schema, RPM/DEB internals, alternatives integration. This is the source of truth for *what* to build. Reference it whenever you need format details, size limits, header layouts, or config field definitions.
- **`IMPLEMENTATION.md`** — The phased build plan. Concrete Rust types, step-by-step instructions, acceptance criteria. This is *how* to build it. Work through phases in order.

## Rules

### General

- **Implement one phase at a time.** Do not skip ahead. Each phase must pass its acceptance criteria before starting the next.
- **Ask before making architectural decisions** that deviate from the spec or implementation guide. If something in the docs is wrong or impractical, say so — don't silently work around it.
- **Use the Rust 2021 edition.** This is a workspace project.
- **Run `cargo clippy` and `cargo fmt`** before considering any phase complete. No warnings allowed.
- **Every public function and type gets a doc comment.** No exceptions. Use `///` style.

### Error Handling

- **Library crates** (`spm-core`, `spm-cpio`, `spm-rpm`, `spm-deb`, `spm-compress`): Use `thiserror` for typed error enums. Every error variant must include enough context to diagnose the problem (file paths, field names, expected vs actual values).
- **CLI binary** (`spm-cli`): Use `anyhow` with `.context()` chains. The user should never see a raw "No such file or directory" — they should see "failed to read script 'scripts/postinst.sh' referenced in scripts.post_install".

### Testing

- **Unit tests live next to the code** they test (`#[cfg(test)] mod tests` in the same file).
- **Integration tests** go in `tests/integration/`. These run actual CLI commands and verify output with `rpm`, `dpkg-deb`, etc.
- **Test fixtures** go in `tests/fixtures/`. YAML configs, small test file trees, test GPG keys.
- **Never skip writing tests.** If the acceptance criteria in `IMPLEMENTATION.md` lists a test, implement it. Tests are not optional.

### Documentation

- **Document each phase after completion.** When a phase passes its acceptance criteria, write a summary in `docs/phases/phase-N.md` that includes:
  - What was implemented
  - Any deviations from the plan and why
  - Key design decisions made during implementation
  - Known limitations or technical debt introduced
  - Instructions for testing what was built (commands to run)
- **Keep a running `docs/ARCHITECTURE.md`** that stays current with the actual code structure. Update it at the end of each phase. Include the workspace layout, crate dependency graph, and key trait/type relationships.
- **Keep a `CHANGELOG.md`** in the repo root. Add entries as features are completed, not retroactively.
- **README.md** should be created in Phase 0 and kept updated. It should include: what spm is, how to build it, basic usage example, and current status (which phases are complete).

### Code Style

- Prefer explicit types over `impl Trait` in public APIs — makes the docs clearer.
- Use `PathBuf` for owned paths, `&Path` for borrowed. Never use `String` for file paths.
- Streaming I/O everywhere for payload handling — see the streaming pattern in `IMPLEMENTATION.md` General Notes. Never buffer a full payload in memory.
- Sort `use` statements: std first, then external crates, then internal crates.
- Keep functions under ~80 lines. If a function is getting long, extract helpers with descriptive names.

### Dependencies

- Be conservative. Every dependency is a supply chain risk. Prefer well-known, actively maintained crates.
- Pin major versions in `Cargo.toml` (e.g., `serde = "1"`, not `serde = "*"`).
- If you need to choose between a pure-Rust implementation and a C binding, prefer pure Rust unless there's a significant performance reason (zstd and xz are acceptable as C bindings since the compression libraries are battle-tested).

### Commit Hygiene (for when this is in version control)

- One logical change per commit.
- Commit messages should reference the phase: `phase-0: implement YAML config parsing and validation`
- Tag phases when complete: `phase-0-complete`, `phase-1-complete`, etc.

## Project Structure

```
spm/
├── Claude.md                       # This file
├── spec.md                         # Specification
├── IMPLEMENTATION.md               # Phased build plan
├── CHANGELOG.md                    # Running changelog
├── README.md                       # Project readme
├── Cargo.toml                      # Workspace root
├── crates/
│   ├── spm-cli/               # Binary crate
│   ├── spm-core/              # Config, planning, shared types
│   ├── spm-compress/          # Compression abstraction
│   ├── spm-cpio/              # CPIO archive writer (070701 + 07070X)
│   ├── spm-rpm/               # RPM format backend
│   └── spm-deb/               # DEB format backend
├── docs/
│   ├── ARCHITECTURE.md             # Living architecture document
│   └── phases/
│       ├── phase-0.md              # Written after Phase 0 complete
│       ├── phase-1.md              # Written after Phase 1 complete
│       └── ...
└── tests/
    ├── fixtures/
    │   ├── minimal.yaml
    │   ├── full.yaml
    │   ├── large.yaml
    │   └── invalid/
    └── integration/
```

## Phase Workflow

For each phase:

1. Read the phase in `IMPLEMENTATION.md`
2. Reference `spec.md` for any format/config details
3. Implement the steps in order
4. Run `cargo fmt && cargo clippy` — fix all warnings
5. Run all tests — unit and integration
6. Verify acceptance criteria from `IMPLEMENTATION.md`
7. Write `docs/phases/phase-N.md`
8. Update `docs/ARCHITECTURE.md` if crate structure or key types changed
9. Update `CHANGELOG.md`
10. Update `README.md` status section