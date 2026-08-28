use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::PiSessionId;
use crate::rpc::ThinkingLevel;

/// Project-resource trust behavior for a Pi launch.
///
/// `Inherit` intentionally emits no CLI override so Pi can apply the closest
/// saved canonical-directory trust decision and then `defaultProjectTrust`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectTrustPolicy {
    Inherit,
    Approve,
    Ignore,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionDiscoveryPolicy {
    #[default]
    Inherit,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupNetworkPolicy {
    #[default]
    Inherit,
    Offline,
}

/// Launch-time visibility of LLM-callable Pi tools.
///
/// This is not a process sandbox: extension code and the Pi process still run
/// with host permissions. It only maps Pi's own tool-selection CLI flags.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSelectionPolicy {
    #[default]
    Inherit,
    NoTools,
    NoBuiltinTools,
    AllowOnly(Vec<String>),
    Exclude(Vec<String>),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextFilesPolicy {
    #[default]
    Inherit,
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionLaunch {
    New,
    NewWithId(PiSessionId),
    Ephemeral,
    Resume(PathBuf),
}

/// Unresolved launch intent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PiLaunchSpec {
    pub executable: PathBuf,
    pub cwd: PathBuf,
    pub project_trust: ProjectTrustPolicy,
    pub context_files: ContextFilesPolicy,
    pub extension_discovery: ExtensionDiscoveryPolicy,
    pub startup_network: StartupNetworkPolicy,
    pub tools: ToolSelectionPolicy,
    pub session: SessionLaunch,
    pub session_dir: Option<PathBuf>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<ThinkingLevel>,
    pub session_name: Option<String>,
}

impl PiLaunchSpec {
    #[must_use]
    pub fn new(
        executable: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
        project_trust: ProjectTrustPolicy,
    ) -> Self {
        Self {
            executable: executable.into(),
            cwd: cwd.into(),
            project_trust,
            context_files: ContextFilesPolicy::Inherit,
            extension_discovery: ExtensionDiscoveryPolicy::Inherit,
            startup_network: StartupNetworkPolicy::Inherit,
            tools: ToolSelectionPolicy::Inherit,
            session: SessionLaunch::New,
            session_dir: None,
            provider: None,
            model: None,
            thinking: None,
            session_name: None,
        }
    }

    pub fn resolve(self) -> Result<ResolvedPiLaunchSpec, LaunchSpecError> {
        let canonical_cwd = self.cwd.canonicalize().map_err(|source| {
            LaunchSpecError::CanonicalizeWorkingDirectory {
                path: self.cwd.clone(),
                source,
            }
        })?;

        let canonical_session_dir = self
            .session_dir
            .map(|path| {
                path.canonicalize().map_err(|source| {
                    LaunchSpecError::CanonicalizeSessionDirectory { path, source }
                })
            })
            .transpose()?;

        validate_tool_policy(&self.tools)?;
        Ok(ResolvedPiLaunchSpec {
            executable: self.executable,
            canonical_cwd,
            project_trust: self.project_trust,
            context_files: self.context_files,
            extension_discovery: self.extension_discovery,
            startup_network: self.startup_network,
            tools: self.tools,
            session: self.session,
            canonical_session_dir,
            provider: self.provider,
            model: self.model,
            thinking: self.thinking,
            session_name: self.session_name,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedPiLaunchSpec {
    pub executable: PathBuf,
    pub canonical_cwd: PathBuf,
    pub project_trust: ProjectTrustPolicy,
    pub context_files: ContextFilesPolicy,
    pub extension_discovery: ExtensionDiscoveryPolicy,
    pub startup_network: StartupNetworkPolicy,
    pub tools: ToolSelectionPolicy,
    pub session: SessionLaunch,
    pub canonical_session_dir: Option<PathBuf>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<ThinkingLevel>,
    pub session_name: Option<String>,
}

impl ResolvedPiLaunchSpec {
    /// Deterministic argument vector for a directly spawnable Pi executable.
    #[must_use]
    pub fn args(&self) -> Vec<OsString> {
        let mut args = vec![OsString::from("--mode"), OsString::from("rpc")];
        match self.project_trust {
            ProjectTrustPolicy::Inherit => {}
            ProjectTrustPolicy::Approve => args.push(OsString::from("--approve")),
            ProjectTrustPolicy::Ignore => args.push(OsString::from("--no-approve")),
        }
        if self.context_files == ContextFilesPolicy::Disabled {
            args.push(OsString::from("--no-context-files"));
        }
        if self.extension_discovery == ExtensionDiscoveryPolicy::Disabled {
            args.push(OsString::from("--no-extensions"));
        }
        if self.startup_network == StartupNetworkPolicy::Offline {
            args.push(OsString::from("--offline"));
        }

        match &self.tools {
            ToolSelectionPolicy::Inherit => {}
            ToolSelectionPolicy::NoTools => args.push(OsString::from("--no-tools")),
            ToolSelectionPolicy::NoBuiltinTools => {
                args.push(OsString::from("--no-builtin-tools"));
            }
            ToolSelectionPolicy::AllowOnly(tools) => {
                args.push(OsString::from("--tools"));
                args.push(OsString::from(tools.join(",")));
            }
            ToolSelectionPolicy::Exclude(tools) => {
                args.push(OsString::from("--exclude-tools"));
                args.push(OsString::from(tools.join(",")));
            }
        }

        match &self.session {
            SessionLaunch::New => {}
            SessionLaunch::NewWithId(id) => {
                args.push(OsString::from("--session-id"));
                args.push(OsString::from(id.to_string()));
            }
            SessionLaunch::Ephemeral => args.push(OsString::from("--no-session")),
            SessionLaunch::Resume(path) => {
                args.push(OsString::from("--session"));
                args.push(path.as_os_str().to_owned());
            }
        }

        if let Some(session_dir) = &self.canonical_session_dir {
            args.push(OsString::from("--session-dir"));
            args.push(session_dir.as_os_str().to_owned());
        }

        if let Some(provider) = &self.provider {
            args.push(OsString::from("--provider"));
            args.push(OsString::from(provider));
        }
        if let Some(model) = &self.model {
            args.push(OsString::from("--model"));
            args.push(OsString::from(model));
        }
        if let Some(thinking) = self.thinking {
            args.push(OsString::from("--thinking"));
            args.push(OsString::from(thinking.as_str()));
        }
        if let Some(name) = &self.session_name {
            args.push(OsString::from("--name"));
            args.push(OsString::from(name));
        }
        args
    }

    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.canonical_cwd
    }
}

fn validate_tool_policy(policy: &ToolSelectionPolicy) -> Result<(), LaunchSpecError> {
    let tools = match policy {
        ToolSelectionPolicy::AllowOnly(tools) | ToolSelectionPolicy::Exclude(tools) => tools,
        _ => return Ok(()),
    };
    if tools.is_empty() {
        return Err(LaunchSpecError::EmptyToolList);
    }
    if let Some(tool) = tools
        .iter()
        .find(|tool| tool.is_empty() || tool.contains(',') || tool.chars().any(char::is_whitespace))
    {
        return Err(LaunchSpecError::InvalidToolName { tool: tool.clone() });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum LaunchSpecError {
    #[error("failed to canonicalize Pi working directory {path}: {source}")]
    CanonicalizeWorkingDirectory { path: PathBuf, source: io::Error },
    #[error("failed to canonicalize Pi session directory {path}: {source}")]
    CanonicalizeSessionDirectory { path: PathBuf, source: io::Error },
    #[error("Pi tool allow/exclude list must not be empty")]
    EmptyToolList,
    #[error("invalid Pi tool name in launch policy: {tool}")]
    InvalidToolName { tool: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn launch_can_inherit_saved_project_trust_without_override() {
        let spec = ResolvedPiLaunchSpec {
            executable: PathBuf::from("pi"),
            canonical_cwd: PathBuf::from("project"),
            project_trust: ProjectTrustPolicy::Inherit,
            context_files: ContextFilesPolicy::Inherit,
            extension_discovery: ExtensionDiscoveryPolicy::Inherit,
            startup_network: StartupNetworkPolicy::Inherit,
            tools: ToolSelectionPolicy::Inherit,
            session: SessionLaunch::New,
            canonical_session_dir: None,
            provider: None,
            model: None,
            thinking: None,
            session_name: None,
        };

        assert_eq!(strings(spec.args()), ["--mode", "rpc"]);
    }

    #[test]
    fn resume_and_model_options_are_separate_arguments() {
        let spec = ResolvedPiLaunchSpec {
            executable: PathBuf::from("pi"),
            canonical_cwd: PathBuf::from("project"),
            project_trust: ProjectTrustPolicy::Approve,
            context_files: ContextFilesPolicy::Disabled,
            extension_discovery: ExtensionDiscoveryPolicy::Inherit,
            startup_network: StartupNetworkPolicy::Inherit,
            tools: ToolSelectionPolicy::Inherit,
            session: SessionLaunch::Resume(PathBuf::from("a session.jsonl")),
            canonical_session_dir: Some(PathBuf::from("sessions")),
            provider: Some("openai".to_owned()),
            model: Some("gpt-5.6".to_owned()),
            thinking: Some(ThinkingLevel::High),
            session_name: Some("foundation work".to_owned()),
        };

        assert_eq!(
            strings(spec.args()),
            [
                "--mode",
                "rpc",
                "--approve",
                "--no-context-files",
                "--session",
                "a session.jsonl",
                "--session-dir",
                "sessions",
                "--provider",
                "openai",
                "--model",
                "gpt-5.6",
                "--thinking",
                "high",
                "--name",
                "foundation work"
            ]
        );
    }

    #[test]
    fn ignore_project_resources_does_not_implicitly_disable_context_files() {
        let spec = ResolvedPiLaunchSpec {
            executable: PathBuf::from("pi"),
            canonical_cwd: PathBuf::from("project"),
            project_trust: ProjectTrustPolicy::Ignore,
            context_files: ContextFilesPolicy::Inherit,
            extension_discovery: ExtensionDiscoveryPolicy::Inherit,
            startup_network: StartupNetworkPolicy::Inherit,
            tools: ToolSelectionPolicy::Inherit,
            session: SessionLaunch::New,
            canonical_session_dir: None,
            provider: None,
            model: None,
            thinking: None,
            session_name: None,
        };

        assert_eq!(strings(spec.args()), ["--mode", "rpc", "--no-approve"]);
    }

    #[test]
    fn app_created_session_id_offline_recovery_and_tool_allowlist_are_typed_flags() {
        let session_id = PiSessionId::new();
        let spec = ResolvedPiLaunchSpec {
            executable: PathBuf::from("pi"),
            canonical_cwd: PathBuf::from("project"),
            project_trust: ProjectTrustPolicy::Ignore,
            context_files: ContextFilesPolicy::Inherit,
            extension_discovery: ExtensionDiscoveryPolicy::Disabled,
            startup_network: StartupNetworkPolicy::Offline,
            tools: ToolSelectionPolicy::AllowOnly(vec![
                "read".to_owned(),
                "grep".to_owned(),
                "find".to_owned(),
                "ls".to_owned(),
            ]),
            session: SessionLaunch::NewWithId(session_id),
            canonical_session_dir: None,
            provider: None,
            model: None,
            thinking: None,
            session_name: None,
        };

        assert_eq!(
            strings(spec.args()),
            [
                "--mode",
                "rpc",
                "--no-approve",
                "--no-extensions",
                "--offline",
                "--tools",
                "read,grep,find,ls",
                "--session-id",
                &session_id.to_string()
            ]
        );
    }
}
