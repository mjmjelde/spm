//! Dependency string validation for RPM and DEB formats.

use crate::config::DependencyConfig;

/// Target package format for dependency validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepFormat {
    Rpm,
    Deb,
}

/// Valid RPM operators (space-separated: `name OP version`).
const RPM_OPS: &[&str] = &[">=", "<=", ">", "<", "="];

/// Valid DEB operators (parenthesized: `name (OP version)`).
const DEB_OPS: &[&str] = &[">=", "<=", ">>", "<<", "="];

/// Check whether `name` contains only valid package-name characters.
fn valid_pkg_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let first = name.as_bytes()[0];
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_' || b == b'+')
}

/// Validate a single dependency string for the given format.
///
/// Returns `Ok(())` if valid, or `Err` with a human-readable message.
pub fn validate_dep(dep: &str, format: DepFormat) -> Result<(), String> {
    let dep = dep.trim();
    if dep.is_empty() {
        return Err("dependency string is empty".to_string());
    }

    match format {
        DepFormat::Rpm => validate_rpm_dep(dep),
        DepFormat::Deb => validate_deb_dep(dep),
    }
}

fn validate_rpm_dep(dep: &str) -> Result<(), String> {
    // Reject DEB-style parenthesized constraints.
    if dep.contains('(') || dep.contains(')') {
        // Try to extract a suggestion.
        if let Some(suggestion) = rpm_suggestion_from_deb(dep) {
            return Err(format!(
                "RPM deps must not use parenthesized version constraints; use '{suggestion}' instead"
            ));
        }
        return Err("RPM deps must not use parenthesized version constraints".to_string());
    }

    let parts: Vec<&str> = dep.split_whitespace().collect();
    match parts.len() {
        1 => {
            if !valid_pkg_name(parts[0]) {
                return Err(format!("invalid package name '{}'", parts[0]));
            }
            Ok(())
        }
        3 => {
            if !valid_pkg_name(parts[0]) {
                return Err(format!("invalid package name '{}'", parts[0]));
            }
            if !RPM_OPS.contains(&parts[1]) {
                return Err(format!(
                    "invalid RPM operator '{}'; expected one of: {}",
                    parts[1],
                    RPM_OPS.join(", ")
                ));
            }
            if parts[2].is_empty() {
                return Err("missing version after operator".to_string());
            }
            Ok(())
        }
        2 => {
            // e.g. "libfoo >=" — missing version
            if RPM_OPS.contains(&parts[1]) {
                return Err(format!("missing version after operator '{}'", parts[1]));
            }
            Err(format!(
                "invalid dependency format; expected 'name' or 'name OP version', got '{dep}'"
            ))
        }
        _ => Err(format!(
            "invalid dependency format; expected 'name' or 'name OP version', got '{dep}'"
        )),
    }
}

/// Try to convert a DEB-style dep to an RPM suggestion.
fn rpm_suggestion_from_deb(dep: &str) -> Option<String> {
    // Parse "name (OP ver)"
    let paren_start = dep.find('(')?;
    let paren_end = dep.find(')')?;
    let name = dep[..paren_start].trim();
    let inner = dep[paren_start + 1..paren_end].trim();
    let parts: Vec<&str> = inner.split_whitespace().collect();
    if parts.len() == 2 {
        let op = match parts[0] {
            ">>" => ">",
            "<<" => "<",
            other => other,
        };
        Some(format!("{name} {op} {}", parts[1]))
    } else {
        None
    }
}

fn validate_deb_dep(dep: &str) -> Result<(), String> {
    if let Some(paren_start) = dep.find('(') {
        // Versioned dep — must have matching closing paren.
        let name = dep[..paren_start].trim();
        if !valid_pkg_name(name) {
            return Err(format!("invalid package name '{name}'"));
        }

        let paren_end = dep
            .find(')')
            .ok_or_else(|| "missing closing ')' in version constraint".to_string())?;

        let inner = dep[paren_start + 1..paren_end].trim();
        let parts: Vec<&str> = inner.split_whitespace().collect();
        if parts.len() != 2 {
            return Err(format!(
                "invalid version constraint '({inner})'; expected '(OP version)'"
            ));
        }
        if !DEB_OPS.contains(&parts[0]) {
            return Err(format!(
                "invalid DEB operator '{}'; expected one of: {}",
                parts[0],
                DEB_OPS.join(", ")
            ));
        }
        if parts[1].is_empty() {
            return Err("missing version after operator".to_string());
        }
        Ok(())
    } else {
        // Unversioned dep — check for bare operators (common mistake).
        let parts: Vec<&str> = dep.split_whitespace().collect();
        if parts.len() >= 3 {
            let maybe_op = parts[1];
            if RPM_OPS.contains(&maybe_op) || DEB_OPS.contains(&maybe_op) {
                return Err(format!(
                    "DEB deps must use parenthesized version constraints; \
                     use '{} ({} {})' instead",
                    parts[0],
                    maybe_op,
                    parts[2..].join(" ")
                ));
            }
        }
        if parts.len() == 2 && (RPM_OPS.contains(&parts[1]) || DEB_OPS.contains(&parts[1])) {
            return Err(format!(
                "missing version after operator '{}'; \
                 expected 'name (OP version)'",
                parts[1]
            ));
        }
        if parts.len() != 1 {
            return Err(format!(
                "invalid dependency format; expected 'name' or 'name (OP version)', got '{dep}'"
            ));
        }
        if !valid_pkg_name(parts[0]) {
            return Err(format!("invalid package name '{}'", parts[0]));
        }
        Ok(())
    }
}

/// Validate all dependency fields in a [`DependencyConfig`] for the given format.
///
/// Returns a list of human-readable error strings. Empty means all valid.
pub fn validate_all_deps(deps: &DependencyConfig, format: DepFormat) -> Vec<String> {
    let mut errors = Vec::new();

    let check = |field: &str, list: &[String], fmt: DepFormat, errors: &mut Vec<String>| {
        for (i, dep) in list.iter().enumerate() {
            if let Err(msg) = validate_dep(dep, fmt) {
                errors.push(format!("dependencies.{field}[{i}] '{dep}': {msg}"));
            }
        }
    };

    // Shared fields — validated against the target format.
    check("requires", &deps.requires, format, &mut errors);
    check("conflicts", &deps.conflicts, format, &mut errors);
    check("provides", &deps.provides, format, &mut errors);
    check("replaces", &deps.replaces, format, &mut errors);

    // Format-specific fields — only validated for their own format.
    match format {
        DepFormat::Rpm => check(
            "requires_rpm",
            &deps.requires_rpm,
            DepFormat::Rpm,
            &mut errors,
        ),
        DepFormat::Deb => check(
            "requires_deb",
            &deps.requires_deb,
            DepFormat::Deb,
            &mut errors,
        ),
    }

    errors
}

/// Validate deps leniently (for `spm validate` without a target format).
///
/// Format-specific fields are checked against their format.
/// Shared fields pass if valid for either RPM or DEB.
pub fn validate_all_deps_lenient(deps: &DependencyConfig) -> Vec<String> {
    let mut errors = Vec::new();

    let check_lenient = |field: &str, list: &[String], errors: &mut Vec<String>| {
        for (i, dep) in list.iter().enumerate() {
            let rpm_ok = validate_dep(dep, DepFormat::Rpm).is_ok();
            let deb_ok = validate_dep(dep, DepFormat::Deb).is_ok();
            if !rpm_ok && !deb_ok {
                // Show the RPM error since it's the simpler format.
                let msg = validate_dep(dep, DepFormat::Rpm).unwrap_err();
                errors.push(format!("dependencies.{field}[{i}] '{dep}': {msg}"));
            }
        }
    };

    let check_strict = |field: &str, list: &[String], fmt: DepFormat, errors: &mut Vec<String>| {
        for (i, dep) in list.iter().enumerate() {
            if let Err(msg) = validate_dep(dep, fmt) {
                errors.push(format!("dependencies.{field}[{i}] '{dep}': {msg}"));
            }
        }
    };

    check_lenient("requires", &deps.requires, &mut errors);
    check_lenient("conflicts", &deps.conflicts, &mut errors);
    check_lenient("provides", &deps.provides, &mut errors);
    check_lenient("replaces", &deps.replaces, &mut errors);

    check_strict(
        "requires_rpm",
        &deps.requires_rpm,
        DepFormat::Rpm,
        &mut errors,
    );
    check_strict(
        "requires_deb",
        &deps.requires_deb,
        DepFormat::Deb,
        &mut errors,
    );

    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── RPM valid ──────────────────────────────────────────────

    #[test]
    fn rpm_unversioned() {
        assert!(validate_dep("libfoo", DepFormat::Rpm).is_ok());
    }

    #[test]
    fn rpm_greater_equal() {
        assert!(validate_dep("libfoo >= 1.0", DepFormat::Rpm).is_ok());
    }

    #[test]
    fn rpm_equal() {
        assert!(validate_dep("libfoo = 1.0", DepFormat::Rpm).is_ok());
    }

    #[test]
    fn rpm_less_equal() {
        assert!(validate_dep("libfoo <= 3", DepFormat::Rpm).is_ok());
    }

    #[test]
    fn rpm_greater() {
        assert!(validate_dep("libfoo > 1", DepFormat::Rpm).is_ok());
    }

    #[test]
    fn rpm_less() {
        assert!(validate_dep("libfoo < 2", DepFormat::Rpm).is_ok());
    }

    #[test]
    fn rpm_name_with_dots_hyphens() {
        assert!(validate_dep("lib-foo.bar_baz+1 >= 2.0", DepFormat::Rpm).is_ok());
    }

    // ── RPM invalid ────────────────────────────────────────────

    #[test]
    fn rpm_rejects_deb_style() {
        let err = validate_dep("libfoo (>= 1.0)", DepFormat::Rpm).unwrap_err();
        assert!(err.contains("parenthesized"), "got: {err}");
        assert!(
            err.contains("libfoo >= 1.0"),
            "should suggest fix, got: {err}"
        );
    }

    #[test]
    fn rpm_rejects_empty() {
        assert!(validate_dep("", DepFormat::Rpm).is_err());
    }

    #[test]
    fn rpm_rejects_missing_version() {
        let err = validate_dep("libfoo >=", DepFormat::Rpm).unwrap_err();
        assert!(err.contains("missing version"), "got: {err}");
    }

    #[test]
    fn rpm_rejects_bad_operator() {
        assert!(validate_dep("libfoo >> 1.0", DepFormat::Rpm).is_err());
    }

    #[test]
    fn rpm_rejects_bad_name() {
        assert!(validate_dep("-libfoo", DepFormat::Rpm).is_err());
    }

    // ── DEB valid ──────────────────────────────────────────────

    #[test]
    fn deb_unversioned() {
        assert!(validate_dep("libfoo", DepFormat::Deb).is_ok());
    }

    #[test]
    fn deb_greater_equal() {
        assert!(validate_dep("libfoo (>= 1.0)", DepFormat::Deb).is_ok());
    }

    #[test]
    fn deb_equal() {
        assert!(validate_dep("libfoo (= 1.0)", DepFormat::Deb).is_ok());
    }

    #[test]
    fn deb_less_equal() {
        assert!(validate_dep("libfoo (<= 3)", DepFormat::Deb).is_ok());
    }

    #[test]
    fn deb_strict_greater() {
        assert!(validate_dep("libfoo (>> 1)", DepFormat::Deb).is_ok());
    }

    #[test]
    fn deb_strict_less() {
        assert!(validate_dep("libfoo (<< 2)", DepFormat::Deb).is_ok());
    }

    // ── DEB invalid ────────────────────────────────────────────

    #[test]
    fn deb_rejects_bare_operator() {
        let err = validate_dep("libfoo >= 1.0", DepFormat::Deb).unwrap_err();
        assert!(err.contains("parenthesized"), "got: {err}");
        assert!(
            err.contains("libfoo (>= 1.0)"),
            "should suggest fix, got: {err}"
        );
    }

    #[test]
    fn deb_rejects_empty() {
        assert!(validate_dep("", DepFormat::Deb).is_err());
    }

    #[test]
    fn deb_rejects_missing_version() {
        assert!(validate_dep("libfoo (>= )", DepFormat::Deb).is_err());
    }

    #[test]
    fn deb_rejects_bad_operator() {
        assert!(validate_dep("libfoo (> 1.0)", DepFormat::Deb).is_err());
    }

    #[test]
    fn deb_rejects_missing_close_paren() {
        assert!(validate_dep("libfoo (>= 1.0", DepFormat::Deb).is_err());
    }

    // ── validate_all_deps ──────────────────────────────────────

    #[test]
    fn skips_requires_deb_for_rpm() {
        let deps = DependencyConfig {
            requires: vec!["libfoo >= 1.0".to_string()],
            requires_rpm: vec![],
            requires_deb: vec!["libfoo (>= 1.0)".to_string()], // DEB syntax, should be skipped
            conflicts: vec![],
            provides: vec![],
            replaces: vec![],
        };
        let errors = validate_all_deps(&deps, DepFormat::Rpm);
        assert!(errors.is_empty(), "got: {errors:?}");
    }

    #[test]
    fn skips_requires_rpm_for_deb() {
        let deps = DependencyConfig {
            requires: vec!["libfoo (>= 1.0)".to_string()],
            requires_rpm: vec!["libfoo >= 1.0".to_string()], // RPM syntax, should be skipped
            requires_deb: vec![],
            conflicts: vec![],
            provides: vec![],
            replaces: vec![],
        };
        let errors = validate_all_deps(&deps, DepFormat::Deb);
        assert!(errors.is_empty(), "got: {errors:?}");
    }

    #[test]
    fn catches_wrong_format_in_requires() {
        let deps = DependencyConfig {
            requires: vec!["libfoo >= 1.0".to_string()], // RPM syntax
            requires_rpm: vec![],
            requires_deb: vec![],
            conflicts: vec![],
            provides: vec![],
            replaces: vec![],
        };
        let errors = validate_all_deps(&deps, DepFormat::Deb);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("parenthesized"), "got: {}", errors[0]);
    }

    #[test]
    fn validates_conflicts_provides_replaces() {
        let deps = DependencyConfig {
            requires: vec![],
            requires_rpm: vec![],
            requires_deb: vec![],
            conflicts: vec!["bad (>= 1)".to_string()],
            provides: vec!["ok = 1.0".to_string()],
            replaces: vec!["".to_string()],
        };
        let errors = validate_all_deps(&deps, DepFormat::Rpm);
        assert_eq!(errors.len(), 2, "got: {errors:?}"); // conflicts + replaces
    }

    // ── validate_all_deps_lenient ──────────────────────────────

    #[test]
    fn lenient_accepts_either_format() {
        let deps = DependencyConfig {
            requires: vec![
                "libfoo >= 1.0".to_string(),   // RPM style — ok
                "libbar (>= 2.0)".to_string(), // DEB style — ok
                "libbaz".to_string(),          // unversioned — ok
            ],
            requires_rpm: vec![],
            requires_deb: vec![],
            conflicts: vec![],
            provides: vec![],
            replaces: vec![],
        };
        let errors = validate_all_deps_lenient(&deps);
        assert!(errors.is_empty(), "got: {errors:?}");
    }

    #[test]
    fn lenient_rejects_garbage() {
        let deps = DependencyConfig {
            requires: vec!["".to_string()],
            requires_rpm: vec![],
            requires_deb: vec![],
            conflicts: vec![],
            provides: vec![],
            replaces: vec![],
        };
        let errors = validate_all_deps_lenient(&deps);
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn lenient_validates_format_specific_strictly() {
        let deps = DependencyConfig {
            requires: vec![],
            requires_rpm: vec!["libfoo (>= 1.0)".to_string()], // wrong format for RPM
            requires_deb: vec!["libbar >= 1.0".to_string()],   // wrong format for DEB
            conflicts: vec![],
            provides: vec![],
            replaces: vec![],
        };
        let errors = validate_all_deps_lenient(&deps);
        assert_eq!(errors.len(), 2, "got: {errors:?}");
    }
}
