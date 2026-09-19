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

    /// Returns argv components for the selected harness. Raw arguments and
    /// legacy project-wide safety arguments remain separate process arguments.
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

        let mut argv = Vec::new();
        match self.harness.as_str() {
            "codex" => codex_argv(self, &mut argv)?,
            "claude" => claude_argv(self, &mut argv)?,
            other => {
                if !self.model.is_empty()
                    || !self.reasoning_effort.is_empty()
                    || !self.permission_profile.is_empty()
                {
                    bail!(
                        "the `{other}` harness has no built-in model, reasoning or permission adapter; pass harness-specific values with repeatable `--raw-agent-arg` options"
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
        ("project safety arguments", safety_args),
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

fn codex_argv(profile: &AgentProfile, argv: &mut Vec<String>) -> Result<()> {
    if !profile.model.is_empty() {
        argv.extend(["--model".into(), profile.model.clone()]);
    }
    if !profile.reasoning_effort.is_empty() {
        if !matches!(
            profile.reasoning_effort.as_str(),
            "minimal" | "low" | "medium" | "high" | "xhigh"
        ) {
            bail!("Codex reasoning effort must be minimal, low, medium, high or xhigh");
        }
        argv.extend([
            "--config".into(),
            format!("model_reasoning_effort={:?}", profile.reasoning_effort),
        ]);
    }
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

fn claude_argv(profile: &AgentProfile, argv: &mut Vec<String>) -> Result<()> {
    if !profile.reasoning_effort.is_empty() {
        bail!(
            "Claude Code has no built-in reasoning-effort adapter; use repeatable `--raw-agent-arg` options if your setup supports one"
        );
    }
    if !profile.model.is_empty() {
        argv.extend(["--model".into(), profile.model.clone()]);
    }
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
                "--config",
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
                .contains("reasoning effort")
        );

        let profile = AgentProfile {
            harness: "claude".into(),
            reasoning_effort: "high".into(),
            ..AgentProfile::default()
        };
        assert!(
            profile
                .argv(&[])
                .unwrap_err()
                .to_string()
                .contains("no built-in reasoning")
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
            model: "model-x".into(),
            ..profile
        };
        assert!(
            profile
                .argv(&[])
                .unwrap_err()
                .to_string()
                .contains("no built-in")
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
        assert!(error.contains("project safety arguments"));
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
        assert!(error.contains("project safety arguments"));
        assert!(error.contains("--dangerously-skip-permissions"));
    }
}
