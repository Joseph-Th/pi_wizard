use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectResourcePreflight {
    pub pi_settings: bool,
    pub extensions: bool,
    pub skills: bool,
    pub prompts: bool,
    pub themes: bool,
    pub system_prompt: bool,
    pub append_system_prompt: bool,
    pub ancestor_agent_skills: bool,
}

impl ProjectResourcePreflight {
    #[must_use]
    pub const fn has_protected_resources(&self) -> bool {
        self.pi_settings
            || self.extensions
            || self.skills
            || self.prompts
            || self.themes
            || self.system_prompt
            || self.append_system_prompt
            || self.ancestor_agent_skills
    }
}

#[derive(Debug, Error)]
pub enum ProjectResourcePreflightError {
    #[error("project path could not be canonicalized: {0}")]
    ProjectPath(std::io::Error),
    #[error("could not inspect Pi project resource {path}: {source}")]
    ResourceMetadata {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub fn inspect_project_resources(
    project_root: &Path,
) -> Result<ProjectResourcePreflight, ProjectResourcePreflightError> {
    let project_root = project_root
        .canonicalize()
        .map_err(ProjectResourcePreflightError::ProjectPath)?;
    let pi = project_root.join(".pi");
    let mut ancestor_agent_skills = false;
    for ancestor in project_root.ancestors() {
        if resource_exists(&ancestor.join(".agents").join("skills"))? {
            ancestor_agent_skills = true;
            break;
        }
    }

    Ok(ProjectResourcePreflight {
        pi_settings: resource_exists(&pi.join("settings.json"))?,
        extensions: resource_exists(&pi.join("extensions"))?,
        skills: resource_exists(&pi.join("skills"))?,
        prompts: resource_exists(&pi.join("prompts"))?,
        themes: resource_exists(&pi.join("themes"))?,
        system_prompt: resource_exists(&pi.join("SYSTEM.md"))?,
        append_system_prompt: resource_exists(&pi.join("APPEND_SYSTEM.md"))?,
        ancestor_agent_skills,
    })
}

fn resource_exists(path: &Path) -> Result<bool, ProjectResourcePreflightError> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(ProjectResourcePreflightError::ResourceMetadata {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunId;

    fn fixture() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("pi-wizard-trust-preflight-{}", RunId::new()));
        let project = root.join("parent").join("project");
        fs::create_dir_all(&project).expect("project fixture");
        (root, project)
    }

    #[test]
    fn detects_only_documented_protected_project_resource_locations() {
        let (root, project) = fixture();
        fs::create_dir_all(project.join(".pi").join("extensions")).expect("extensions");
        fs::write(project.join(".pi").join("SYSTEM.md"), "system").expect("system prompt");
        fs::create_dir_all(root.join("parent").join(".agents").join("skills"))
            .expect("ancestor skills");
        fs::write(project.join("AGENTS.md"), "context only").expect("context file");

        let preflight = inspect_project_resources(&project).expect("inspect resources");
        assert!(preflight.has_protected_resources());
        assert!(preflight.extensions);
        assert!(preflight.system_prompt);
        assert!(preflight.ancestor_agent_skills);
        assert!(!preflight.pi_settings);
        assert!(!preflight.skills);
        fs::remove_dir_all(root).expect("cleanup fixture");
    }

    #[test]
    fn bare_pi_and_context_files_do_not_count_as_protected_resources() {
        let (root, project) = fixture();
        fs::create_dir_all(project.join(".pi")).expect("bare pi");
        fs::write(project.join("AGENTS.md"), "context").expect("context");
        fs::write(project.join("CLAUDE.md"), "context").expect("context");

        let preflight = inspect_project_resources(&project).expect("inspect resources");
        assert!(!preflight.has_protected_resources());
        fs::remove_dir_all(root).expect("cleanup fixture");
    }
}
