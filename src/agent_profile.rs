//! Harness-specific command-line adapters for node profiles.

use anyhow::{Result, bail};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentProfile {
    pub harness: String,
    pub model: String,
    pub reasoning_effort: String,
    pub permission_profile: String,
    pub raw_agent_args: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileOverrides {
    pub harness: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_profile: Option<String>,
    pub raw_agent_args: Vec<String>,
}

impl AgentProfile {
    /// The node's own launch flags as its record stores them.
    pub fn of_record(record: &crate::thread::Thread) -> AgentProfile {
        AgentProfile {
            harness: record.agent.clone(),
            model: record.model.clone(),
            reasoning_effort: record.reasoning_effort.clone(),
            permission_profile: record.permission_profile.clone(),
            raw_agent_args: record.raw_agent_args.clone(),
        }
    }

    /// True when the node sets no flag of its own: its profile alone launches it.
    pub fn is_plain(&self) -> bool {
        self.model.is_empty()
            && self.reasoning_effort.is_empty()
            && self.permission_profile.is_empty()
            && self.raw_agent_args.is_empty()
    }

    pub fn apply(&mut self, overrides: &ProfileOverrides) {
        if let Some(harness) = &overrides.harness {
            self.harness = harness.clone();
        }
        if let Some(model) = &overrides.model {
            self.model = model.clone();
        }
        if let Some(effort) = &overrides.reasoning_effort {
            self.reasoning_effort = effort.clone();
        }
        if let Some(permissions) = &overrides.permission_profile {
            self.permission_profile = permissions.clone();
        }
        self.raw_agent_args
            .extend(overrides.raw_agent_args.iter().cloned());
    }

    /// Returns argv components for the selected harness: the node's model and
    /// effort flags (the profile flags' own adapter), its permission flags,
    /// its raw arguments, then `safety_args` (the profile's and the project's
    /// arguments). Each stays a separate process argument.
    pub fn argv(&self, safety_args: &[String]) -> Result<Vec<String>> {
        validate_arg("harness", &self.harness)?;
        validate_optional_arg("model", &self.model)?;
        validate_optional_arg("reasoning effort", &self.reasoning_effort)?;
        validate_optional_arg("permission profile", &self.permission_profile)?;
        for argument in self.raw_agent_args.iter().chain(safety_args) {
            if argument.contains('\0') {
                bail!("agent argv components may not contain a NUL byte");
            }
        }
        validate_permission_overrides(self, safety_args)?;

        let mut argv =
            crate::profiles::typed_args(&self.harness, &self.model, &self.reasoning_effort)?;
        match self.harness.as_str() {
            "codex" => codex_permissions(self, &mut argv)?,
            "claude" => claude_permissions(self, &mut argv)?,
            other => {
                if !self.permission_profile.is_empty() {
                    bail!(
                        "the `{other}` harness has no built-in permission adapter; pass harness-specific values with repeatable `--raw-agent-arg` options"
                    );
                }
            }
        }
        argv.extend(self.raw_agent_args.iter().cloned());
        argv.extend(safety_args.iter().cloned());
        Ok(argv)
    }
}

fn validate_permission_overrides(profile: &AgentProfile, safety_args: &[String]) -> Result<()> {
    if profile.permission_profile.is_empty() {
        return Ok(());
    }
    let protected_flags: &[&str] = match profile.harness.as_str() {
        "codex" => &[
            "--sandbox",
            "-s",
            "--ask-for-approval",
            "-a",
            "--full-auto",
            "--dangerously-bypass-approvals-and-sandbox",
            "--yolo",
        ],
        "claude" => &[
            "--permission-mode",
            "--permission-prompt-tool",
            "--dangerously-skip-permissions",
            "--allow-dangerously-skip-permissions",
            "--allowedTools",
            "--disallowedTools",
            "--tools",
        ],
        _ => return Ok(()),
    };

    for (source, arguments) in [
        ("raw agent arguments", profile.raw_agent_args.as_slice()),
        ("profile and project arguments", safety_args),
    ] {
        for argument in arguments {
            let flag = argument
                .split_once('=')
                .map_or(argument.as_str(), |(name, _)| name);
            let protected = protected_flags.iter().find(|protected| {
                if protected.starts_with("--") {
                    flag == **protected
                } else {
                    argument == **protected
                        || argument
                            .strip_prefix(**protected)
                            .is_some_and(|value| !value.is_empty() && !value.starts_with('-'))
                }
            });
            if protected.is_some() {
                bail!(
                    "{source} may not override `{}` permission profile with `{flag}`",
                    profile.harness
                );
            }
        }
    }
    Ok(())
}

fn validate_arg(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() || value.contains('\0') || value.chars().any(char::is_control) {
        bail!("{label} must be non-empty and contain no control characters");
    }
    Ok(())
}

fn validate_optional_arg(label: &str, value: &str) -> Result<()> {
    if !value.is_empty() {
        validate_arg(label, value)?;
    }
    Ok(())
}

fn codex_permissions(profile: &AgentProfile, argv: &mut Vec<String>) -> Result<()> {
    match profile.permission_profile.as_str() {
        "" => {}
        "read-only" => argv.extend([
            "--sandbox".into(),
            "read-only".into(),
            "--ask-for-approval".into(),
            "on-request".into(),
        ]),
        "workspace-write" => argv.extend([
            "--sandbox".into(),
            "workspace-write".into(),
            "--ask-for-approval".into(),
            "on-request".into(),
        ]),
        "full-access" => argv.extend([
            "--sandbox".into(),
            "danger-full-access".into(),
            "--ask-for-approval".into(),
            "never".into(),
        ]),
        value => bail!(
            "Codex permission profile must be read-only, workspace-write or full-access, not `{value}`"
        ),
    }
    Ok(())
}

fn claude_permissions(profile: &AgentProfile, argv: &mut Vec<String>) -> Result<()> {
    let permission_mode = match profile.permission_profile.as_str() {
        "" => None,
        "default" => Some("default"),
        "plan" => Some("plan"),
        "accept-edits" => Some("acceptEdits"),
        "bypass-permissions" => Some("bypassPermissions"),
        value => bail!(
            "Claude permission profile must be default, plan, accept-edits or bypass-permissions, not `{value}`"
        ),
    };
    if let Some(permission_mode) = permission_mode {
        argv.extend(["--permission-mode".into(), permission_mode.into()]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_profile_and_legacy_safety_args_keep_exact_argv_order() {
        let profile = AgentProfile {
            harness: "codex".into(),
            model: "gpt-5.6".into(),
            reasoning_effort: "high".into(),
            permission_profile: "workspace-write".into(),
            raw_agent_args: vec![
                "--literal".into(),
                "$(touch must-not-exist); 'quoted'".into(),
            ],
        };
        let safety = vec!["--legacy-safe".into(), "two words".into()];
        assert_eq!(
            profile.argv(&safety).unwrap(),
            [
                "--model",
                "gpt-5.6",
                "-c",
                "model_reasoning_effort=\"high\"",
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request",
                "--literal",
                "$(touch must-not-exist); 'quoted'",
                "--legacy-safe",
                "two words",
            ]
        );
    }

    #[test]
    fn codex_profile_accepts_max_reasoning_effort() {
        let profile = AgentProfile {
            harness: "codex".into(),
            reasoning_effort: "max".into(),
            ..AgentProfile::default()
        };
        assert_eq!(
            profile.argv(&[]).unwrap(),
            ["-c", "model_reasoning_effort=\"max\""]
        );
    }

    #[test]
    fn claude_profile_uses_its_own_model_and_permission_flags() {
        let profile = AgentProfile {
            harness: "claude".into(),
            model: "claude-sonnet-4".into(),
            permission_profile: "accept-edits".into(),
            ..AgentProfile::default()
        };
        assert_eq!(
            profile.argv(&[]).unwrap(),
            [
                "--model",
                "claude-sonnet-4",
                "--permission-mode",
                "acceptEdits"
            ]
        );
    }

    #[test]
    fn unsupported_profile_values_fail_before_launch() {
        let profile = AgentProfile {
            harness: "codex".into(),
            reasoning_effort: "extreme".into(),
            ..AgentProfile::default()
        };
        assert!(
            profile
                .argv(&[])
                .unwrap_err()
                .to_string()
                .contains("effort")
        );

        let profile = AgentProfile {
            harness: "claude".into(),
            reasoning_effort: "high".into(),
            ..AgentProfile::default()
        };
        assert_eq!(profile.argv(&[]).unwrap(), ["--effort", "high"]);

        let profile = AgentProfile {
            harness: "gemini".into(),
            reasoning_effort: "high".into(),
            ..AgentProfile::default()
        };
        assert!(
            profile
                .argv(&[])
                .unwrap_err()
                .to_string()
                .contains("effort")
        );
    }

    #[test]
    fn unknown_harness_accepts_only_explicit_raw_arguments() {
        let profile = AgentProfile {
            harness: "custom-agent".into(),
            raw_agent_args: vec!["--custom-flag".into()],
            ..AgentProfile::default()
        };
        assert_eq!(profile.argv(&[]).unwrap(), ["--custom-flag"]);
        let profile = AgentProfile {
            permission_profile: "plan".into(),
            ..profile
        };
        assert!(
            profile
                .argv(&[])
                .unwrap_err()
                .to_string()
                .contains("no built-in permission")
        );
    }

    #[test]
    fn rejects_nul_in_any_persisted_argv_component() {
        let profile = AgentProfile {
            harness: "custom-agent".into(),
            raw_agent_args: vec!["bad\0value".into()],
            ..AgentProfile::default()
        };
        assert!(profile.argv(&[]).unwrap_err().to_string().contains("NUL"));

        let profile = AgentProfile {
            harness: "custom-agent".into(),
            ..AgentProfile::default()
        };
        assert!(profile.argv(&["bad\0value".into()]).is_err());
    }

    #[test]
    fn codex_permission_profile_rejects_raw_and_safety_overrides() {
        let raw = AgentProfile {
            harness: "codex".into(),
            permission_profile: "workspace-write".into(),
            raw_agent_args: vec!["--sandbox=danger-full-access".into()],
            ..AgentProfile::default()
        };
        assert!(raw.argv(&[]).unwrap_err().to_string().contains("--sandbox"));

        let short = AgentProfile {
            harness: "codex".into(),
            permission_profile: "workspace-write".into(),
            raw_agent_args: vec!["-sdanger-full-access".into()],
            ..AgentProfile::default()
        };
        assert!(short.argv(&[]).unwrap_err().to_string().contains("-s"));

        let profile = AgentProfile {
            harness: "codex".into(),
            permission_profile: "read-only".into(),
            ..AgentProfile::default()
        };
        let error = profile
            .argv(&["--ask-for-approval".into(), "never".into()])
            .unwrap_err()
            .to_string();
        assert!(error.contains("profile and project arguments"));
        assert!(error.contains("--ask-for-approval"));
    }

    #[test]
    fn claude_permission_profile_rejects_raw_and_safety_overrides() {
        let raw = AgentProfile {
            harness: "claude".into(),
            permission_profile: "plan".into(),
            raw_agent_args: vec!["--permission-mode=default".into()],
            ..AgentProfile::default()
        };
        assert!(
            raw.argv(&[])
                .unwrap_err()
                .to_string()
                .contains("--permission-mode")
        );

        let profile = AgentProfile {
            harness: "claude".into(),
            permission_profile: "accept-edits".into(),
            ..AgentProfile::default()
        };
        let error = profile
            .argv(&["--dangerously-skip-permissions".into()])
            .unwrap_err()
            .to_string();
        assert!(error.contains("profile and project arguments"));
        assert!(error.contains("--dangerously-skip-permissions"));
    }
}
