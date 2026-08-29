use std::ffi::OsString;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::RuntimeLimits;
use crate::environment::ResolvedLaunchEnvironment;
use crate::probe::run_bounded_command;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub prerelease: Option<String>,
    pub display: String,
}

pub async fn probe_pi_version(
    environment: &ResolvedLaunchEnvironment,
    limits: RuntimeLimits,
) -> Result<PiVersion, PiVersionProbeError> {
    let invocation = environment.pi_invocation();
    let mut args = invocation.prefix_args().to_vec();
    args.push(OsString::from("--version"));
    let output = run_bounded_command(
        invocation.executable(),
        &args,
        None,
        environment.variables(),
        limits.max_version_probe_bytes,
        Duration::from_millis(limits.version_probe_deadline_ms),
    )
    .await
    .map_err(|error| PiVersionProbeError::CommandFailed {
        detail: error.to_string(),
    })?;

    if !output.status.success() {
        return Err(PiVersionProbeError::NonZeroExit {
            code: output.status.code(),
        });
    }
    if output.stdout_exceeded {
        return Err(PiVersionProbeError::OutputTooLarge {
            limit: limits.max_version_probe_bytes,
        });
    }
    let text = std::str::from_utf8(&output.stdout).map_err(|_| PiVersionProbeError::InvalidUtf8)?;
    parse_pi_version(text)
}

pub fn parse_pi_version(output: &str) -> Result<PiVersion, PiVersionProbeError> {
    let token = output
        .split_whitespace()
        .map(|token| token.trim_matches(|c: char| c == ',' || c == ';'))
        .find(|token| {
            token
                .trim_start_matches(['v', 'V'])
                .split('-')
                .next()
                .is_some_and(|core| core.split('.').count() == 3)
        })
        .ok_or(PiVersionProbeError::UnrecognizedVersion)?;
    let normalized = token.trim_start_matches(['v', 'V']);
    let (core, prerelease) = normalized
        .split_once('-')
        .map_or((normalized, None), |(core, suffix)| (core, Some(suffix)));
    let mut components = core.split('.');
    let major = parse_component(components.next())?;
    let minor = parse_component(components.next())?;
    let patch = parse_component(components.next())?;
    if components.next().is_some() {
        return Err(PiVersionProbeError::UnrecognizedVersion);
    }
    let prerelease = prerelease
        .filter(|suffix| !suffix.is_empty())
        .map(str::to_owned);
    let display = match &prerelease {
        Some(suffix) => format!("{major}.{minor}.{patch}-{suffix}"),
        None => format!("{major}.{minor}.{patch}"),
    };
    Ok(PiVersion {
        major,
        minor,
        patch,
        prerelease,
        display,
    })
}

fn parse_component(value: Option<&str>) -> Result<u64, PiVersionProbeError> {
    value
        .and_then(|value| value.parse().ok())
        .ok_or(PiVersionProbeError::UnrecognizedVersion)
}

#[derive(Debug, Error)]
pub enum PiVersionProbeError {
    #[error("Pi version probe command failed: {detail}")]
    CommandFailed { detail: String },
    #[error("Pi version probe exited unsuccessfully with code {code:?}")]
    NonZeroExit { code: Option<i32> },
    #[error("Pi version probe exceeded {limit} bytes")]
    OutputTooLarge { limit: usize },
    #[error("Pi version probe output is not valid UTF-8")]
    InvalidUtf8,
    #[error("Pi version probe output does not contain a recognizable semantic version")]
    UnrecognizedVersion,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::RunId;
    use crate::environment::{LaunchEnvironmentInput, resolve_launch_environment};

    struct Fixture {
        root: PathBuf,
        pi: PathBuf,
    }

    impl Fixture {
        fn new(name: &str, output: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("pi-wizard-version-{name}-{}", RunId::new()));
            fs::create_dir_all(&root).expect("create fixture");
            #[cfg(windows)]
            let pi = root.join("pi.cmd");
            #[cfg(not(windows))]
            let pi = root.join("pi");

            #[cfg(windows)]
            fs::write(&pi, format!("@echo off\r\necho {output}\r\n")).expect("write fake Pi");
            #[cfg(not(windows))]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::write(&pi, format!("#!/bin/sh\nprintf '%s\\n' '{output}'\n"))
                    .expect("write fake Pi");
                let mut permissions = fs::metadata(&pi).expect("metadata").permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&pi, permissions).expect("permissions");
            }
            Self { root, pi }
        }

        fn environment(&self) -> ResolvedLaunchEnvironment {
            let desktop_environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
            resolve_launch_environment(LaunchEnvironmentInput {
                configured_pi: Some(self.pi.clone()),
                desktop_environment,
                ..LaunchEnvironmentInput::default()
            })
            .expect("resolve fake Pi")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn parses_plain_prefixed_and_prerelease_versions() {
        assert_eq!(
            parse_pi_version("0.84.3\n").expect("plain").display,
            "0.84.3"
        );
        assert_eq!(
            parse_pi_version("pi v0.85.0-beta.2\n")
                .expect("prefixed")
                .prerelease
                .as_deref(),
            Some("beta.2")
        );
    }

    #[test]
    fn malformed_version_is_explicit() {
        assert!(matches!(
            parse_pi_version("pi development build"),
            Err(PiVersionProbeError::UnrecognizedVersion)
        ));
    }

    #[tokio::test]
    async fn version_probe_uses_exact_resolved_pi_environment() {
        let fixture = Fixture::new("success", "0.84.3");
        let version = probe_pi_version(&fixture.environment(), RuntimeLimits::default())
            .await
            .expect("version probe");
        assert_eq!(version.display, "0.84.3");
    }

    #[tokio::test]
    async fn version_probe_output_is_bounded_before_parsing() {
        let fixture = Fixture::new("bounded", "01234567890123456789");
        let limits = RuntimeLimits {
            max_version_probe_bytes: 4,
            ..RuntimeLimits::default()
        };
        assert!(matches!(
            probe_pi_version(&fixture.environment(), limits).await,
            Err(PiVersionProbeError::OutputTooLarge { limit: 4 })
        ));
    }
}
