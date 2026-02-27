/// Alternatives scriptlet generation for `update-alternatives`.
///
/// Generates shell scriptlets that register/deregister alternatives
/// entries during package install/remove. See spec.md Section 7.
use std::path::Path;

use crate::config::{AlternativeConfig, ScriptsConfig};
use crate::error::PlanError;

/// Resolved script contents with alternatives scriptlets injected.
#[derive(Debug, Default, Clone)]
pub struct ResolvedScripts {
    pub pre_install: Option<String>,
    pub post_install: Option<String>,
    pub pre_remove: Option<String>,
    pub post_remove: Option<String>,
    /// RPM only.
    pub pre_trans: Option<String>,
    /// RPM only.
    pub post_trans: Option<String>,
}

/// Generate the `update-alternatives --install` scriptlet.
///
/// Returns `None` if `alternatives` is empty.
pub fn generate_install_scriptlet(alternatives: &[AlternativeConfig]) -> Option<String> {
    if alternatives.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    lines.push("# [spm:alternatives] Auto-generated — do not edit".to_string());

    for alt in alternatives {
        let mut cmd = format!(
            "update-alternatives \\\n  --install {} {} {} {}",
            alt.link, alt.name, alt.path, alt.priority
        );
        for follower in &alt.followers {
            cmd.push_str(&format!(
                " \\\n  --slave {} {} {}",
                follower.link, follower.name, follower.path
            ));
        }
        lines.push(cmd);
    }

    Some(lines.join("\n"))
}

/// Generate the `update-alternatives --remove` scriptlet with `$1` guard.
///
/// The guard checks `$1 = "0"` (RPM full removal) or `$1 = "remove"` (DEB removal)
/// to avoid deregistering alternatives during upgrades.
///
/// Returns `None` if `alternatives` is empty.
pub fn generate_remove_scriptlet(alternatives: &[AlternativeConfig]) -> Option<String> {
    if alternatives.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    lines.push("# [spm:alternatives] Auto-generated — do not edit".to_string());
    lines.push("if [ \"$1\" = \"0\" ] || [ \"$1\" = \"remove\" ]; then".to_string());

    for alt in alternatives {
        lines.push(format!(
            "  update-alternatives --remove {} {}",
            alt.name, alt.path
        ));
    }

    lines.push("fi".to_string());

    Some(lines.join("\n"))
}

/// Load script files from disk and inject alternatives scriptlets.
///
/// Script paths in `scripts_config` are resolved relative to `config_dir`.
/// Alternatives scriptlets are:
/// - **post_install**: alternatives install scriptlet BEFORE user's script
/// - **pre_remove**: alternatives remove scriptlet AFTER user's script
pub fn resolve_scripts(
    scripts_config: &ScriptsConfig,
    alternatives: &[AlternativeConfig],
    config_dir: &Path,
) -> Result<ResolvedScripts, PlanError> {
    let load_script = |path_opt: &Option<std::path::PathBuf>| -> Result<Option<String>, PlanError> {
        match path_opt {
            Some(path) => {
                let resolved = if path.is_absolute() {
                    path.clone()
                } else {
                    config_dir.join(path)
                };
                let content =
                    std::fs::read_to_string(&resolved).map_err(|e| PlanError::ScriptRead {
                        path: resolved,
                        source: e,
                    })?;
                Ok(Some(content))
            }
            None => Ok(None),
        }
    };

    let user_pre_install = load_script(&scripts_config.pre_install)?;
    let user_post_install = load_script(&scripts_config.post_install)?;
    let user_pre_remove = load_script(&scripts_config.pre_remove)?;
    let user_post_remove = load_script(&scripts_config.post_remove)?;
    let user_pre_trans = load_script(&scripts_config.pre_trans)?;
    let user_post_trans = load_script(&scripts_config.post_trans)?;

    let alt_install = generate_install_scriptlet(alternatives);
    let alt_remove = generate_remove_scriptlet(alternatives);

    // Post-install: alternatives BEFORE user script.
    let post_install = match (alt_install, user_post_install) {
        (Some(alt), Some(user)) => Some(format!("{alt}\n{user}")),
        (Some(alt), None) => Some(alt),
        (None, Some(user)) => Some(user),
        (None, None) => None,
    };

    // Pre-remove: user script BEFORE alternatives remove.
    let pre_remove = match (user_pre_remove, alt_remove) {
        (Some(user), Some(alt)) => Some(format!("{user}\n{alt}")),
        (None, Some(alt)) => Some(alt),
        (Some(user), None) => Some(user),
        (None, None) => None,
    };

    Ok(ResolvedScripts {
        pre_install: user_pre_install,
        post_install,
        pre_remove,
        post_remove: user_post_remove,
        pre_trans: user_pre_trans,
        post_trans: user_post_trans,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AlternativeFollower;
    use tempfile::TempDir;

    fn matlab_alt() -> AlternativeConfig {
        AlternativeConfig {
            name: "matlab".to_string(),
            link: "/usr/bin/matlab".to_string(),
            path: "/opt/matlab/R2025a/bin/matlab".to_string(),
            priority: 2025,
            followers: vec![],
        }
    }

    fn matlab_alt_with_followers() -> AlternativeConfig {
        AlternativeConfig {
            name: "matlab".to_string(),
            link: "/usr/bin/matlab".to_string(),
            path: "/opt/matlab/R2025a/bin/matlab".to_string(),
            priority: 2025,
            followers: vec![
                AlternativeFollower {
                    name: "mex".to_string(),
                    link: "/usr/bin/mex".to_string(),
                    path: "/opt/matlab/R2025a/bin/mex".to_string(),
                },
                AlternativeFollower {
                    name: "matlab-help".to_string(),
                    link: "/usr/bin/matlab-help".to_string(),
                    path: "/opt/matlab/R2025a/bin/matlab-help".to_string(),
                },
            ],
        }
    }

    #[test]
    fn test_install_scriptlet_basic() {
        let script = generate_install_scriptlet(&[matlab_alt()]).unwrap();
        assert!(script.contains("# [spm:alternatives]"));
        assert!(
            script.contains("--install /usr/bin/matlab matlab /opt/matlab/R2025a/bin/matlab 2025")
        );
        assert!(!script.contains("--slave"));
    }

    #[test]
    fn test_install_scriptlet_with_followers() {
        let script = generate_install_scriptlet(&[matlab_alt_with_followers()]).unwrap();
        assert!(script.contains("--slave /usr/bin/mex mex /opt/matlab/R2025a/bin/mex"));
        assert!(script.contains(
            "--slave /usr/bin/matlab-help matlab-help /opt/matlab/R2025a/bin/matlab-help"
        ));
    }

    #[test]
    fn test_remove_scriptlet() {
        let script = generate_remove_scriptlet(&[matlab_alt()]).unwrap();
        assert!(script.contains("# [spm:alternatives]"));
        assert!(script.contains("\"$1\" = \"0\""));
        assert!(script.contains("\"$1\" = \"remove\""));
        assert!(
            script.contains("update-alternatives --remove matlab /opt/matlab/R2025a/bin/matlab")
        );
        assert!(script.contains("fi"));
    }

    #[test]
    fn test_empty_alternatives_returns_none() {
        assert!(generate_install_scriptlet(&[]).is_none());
        assert!(generate_remove_scriptlet(&[]).is_none());
    }

    #[test]
    fn test_multiple_alternatives() {
        let alts = vec![
            AlternativeConfig {
                name: "matlab".to_string(),
                link: "/usr/bin/matlab".to_string(),
                path: "/opt/matlab/R2025a/bin/matlab".to_string(),
                priority: 2025,
                followers: vec![],
            },
            AlternativeConfig {
                name: "python".to_string(),
                link: "/usr/bin/python".to_string(),
                path: "/usr/bin/python3.11".to_string(),
                priority: 311,
                followers: vec![],
            },
        ];

        let install = generate_install_scriptlet(&alts).unwrap();
        assert!(install.contains("--install /usr/bin/matlab matlab"));
        assert!(install.contains("--install /usr/bin/python python"));

        let remove = generate_remove_scriptlet(&alts).unwrap();
        assert!(remove.contains("--remove matlab"));
        assert!(remove.contains("--remove python"));
    }

    #[test]
    fn test_resolve_scripts_with_alternatives_only() {
        let tmp = TempDir::new().unwrap();
        let scripts_config = ScriptsConfig::default();
        let alts = vec![matlab_alt()];

        let resolved = resolve_scripts(&scripts_config, &alts, tmp.path()).unwrap();

        assert!(resolved.pre_install.is_none());
        assert!(resolved.post_install.is_some());
        assert!(resolved
            .post_install
            .as_ref()
            .unwrap()
            .contains("--install"));
        assert!(resolved.pre_remove.is_some());
        assert!(resolved.pre_remove.as_ref().unwrap().contains("--remove"));
        assert!(resolved.post_remove.is_none());
    }

    #[test]
    fn test_resolve_scripts_with_user_scripts() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();

        std::fs::write(base.join("postinst.sh"), "echo post-install\n").unwrap();
        std::fs::write(base.join("prerm.sh"), "echo pre-remove\n").unwrap();

        let scripts_config = ScriptsConfig {
            pre_install: None,
            post_install: Some("postinst.sh".into()),
            pre_remove: Some("prerm.sh".into()),
            post_remove: None,
            pre_trans: None,
            post_trans: None,
        };
        let alts = vec![matlab_alt()];

        let resolved = resolve_scripts(&scripts_config, &alts, base).unwrap();

        // Post-install: alternatives BEFORE user script.
        let post = resolved.post_install.unwrap();
        let alt_pos = post.find("[spm:alternatives]").unwrap();
        let user_pos = post.find("echo post-install").unwrap();
        assert!(
            alt_pos < user_pos,
            "alternatives should come before user script in post_install"
        );

        // Pre-remove: user script BEFORE alternatives remove.
        let pre = resolved.pre_remove.unwrap();
        let user_pos = pre.find("echo pre-remove").unwrap();
        let alt_pos = pre.find("[spm:alternatives]").unwrap();
        assert!(
            user_pos < alt_pos,
            "user script should come before alternatives in pre_remove"
        );
    }

    #[test]
    fn test_resolve_scripts_no_alternatives_no_scripts() {
        let tmp = TempDir::new().unwrap();
        let scripts_config = ScriptsConfig::default();
        let resolved = resolve_scripts(&scripts_config, &[], tmp.path()).unwrap();

        assert!(resolved.pre_install.is_none());
        assert!(resolved.post_install.is_none());
        assert!(resolved.pre_remove.is_none());
        assert!(resolved.post_remove.is_none());
    }

    #[test]
    fn test_resolve_scripts_missing_file() {
        let tmp = TempDir::new().unwrap();
        let scripts_config = ScriptsConfig {
            pre_install: None,
            post_install: Some("nonexistent.sh".into()),
            pre_remove: None,
            post_remove: None,
            pre_trans: None,
            post_trans: None,
        };

        let result = resolve_scripts(&scripts_config, &[], tmp.path());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PlanError::ScriptRead { .. }));
    }
}
