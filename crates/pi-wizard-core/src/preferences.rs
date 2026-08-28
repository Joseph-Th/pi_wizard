use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use atomic_write_file::AtomicWriteFile;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{RunId, RuntimeLimits};

pub const PREFERENCES_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedPreferences {
    schema_version: u32,
    live_run_limit: usize,
}

/// Recoverable app-owned preferences. Preferences influence orchestration but
/// never own Pi session, process, project, or Git state.
#[derive(Debug)]
pub struct PreferencesStore {
    preferences_path: Option<PathBuf>,
    quarantine_dir: Option<PathBuf>,
    limits: RuntimeLimits,
    live_run_limit: usize,
    recovery_notice: Option<String>,
}

impl PreferencesStore {
    #[must_use]
    pub fn ephemeral(limits: RuntimeLimits) -> Self {
        Self {
            preferences_path: None,
            quarantine_dir: None,
            limits,
            live_run_limit: limits.max_live_runs,
            recovery_notice: None,
        }
    }

    pub fn open(root: impl AsRef<Path>, limits: RuntimeLimits) -> Result<Self, PreferencesError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|source| PreferencesError::CreateDirectory {
            path: root.to_path_buf(),
            source,
        })?;
        let preferences_path = root.join("preferences.json");
        let quarantine_dir = root.join("preferences-quarantine");
        let mut store = Self {
            preferences_path: Some(preferences_path),
            quarantine_dir: Some(quarantine_dir),
            limits,
            live_run_limit: limits.max_live_runs,
            recovery_notice: None,
        };
        match store.load_from_disk() {
            Ok(()) => Ok(store),
            Err(PreferencesError::Read { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(store)
            }
            Err(error) if error.is_recoverable_corruption() => {
                let notice = error.to_string();
                store.quarantine_corrupt_preferences(error)?;
                store.live_run_limit = limits.max_live_runs;
                store.recovery_notice = Some(notice);
                Ok(store)
            }
            Err(error) => Err(error),
        }
    }

    #[must_use]
    pub fn live_run_limit(&self) -> usize {
        self.live_run_limit
    }

    #[must_use]
    pub fn recovery_notice(&self) -> Option<&str> {
        self.recovery_notice.as_deref()
    }

    pub fn set_live_run_limit(&mut self, limit: usize) -> Result<(), PreferencesError> {
        self.validate_live_run_limit(limit)?;
        self.persist(limit)?;
        self.live_run_limit = limit;
        self.recovery_notice = None;
        Ok(())
    }

    fn validate_live_run_limit(&self, limit: usize) -> Result<(), PreferencesError> {
        if limit == 0 || limit > self.limits.max_live_runs {
            return Err(PreferencesError::InvalidLiveRunLimit {
                value: limit,
                maximum: self.limits.max_live_runs,
            });
        }
        Ok(())
    }

    fn load_from_disk(&mut self) -> Result<(), PreferencesError> {
        let path = self
            .preferences_path
            .as_ref()
            .expect("persistent preferences path");
        let metadata = fs::metadata(path).map_err(|source| PreferencesError::Read {
            path: path.clone(),
            source,
        })?;
        let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if size > self.limits.max_preferences_bytes {
            return Err(PreferencesError::ByteLimit {
                attempted: size,
                limit: self.limits.max_preferences_bytes,
            });
        }
        let bytes = fs::read(path).map_err(|source| PreferencesError::Read {
            path: path.clone(),
            source,
        })?;
        let persisted: PersistedPreferences =
            serde_json::from_slice(&bytes).map_err(PreferencesError::Decode)?;
        if persisted.schema_version != PREFERENCES_SCHEMA_VERSION {
            return Err(PreferencesError::UnsupportedSchema {
                actual: persisted.schema_version,
                supported: PREFERENCES_SCHEMA_VERSION,
            });
        }
        self.validate_live_run_limit(persisted.live_run_limit)?;
        self.live_run_limit = persisted.live_run_limit;
        Ok(())
    }

    fn persist(&self, live_run_limit: usize) -> Result<(), PreferencesError> {
        let Some(path) = self.preferences_path.as_ref() else {
            return Ok(());
        };
        let encoded = serde_json::to_vec(&PersistedPreferences {
            schema_version: PREFERENCES_SCHEMA_VERSION,
            live_run_limit,
        })
        .map_err(PreferencesError::Encode)?;
        if encoded.len() > self.limits.max_preferences_bytes {
            return Err(PreferencesError::ByteLimit {
                attempted: encoded.len(),
                limit: self.limits.max_preferences_bytes,
            });
        }
        let mut file = AtomicWriteFile::options().open(path).map_err(|source| {
            PreferencesError::OpenAtomic {
                path: path.clone(),
                source,
            }
        })?;
        file.write_all(&encoded)
            .map_err(|source| PreferencesError::Write {
                path: path.clone(),
                source,
            })?;
        file.commit().map_err(|source| PreferencesError::Commit {
            path: path.clone(),
            source,
        })
    }

    fn quarantine_corrupt_preferences(
        &self,
        cause: PreferencesError,
    ) -> Result<(), PreferencesError> {
        let path = self
            .preferences_path
            .as_ref()
            .expect("persistent preferences path");
        let quarantine_dir = self
            .quarantine_dir
            .as_ref()
            .expect("persistent preferences quarantine directory");
        if let Err(source) = fs::create_dir_all(quarantine_dir) {
            return Err(PreferencesError::QuarantineFailed {
                path: path.clone(),
                cause: Box::new(cause),
                source,
            });
        }
        let quarantine_path = quarantine_dir.join(format!("{}-preferences.json", RunId::new()));
        fs::rename(path, quarantine_path).map_err(|source| PreferencesError::QuarantineFailed {
            path: path.clone(),
            cause: Box::new(cause),
            source,
        })
    }
}

#[derive(Debug, Error)]
pub enum PreferencesError {
    #[error("live run preference {value} must be between 1 and configured maximum {maximum}")]
    InvalidLiveRunLimit { value: usize, maximum: usize },
    #[error("preferences use {attempted} bytes, exceeding limit {limit}")]
    ByteLimit { attempted: usize, limit: usize },
    #[error("preferences use schema {actual}; supported schema is {supported}")]
    UnsupportedSchema { actual: u32, supported: u32 },
    #[error("could not create preferences directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not read preferences {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not decode preferences: {0}")]
    Decode(serde_json::Error),
    #[error("could not encode preferences: {0}")]
    Encode(serde_json::Error),
    #[error("could not open atomic preferences file {path}: {source}")]
    OpenAtomic {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write preferences {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not commit preferences {path}: {source}")]
    Commit {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid preferences {path} could not be quarantined: {cause}; {source}")]
    QuarantineFailed {
        path: PathBuf,
        cause: Box<PreferencesError>,
        source: std::io::Error,
    },
}

impl PreferencesError {
    fn is_recoverable_corruption(&self) -> bool {
        matches!(
            self,
            Self::InvalidLiveRunLimit { .. }
                | Self::ByteLimit { .. }
                | Self::UnsupportedSchema { .. }
                | Self::Decode(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("pi-wizard-preferences-{name}-{}", RunId::new()));
        fs::create_dir_all(&root).expect("fixture root");
        root
    }

    #[test]
    fn live_run_limit_round_trips_across_reopen() {
        let root = fixture("round-trip");
        let limits = RuntimeLimits {
            max_live_runs: 8,
            ..RuntimeLimits::default()
        };
        let mut preferences = PreferencesStore::open(&root, limits).expect("preferences");
        preferences.set_live_run_limit(3).expect("save limit");
        drop(preferences);

        let reopened = PreferencesStore::open(&root, limits).expect("reopen");
        assert_eq!(reopened.live_run_limit(), 3);
        assert!(reopened.recovery_notice().is_none());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn corrupt_or_out_of_range_preferences_are_quarantined_to_safe_default() {
        let root = fixture("quarantine");
        fs::write(root.join("preferences.json"), b"{not-json").expect("corrupt preferences");
        let recovered = PreferencesStore::open(&root, RuntimeLimits::default()).expect("recover");
        assert_eq!(
            recovered.live_run_limit(),
            RuntimeLimits::default().max_live_runs
        );
        assert!(recovered.recovery_notice().is_some());
        assert!(!root.join("preferences.json").exists());
        assert!(root.join("preferences-quarantine").is_dir());
        fs::remove_dir_all(root).expect("cleanup");

        let root = fixture("out-of-range");
        fs::write(
            root.join("preferences.json"),
            br#"{"schemaVersion":1,"liveRunLimit":999}"#,
        )
        .expect("invalid preferences");
        let recovered = PreferencesStore::open(&root, RuntimeLimits::default()).expect("recover");
        assert_eq!(
            recovered.live_run_limit(),
            RuntimeLimits::default().max_live_runs
        );
        assert!(recovered.recovery_notice().is_some());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn failed_preference_write_does_not_mutate_in_memory_value() {
        let root = fixture("write-failure");
        let mut preferences =
            PreferencesStore::open(&root, RuntimeLimits::default()).expect("open");
        let original = preferences.live_run_limit();
        fs::create_dir_all(root.join("preferences.json")).expect("block preference file");
        assert!(preferences.set_live_run_limit(2).is_err());
        assert_eq!(preferences.live_run_limit(), original);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn oversized_preferences_are_quarantined_independently() {
        let root = fixture("oversized");
        fs::write(root.join("preferences.json"), vec![b'x'; 128]).expect("oversized file");
        let limits = RuntimeLimits {
            max_preferences_bytes: 64,
            ..RuntimeLimits::default()
        };
        let recovered = PreferencesStore::open(&root, limits).expect("recover oversized");
        assert_eq!(recovered.live_run_limit(), limits.max_live_runs);
        assert!(recovered.recovery_notice().is_some());
        fs::remove_dir_all(root).expect("cleanup");
    }
}
