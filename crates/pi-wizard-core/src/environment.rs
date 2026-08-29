use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::RuntimeLimits;
use crate::probe::run_bounded_command;

const SHELL_ENV_MARKER: &[u8] = b"PI_WIZARD_ENV_V1\0";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentSource {
    Configured,
    DesktopProcess,
    ShellProbe,
}

/// Runs one bounded login-shell probe and returns the resulting process
/// environment for use as a complete launch profile.
///
/// The returned values may contain credentials and must remain backend-only.
/// Errors deliberately omit stdout/stderr and environment values.
pub async fn probe_login_shell_environment(
    desktop_environment: &BTreeMap<OsString, OsString>,
    limits: RuntimeLimits,
) -> Result<BTreeMap<OsString, OsString>, EnvironmentProbeError> {
    let (shell, args) = platform_shell_probe(desktop_environment)?;
    let output = run_bounded_command(
        &shell,
        &args,
        None,
        desktop_environment,
        limits.max_environment_probe_bytes,
        Duration::from_millis(limits.environment_probe_deadline_ms),
    )
    .await
    .map_err(|_| EnvironmentProbeError::CommandFailed)?;

    if !output.status.success() {
        return Err(EnvironmentProbeError::NonZeroExit {
            code: output.status.code(),
        });
    }
    if output.stdout_exceeded {
        return Err(EnvironmentProbeError::OutputTooLarge {
            limit: limits.max_environment_probe_bytes,
        });
    }

    parse_probed_environment(&output.stdout)
}

fn parse_probed_environment(
    output: &[u8],
) -> Result<BTreeMap<OsString, OsString>, EnvironmentProbeError> {
    let marker_index = output
        .windows(SHELL_ENV_MARKER.len())
        .position(|window| window == SHELL_ENV_MARKER)
        .ok_or(EnvironmentProbeError::MissingMarker)?;
    let payload = &output[marker_index + SHELL_ENV_MARKER.len()..];
    let mut environment = BTreeMap::new();

    for entry in payload
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let Some(separator) = entry.iter().position(|byte| *byte == b'=') else {
            return Err(EnvironmentProbeError::MalformedEntry);
        };
        if separator == 0 {
            return Err(EnvironmentProbeError::MalformedEntry);
        }
        let key = bytes_to_os_string(&entry[..separator])?;
        let value = bytes_to_os_string(&entry[separator + 1..])?;
        remove_env_key(&mut environment, &key);
        environment.insert(key, value);
    }

    if env_value(&environment, "PATH").is_none() {
        return Err(EnvironmentProbeError::MissingPath);
    }
    Ok(environment)
}

#[cfg(unix)]
fn platform_shell_probe(
    environment: &BTreeMap<OsString, OsString>,
) -> Result<(PathBuf, Vec<OsString>), EnvironmentProbeError> {
    let shell = env_value(environment, "SHELL").ok_or(EnvironmentProbeError::ShellUnavailable)?;
    let shell = canonical_usable_executable(Path::new(shell))
        .map_err(|_| EnvironmentProbeError::ShellUnavailable)?;
    Ok((
        shell,
        vec![
            OsString::from("-l"),
            OsString::from("-c"),
            OsString::from("printf 'PI_WIZARD_ENV_V1\\0'; env -0"),
        ],
    ))
}

#[cfg(windows)]
fn platform_shell_probe(
    environment: &BTreeMap<OsString, OsString>,
) -> Result<(PathBuf, Vec<OsString>), EnvironmentProbeError> {
    let pwsh = find_executable(environment, "pwsh")
        .map_err(|_| EnvironmentProbeError::ShellUnavailable)?;
    let powershell = find_executable(environment, "powershell")
        .map_err(|_| EnvironmentProbeError::ShellUnavailable)?;
    let shell = pwsh
        .or(powershell)
        .ok_or(EnvironmentProbeError::ShellUnavailable)?;
    let script = concat!(
        "$o=[Console]::Out;",
        "$o.Write(\"PI_WIZARD_ENV_V1`0\");",
        "Get-ChildItem Env: | ForEach-Object {",
        "$o.Write($_.Name + '=' + $_.Value + \"`0\")",
        "}"
    );
    Ok((
        shell,
        vec![
            OsString::from("-NoLogo"),
            OsString::from("-NonInteractive"),
            OsString::from("-Command"),
            OsString::from(script),
        ],
    ))
}

#[cfg(unix)]
fn bytes_to_os_string(bytes: &[u8]) -> Result<OsString, EnvironmentProbeError> {
    use std::os::unix::ffi::OsStringExt;
    Ok(OsString::from_vec(bytes.to_vec()))
}

#[cfg(windows)]
fn bytes_to_os_string(bytes: &[u8]) -> Result<OsString, EnvironmentProbeError> {
    String::from_utf8(bytes.to_vec())
        .map(OsString::from)
        .map_err(|_| EnvironmentProbeError::InvalidEncoding)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutableSource {
    Configured,
    Path,
}

#[derive(Debug, Error)]
pub enum EnvironmentProbeError {
    #[error("no supported login shell executable is available for environment probing")]
    ShellUnavailable,
    #[error("login-shell environment probe command failed")]
    CommandFailed,
    #[error("login-shell environment probe exited unsuccessfully with code {code:?}")]
    NonZeroExit { code: Option<i32> },
    #[error("login-shell environment probe exceeded {limit} bytes")]
    OutputTooLarge { limit: usize },
    #[error("login-shell environment probe did not emit its protocol marker")]
    MissingMarker,
    #[error("login-shell environment probe emitted a malformed environment entry")]
    MalformedEntry,
    #[error("login-shell environment probe did not provide PATH")]
    MissingPath,
    #[error(
        "login-shell environment probe emitted text that cannot be represented on this platform"
    )]
    InvalidEncoding,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedExecutable {
    pub path: PathBuf,
    pub source: ExecutableSource,
}

/// Inputs to desktop launch-environment selection.
///
/// Environment values are intentionally plain process data rather than a
/// serializable settings object. They may contain provider credentials. The
/// resolved owner never includes values in Debug output or diagnostics.
#[derive(Clone, Default)]
pub struct LaunchEnvironmentInput {
    pub configured_pi: Option<PathBuf>,
    pub configured_environment: BTreeMap<OsString, OsString>,
    pub desktop_environment: BTreeMap<OsString, OsString>,
    pub shell_probe_environment: Option<BTreeMap<OsString, OsString>>,
}

impl fmt::Debug for LaunchEnvironmentInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LaunchEnvironmentInput")
            .field("configured_pi", &self.configured_pi)
            .field(
                "configured_environment_entries",
                &self.configured_environment.len(),
            )
            .field(
                "desktop_environment_entries",
                &self.desktop_environment.len(),
            )
            .field(
                "has_shell_probe_environment",
                &self.shell_probe_environment.is_some(),
            )
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchEnvironmentDiagnostics {
    pub path_source: EnvironmentSource,
    pub pi: ResolvedExecutable,
    pub git: Option<ResolvedExecutable>,
    pub environment_entry_count: usize,
}

/// Secret-bearing environment selected for all tools owned by one launch
/// profile. Pi, Git, and later toolchain probes must use this same environment
/// so discovery cannot succeed under one PATH and execution happen under
/// another.
#[derive(Clone)]
pub struct ResolvedLaunchEnvironment {
    variables: BTreeMap<OsString, OsString>,
    diagnostics: LaunchEnvironmentDiagnostics,
    pi_invocation: ResolvedPiInvocation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedPiInvocation {
    executable: PathBuf,
    prefix_args: Vec<OsString>,
    direct_npm_node: bool,
}

impl ResolvedPiInvocation {
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    #[must_use]
    pub fn prefix_args(&self) -> &[OsString] {
        &self.prefix_args
    }

    #[must_use]
    pub const fn is_direct_npm_node(&self) -> bool {
        self.direct_npm_node
    }
}

impl ResolvedLaunchEnvironment {
    #[must_use]
    pub fn variables(&self) -> &BTreeMap<OsString, OsString> {
        &self.variables
    }

    #[must_use]
    pub const fn diagnostics(&self) -> &LaunchEnvironmentDiagnostics {
        &self.diagnostics
    }

    #[must_use]
    pub fn pi_executable(&self) -> &Path {
        &self.diagnostics.pi.path
    }

    #[must_use]
    pub const fn pi_invocation(&self) -> &ResolvedPiInvocation {
        &self.pi_invocation
    }

    #[must_use]
    pub fn git_executable(&self) -> Option<&Path> {
        self.diagnostics.git.as_ref().map(|git| git.path.as_path())
    }
}

impl fmt::Debug for ResolvedLaunchEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedLaunchEnvironment")
            .field("diagnostics", &self.diagnostics)
            .finish_non_exhaustive()
    }
}

pub fn resolve_launch_environment(
    input: LaunchEnvironmentInput,
) -> Result<ResolvedLaunchEnvironment, EnvironmentResolutionError> {
    let configured_has_path = env_value(&input.configured_environment, "PATH").is_some();
    let mut desktop = input.desktop_environment;
    overlay_environment(&mut desktop, &input.configured_environment);

    let (mut selected, selected_source, pi) = if let Some(configured_pi) = input.configured_pi {
        let path = canonical_usable_executable(&configured_pi).map_err(|source| {
            EnvironmentResolutionError::ConfiguredPiUnavailable {
                path: configured_pi,
                source,
            }
        })?;
        let source = if configured_has_path {
            EnvironmentSource::Configured
        } else {
            EnvironmentSource::DesktopProcess
        };
        (
            desktop,
            source,
            ResolvedExecutable {
                path,
                source: ExecutableSource::Configured,
            },
        )
    } else if let Some(path) = find_executable(&desktop, "pi")? {
        let source = if configured_has_path {
            EnvironmentSource::Configured
        } else {
            EnvironmentSource::DesktopProcess
        };
        (
            desktop,
            source,
            ResolvedExecutable {
                path,
                source: ExecutableSource::Path,
            },
        )
    } else if let Some(mut shell) = input.shell_probe_environment {
        overlay_environment(&mut shell, &input.configured_environment);
        let path = find_executable(&shell, "pi")?
            .ok_or(EnvironmentResolutionError::PiNotFoundInAnyEnvironment)?;
        let source = if configured_has_path {
            EnvironmentSource::Configured
        } else {
            EnvironmentSource::ShellProbe
        };
        (
            shell,
            source,
            ResolvedExecutable {
                path,
                source: ExecutableSource::Path,
            },
        )
    } else {
        return Err(EnvironmentResolutionError::PiNotFoundInAnyEnvironment);
    };

    // Normalize PATH key casing after selection so a Windows environment does
    // not accidentally carry both `Path` and `PATH` when a configured override
    // was applied.
    normalize_path_key(&mut selected);

    let pi_invocation = resolve_pi_invocation(&pi.path, &selected)?;
    let git = find_executable(&selected, "git")?.map(|path| ResolvedExecutable {
        path,
        source: ExecutableSource::Path,
    });
    let diagnostics = LaunchEnvironmentDiagnostics {
        path_source: selected_source,
        pi,
        git,
        environment_entry_count: selected.len(),
    };
    Ok(ResolvedLaunchEnvironment {
        variables: selected,
        diagnostics,
        pi_invocation,
    })
}

fn resolve_pi_invocation(
    logical_pi: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<ResolvedPiInvocation, EnvironmentResolutionError> {
    #[cfg(windows)]
    {
        // npm installs both an extensionless POSIX shim (`pi`) and `pi.cmd` on
        // Windows. PATH resolution can legitimately return either one. Detect
        // the package layout instead of guessing from the shim extension so the
        // long-lived Pi child is always direct node.exe + cli.js.
        if logical_pi
            .file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("pi"))
            && let Some(parent) = logical_pi.parent()
        {
            let cli = parent
                .join("node_modules")
                .join("@earendil-works")
                .join("pi-coding-agent")
                .join("dist")
                .join("bundle")
                .join("cli.js");
            if cli.is_file() {
                let adjacent_node = parent.join("node.exe");
                let node = adjacent_node
                    .is_file()
                    .then(|| canonical_usable_executable(&adjacent_node).ok())
                    .flatten()
                    .or(find_executable(environment, "node")?);
                if let Some(node) = node {
                    return Ok(ResolvedPiInvocation {
                        executable: node,
                        prefix_args: vec![process_argument_path(&cli)],
                        direct_npm_node: true,
                    });
                }
                return Err(EnvironmentResolutionError::StandardNpmPiNodeUnavailable {
                    pi: logical_pi.to_path_buf(),
                    cli,
                });
            }
        }
    }
    let _ = environment;
    Ok(ResolvedPiInvocation {
        executable: logical_pi.to_path_buf(),
        prefix_args: Vec::new(),
        direct_npm_node: false,
    })
}

fn process_argument_path(path: &Path) -> OsString {
    #[cfg(windows)]
    {
        let value = path.as_os_str().to_string_lossy();
        if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            return OsString::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            return OsString::from(rest);
        }
    }
    path.as_os_str().to_os_string()
}

fn overlay_environment(
    destination: &mut BTreeMap<OsString, OsString>,
    overrides: &BTreeMap<OsString, OsString>,
) {
    for (key, value) in overrides {
        remove_env_key(destination, key);
        destination.insert(key.clone(), value.clone());
    }
}

fn normalize_path_key(environment: &mut BTreeMap<OsString, OsString>) {
    let Some(path) = env_value(environment, "PATH").cloned() else {
        return;
    };
    remove_env_key(environment, OsStr::new("PATH"));
    environment.insert(OsString::from("PATH"), path);
}

fn env_value<'a>(environment: &'a BTreeMap<OsString, OsString>, key: &str) -> Option<&'a OsString> {
    #[cfg(windows)]
    {
        environment.iter().find_map(|(candidate, value)| {
            candidate
                .to_string_lossy()
                .eq_ignore_ascii_case(key)
                .then_some(value)
        })
    }
    #[cfg(not(windows))]
    {
        environment.get(OsStr::new(key))
    }
}

fn remove_env_key(environment: &mut BTreeMap<OsString, OsString>, key: &OsStr) {
    #[cfg(windows)]
    {
        let key = key.to_string_lossy();
        let matches: Vec<OsString> = environment
            .keys()
            .filter(|candidate| {
                candidate
                    .to_string_lossy()
                    .eq_ignore_ascii_case(key.as_ref())
            })
            .cloned()
            .collect();
        for candidate in matches {
            environment.remove(&candidate);
        }
    }
    #[cfg(not(windows))]
    {
        environment.remove(key);
    }
}

fn find_executable(
    environment: &BTreeMap<OsString, OsString>,
    name: &str,
) -> Result<Option<PathBuf>, EnvironmentResolutionError> {
    let Some(path) = env_value(environment, "PATH") else {
        return Ok(None);
    };

    for directory in std::env::split_paths(path) {
        for candidate in executable_candidates(&directory, name, environment) {
            if let Ok(canonical) = canonical_usable_executable(&candidate) {
                return Ok(Some(canonical));
            }
        }
    }
    Ok(None)
}

fn executable_candidates(
    directory: &Path,
    name: &str,
    environment: &BTreeMap<OsString, OsString>,
) -> Vec<PathBuf> {
    let exact = directory.join(name);
    #[cfg(windows)]
    {
        if exact.extension().is_some() {
            return vec![exact];
        }
        let path_ext = env_value(environment, "PATHEXT")
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_owned());
        let mut candidates = vec![exact];
        candidates.extend(
            path_ext
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(|extension| directory.join(format!("{name}{extension}"))),
        );
        candidates
    }
    #[cfg(not(windows))]
    {
        let _ = environment;
        vec![exact]
    }
}

fn canonical_usable_executable(path: &Path) -> Result<PathBuf, io::Error> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || !is_executable(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "path is not a usable executable file",
        ));
    }
    path.canonicalize()
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    true
}

#[derive(Debug, Error)]
pub enum EnvironmentResolutionError {
    #[error("configured Pi executable {path} is not usable: {source}")]
    ConfiguredPiUnavailable { path: PathBuf, source: io::Error },
    #[error("Pi executable was not found in configured, desktop, or probed environments")]
    PiNotFoundInAnyEnvironment,
    #[error(
        "standard npm Pi shim {pi} was found with CLI {cli}, but node.exe was not available in the selected environment"
    )]
    StandardNpmPiNodeUnavailable { pi: PathBuf, cli: PathBuf },
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use super::*;
    use crate::RunId;

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("pi-wizard-environment-{name}-{}", RunId::new()));
            fs::create_dir_all(&root).expect("create fixture");
            Self { root }
        }

        fn executable(&self, directory: &str, name: &str) -> PathBuf {
            let directory = self.root.join(directory);
            fs::create_dir_all(&directory).expect("create bin dir");
            #[cfg(windows)]
            let filename = if name == "git" {
                format!("{name}.exe")
            } else {
                format!("{name}.cmd")
            };
            #[cfg(not(windows))]
            let filename = name.to_owned();
            let path = directory.join(filename);
            File::create(&path).expect("create executable");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut permissions = fs::metadata(&path).expect("metadata").permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&path, permissions).expect("permissions");
            }
            path
        }

        fn path(&self, directories: &[&str]) -> OsString {
            std::env::join_paths(
                directories
                    .iter()
                    .map(|directory| self.root.join(directory)),
            )
            .expect("join path")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn configured_pi_path_wins_without_logging_environment_values() {
        let fixture = Fixture::new("configured");
        let configured_pi = fixture.executable("configured", "pi");
        fixture.executable("desktop", "git");
        let mut desktop = BTreeMap::new();
        desktop.insert(OsString::from("PATH"), fixture.path(&["desktop"]));
        desktop.insert(
            OsString::from("OPENAI_API_KEY"),
            OsString::from("super-secret-value"),
        );

        let resolved = resolve_launch_environment(LaunchEnvironmentInput {
            configured_pi: Some(configured_pi.clone()),
            desktop_environment: desktop,
            ..LaunchEnvironmentInput::default()
        })
        .expect("resolve configured Pi");

        assert_eq!(
            resolved.pi_executable(),
            configured_pi.canonicalize().expect("canonical Pi")
        );
        assert_eq!(
            resolved.diagnostics().pi.source,
            ExecutableSource::Configured
        );
        let debug = format!("{resolved:?}");
        assert!(!debug.contains("super-secret-value"));
        assert!(!debug.contains("OPENAI_API_KEY"));
    }

    #[test]
    fn desktop_path_is_used_when_it_contains_pi() {
        let fixture = Fixture::new("desktop");
        let pi = fixture.executable("desktop", "pi");
        let git = fixture.executable("desktop", "git");
        let mut desktop = BTreeMap::new();
        desktop.insert(OsString::from("PATH"), fixture.path(&["desktop"]));

        let resolved = resolve_launch_environment(LaunchEnvironmentInput {
            desktop_environment: desktop,
            ..LaunchEnvironmentInput::default()
        })
        .expect("desktop resolution");

        assert_eq!(
            resolved.diagnostics().path_source,
            EnvironmentSource::DesktopProcess
        );
        assert_eq!(resolved.pi_executable(), pi.canonicalize().expect("pi"));
        assert_eq!(
            resolved.git_executable(),
            Some(git.canonicalize().expect("git").as_path())
        );
    }

    #[cfg(windows)]
    #[test]
    fn standard_npm_pi_shim_resolves_to_direct_node_cli_invocation() {
        let fixture = Fixture::new("npm-direct");
        let bin = fixture.root.join("npm");
        fs::create_dir_all(
            bin.join("node_modules")
                .join("@earendil-works")
                .join("pi-coding-agent")
                .join("dist")
                .join("bundle"),
        )
        .expect("create npm package layout");
        let pi = bin.join("pi");
        File::create(&pi).expect("extensionless npm Pi shim");
        File::create(bin.join("pi.cmd")).expect("Windows npm Pi shim");
        let node = bin.join("node.exe");
        File::create(&node).expect("node executable");
        let cli = bin
            .join("node_modules")
            .join("@earendil-works")
            .join("pi-coding-agent")
            .join("dist")
            .join("bundle")
            .join("cli.js");
        File::create(&cli).expect("Pi CLI entrypoint");

        let mut desktop = BTreeMap::new();
        desktop.insert(OsString::from("PATH"), fixture.path(&["npm"]));
        desktop.insert(OsString::from("PATHEXT"), OsString::from(".EXE;.CMD"));
        let resolved = resolve_launch_environment(LaunchEnvironmentInput {
            desktop_environment: desktop,
            ..LaunchEnvironmentInput::default()
        })
        .expect("resolve npm Pi");

        assert_eq!(
            resolved.pi_executable(),
            pi.canonicalize().expect("logical Pi")
        );
        assert_eq!(
            resolved.pi_invocation().executable(),
            node.canonicalize().expect("direct Node")
        );
        assert_eq!(
            resolved.pi_invocation().prefix_args(),
            [process_argument_path(
                &cli.canonicalize().expect("canonical Pi CLI entrypoint")
            )]
        );
        assert!(
            !resolved.pi_invocation().prefix_args()[0]
                .to_string_lossy()
                .starts_with(r"\\?\")
        );
        assert!(resolved.pi_invocation().is_direct_npm_node());
    }

    #[cfg(windows)]
    #[test]
    fn standard_npm_pi_shim_without_node_requires_environment_retry() {
        let fixture = Fixture::new("npm-node-missing");
        let bin = fixture.root.join("npm");
        let cli = bin
            .join("node_modules")
            .join("@earendil-works")
            .join("pi-coding-agent")
            .join("dist")
            .join("bundle")
            .join("cli.js");
        fs::create_dir_all(cli.parent().expect("Pi CLI parent")).expect("package layout");
        File::create(bin.join("pi.cmd")).expect("Pi shim");
        File::create(&cli).expect("Pi CLI entrypoint");

        let mut desktop = BTreeMap::new();
        desktop.insert(OsString::from("PATH"), fixture.path(&["npm"]));
        desktop.insert(OsString::from("PATHEXT"), OsString::from(".EXE;.CMD"));
        assert!(matches!(
            resolve_launch_environment(LaunchEnvironmentInput {
                desktop_environment: desktop,
                ..LaunchEnvironmentInput::default()
            }),
            Err(EnvironmentResolutionError::StandardNpmPiNodeUnavailable { .. })
        ));
    }

    #[test]
    fn shell_probe_environment_is_selected_as_one_execution_profile() {
        let fixture = Fixture::new("shell");
        fixture.executable("shell", "pi");
        fixture.executable("shell", "git");
        let mut desktop = BTreeMap::new();
        desktop.insert(OsString::from("PATH"), fixture.path(&["empty"]));
        let mut shell = BTreeMap::new();
        shell.insert(OsString::from("PATH"), fixture.path(&["shell"]));
        shell.insert(
            OsString::from("SHELL_ONLY_TOOLCHAIN_MARKER"),
            OsString::from("present"),
        );

        let resolved = resolve_launch_environment(LaunchEnvironmentInput {
            desktop_environment: desktop,
            shell_probe_environment: Some(shell),
            ..LaunchEnvironmentInput::default()
        })
        .expect("shell fallback");

        assert_eq!(
            resolved.diagnostics().path_source,
            EnvironmentSource::ShellProbe
        );
        assert_eq!(
            env_value(resolved.variables(), "SHELL_ONLY_TOOLCHAIN_MARKER")
                .expect("selected shell environment"),
            &OsString::from("present")
        );
    }

    #[test]
    fn configured_path_override_precedes_desktop_and_shell_paths() {
        let fixture = Fixture::new("configured-path");
        let configured_pi = fixture.executable("configured", "pi");
        fixture.executable("desktop", "pi");
        fixture.executable("shell", "pi");
        let mut configured = BTreeMap::new();
        configured.insert(OsString::from("PATH"), fixture.path(&["configured"]));
        let mut desktop = BTreeMap::new();
        desktop.insert(OsString::from("PATH"), fixture.path(&["desktop"]));
        let mut shell = BTreeMap::new();
        shell.insert(OsString::from("PATH"), fixture.path(&["shell"]));

        let resolved = resolve_launch_environment(LaunchEnvironmentInput {
            configured_environment: configured,
            desktop_environment: desktop,
            shell_probe_environment: Some(shell),
            ..LaunchEnvironmentInput::default()
        })
        .expect("configured PATH");

        assert_eq!(
            resolved.diagnostics().path_source,
            EnvironmentSource::Configured
        );
        assert_eq!(
            resolved.pi_executable(),
            configured_pi.canonicalize().expect("configured Pi")
        );
    }

    #[test]
    fn missing_pi_is_actionable_failure_not_implicit_current_directory_fallback() {
        let fixture = Fixture::new("missing");
        let mut desktop = BTreeMap::new();
        desktop.insert(OsString::from("PATH"), fixture.path(&["empty"]));

        assert!(matches!(
            resolve_launch_environment(LaunchEnvironmentInput {
                desktop_environment: desktop,
                ..LaunchEnvironmentInput::default()
            }),
            Err(EnvironmentResolutionError::PiNotFoundInAnyEnvironment)
        ));
    }

    #[test]
    fn probed_environment_parser_ignores_shell_chatter_before_marker() {
        let mut output = b"profile banner\n".to_vec();
        output.extend_from_slice(SHELL_ENV_MARKER);
        output.extend_from_slice(b"PATH=/tool/bin\0PRIVATE_TOKEN=secret-value\0");
        let environment = parse_probed_environment(&output).expect("parse probe");

        assert_eq!(
            env_value(&environment, "PATH"),
            Some(&OsString::from("/tool/bin"))
        );
        assert_eq!(
            env_value(&environment, "PRIVATE_TOKEN"),
            Some(&OsString::from("secret-value"))
        );
    }

    #[test]
    fn probed_environment_requires_protocol_marker_and_path() {
        assert!(matches!(
            parse_probed_environment(b"PATH=/bin\0"),
            Err(EnvironmentProbeError::MissingMarker)
        ));
        let mut output = SHELL_ENV_MARKER.to_vec();
        output.extend_from_slice(b"HOME=/home/test\0");
        assert!(matches!(
            parse_probed_environment(&output),
            Err(EnvironmentProbeError::MissingPath)
        ));
    }
}
