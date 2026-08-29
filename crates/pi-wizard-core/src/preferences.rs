use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use atomic_write_file::AtomicWriteFile;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{RunId, RuntimeLimits};

pub const PREFERENCES_SCHEMA_VERSION: u32 = 2;
pub const DEFAULT_NEW_RUN_MODEL_PROVIDER: &str = "opencode-go";
pub const DEFAULT_NEW_RUN_MODEL_ID: &str = "muse-spark-1.2-contributor";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPreference {
    pub provider: String,
    pub model: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPreferencesSnapshot {
    pub new_run_model: Option<ModelPreference>,
    pub favorite_models: Vec<ModelPreference>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedPreferencesV1 {
    schema_version: u32,
    live_run_limit: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedPreferences {
    schema_version: u32,
    live_run_limit: usize,
    new_run_model: Option<ModelPreference>,
    #[serde(default)]
    favorite_models: Vec<ModelPreference>,
}

/// Recoverable app-owned preferences. Preferences influence orchestration but
/// never own Pi session, process, project, or Git state.
#[derive(Debug)]
pub struct PreferencesStore {
    preferences_path: Option<PathBuf>,
    quarantine_dir: Option<PathBuf>,
    limits: RuntimeLimits,
    live_run_limit: usize,
    new_run_model: Option<ModelPreference>,
    favorite_models: BTreeSet<ModelPreference>,
    recovery_notice: Option<String>,
}

impl PreferencesStore {
    fn default_new_run_model() -> ModelPreference {
        ModelPreference {
            provider: DEFAULT_NEW_RUN_MODEL_PROVIDER.to_owned(),
            model: DEFAULT_NEW_RUN_MODEL_ID.to_owned(),
        }
    }

    #[must_use]
    pub fn ephemeral(limits: RuntimeLimits) -> Self {
        Self {
            preferences_path: None,
            quarantine_dir: None,
            limits,
            live_run_limit: limits.max_live_runs,
            new_run_model: Some(Self::default_new_run_model()),
            favorite_models: BTreeSet::new(),
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
            new_run_model: Some(Self::default_new_run_model()),
            favorite_models: BTreeSet::new(),
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
                store.new_run_model = Some(Self::default_new_run_model());
                store.favorite_models.clear();
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

    #[must_use]
    pub fn model_preferences(&self) -> ModelPreferencesSnapshot {
        ModelPreferencesSnapshot {
            new_run_model: self.new_run_model.clone(),
            favorite_models: self.favorite_models.iter().cloned().collect(),
        }
    }

    pub fn set_live_run_limit(&mut self, limit: usize) -> Result<(), PreferencesError> {
        self.validate_live_run_limit(limit)?;
        self.persist(limit, &self.new_run_model, &self.favorite_models)?;
        self.live_run_limit = limit;
        self.recovery_notice = None;
        Ok(())
    }

    pub fn set_new_run_model(
        &mut self,
        model: Option<ModelPreference>,
    ) -> Result<ModelPreferencesSnapshot, PreferencesError> {
        let model = model
            .map(|model| self.normalize_model_preference(model))
            .transpose()?;
        self.persist(self.live_run_limit, &model, &self.favorite_models)?;
        self.new_run_model = model;
        self.recovery_notice = None;
        Ok(self.model_preferences())
    }

    pub fn set_model_favorite(
        &mut self,
        model: ModelPreference,
        favorite: bool,
    ) -> Result<ModelPreferencesSnapshot, PreferencesError> {
        let model = self.normalize_model_preference(model)?;
        let mut favorites = self.favorite_models.clone();
        if favorite {
            if !favorites.contains(&model)
                && favorites.len() >= self.limits.max_custom_model_profiles
            {
                return Err(PreferencesError::FavoriteModelLimit {
                    limit: self.limits.max_custom_model_profiles,
                });
            }
            favorites.insert(model);
        } else {
            favorites.remove(&model);
        }
        self.persist(self.live_run_limit, &self.new_run_model, &favorites)?;
        self.favorite_models = favorites;
        self.recovery_notice = None;
        Ok(self.model_preferences())
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

    fn normalize_model_preference(
        &self,
        mut model: ModelPreference,
    ) -> Result<ModelPreference, PreferencesError> {
        model.provider = model.provider.trim().to_owned();
        model.model = model.model.trim().to_owned();
        if model.provider.is_empty() {
            return Err(PreferencesError::EmptyModelProvider);
        }
        if model.model.is_empty() {
            return Err(PreferencesError::EmptyModelId);
        }
        for (field, value) in [("provider", &model.provider), ("model", &model.model)] {
            if value.len() > self.limits.max_custom_model_field_bytes {
                return Err(PreferencesError::ModelFieldTooLarge {
                    field,
                    attempted: value.len(),
                    limit: self.limits.max_custom_model_field_bytes,
                });
            }
        }
        Ok(model)
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
        let schema = serde_json::from_slice::<serde_json::Value>(&bytes)
            .map_err(PreferencesError::Decode)?
            .get("schemaVersion")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or(PreferencesError::MissingSchemaVersion)?;
        match schema {
            1 => {
                let persisted: PersistedPreferencesV1 =
                    serde_json::from_slice(&bytes).map_err(PreferencesError::Decode)?;
                debug_assert_eq!(persisted.schema_version, 1);
                self.validate_live_run_limit(persisted.live_run_limit)?;
                self.live_run_limit = persisted.live_run_limit;
                self.new_run_model = Some(Self::default_new_run_model());
                self.favorite_models.clear();
            }
            PREFERENCES_SCHEMA_VERSION => {
                let persisted: PersistedPreferences =
                    serde_json::from_slice(&bytes).map_err(PreferencesError::Decode)?;
                self.validate_live_run_limit(persisted.live_run_limit)?;
                let new_run_model = persisted
                    .new_run_model
                    .map(|model| self.normalize_model_preference(model))
                    .transpose()?;
                if persisted.favorite_models.len() > self.limits.max_custom_model_profiles {
                    return Err(PreferencesError::FavoriteModelLimit {
                        limit: self.limits.max_custom_model_profiles,
                    });
                }
                let mut favorites = BTreeSet::new();
                for model in persisted.favorite_models {
                    let model = self.normalize_model_preference(model)?;
                    if !favorites.insert(model) {
                        return Err(PreferencesError::DuplicateFavoriteModel);
                    }
                }
                self.live_run_limit = persisted.live_run_limit;
                self.new_run_model = new_run_model;
                self.favorite_models = favorites;
            }
            actual => {
                return Err(PreferencesError::UnsupportedSchema {
                    actual,
                    supported: PREFERENCES_SCHEMA_VERSION,
                });
            }
        }
        Ok(())
    }

    fn persist(
        &self,
        live_run_limit: usize,
        new_run_model: &Option<ModelPreference>,
        favorite_models: &BTreeSet<ModelPreference>,
    ) -> Result<(), PreferencesError> {
        let Some(path) = self.preferences_path.as_ref() else {
            return Ok(());
        };
        let encoded = serde_json::to_vec(&PersistedPreferences {
            schema_version: PREFERENCES_SCHEMA_VERSION,
            live_run_limit,
            new_run_model: new_run_model.clone(),
            favorite_models: favorite_models.iter().cloned().collect(),
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
    #[error("preferences are missing a valid schemaVersion")]
    MissingSchemaVersion,
    #[error("model preference provider cannot be empty")]
    EmptyModelProvider,
    #[error("model preference id cannot be empty")]
    EmptyModelId,
    #[error("model preference {field} uses {attempted} bytes, exceeding limit {limit}")]
    ModelFieldTooLarge {
        field: &'static str,
        attempted: usize,
        limit: usize,
    },
    #[error("favorite model preference limit {limit} reached")]
    FavoriteModelLimit { limit: usize },
    #[error("favorite model preferences contain a duplicate provider/model identity")]
    DuplicateFavoriteModel,
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
                | Self::MissingSchemaVersion
                | Self::EmptyModelProvider
                | Self::EmptyModelId
                | Self::ModelFieldTooLarge { .. }
                | Self::FavoriteModelLimit { .. }
                | Self::DuplicateFavoriteModel
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
    fn new_run_model_defaults_to_muse_and_selection_and_favorites_round_trip() {
        let root = fixture("model-round-trip");
        let limits = RuntimeLimits::default();
        let mut preferences = PreferencesStore::open(&root, limits).expect("preferences");
        assert_eq!(
            preferences.model_preferences().new_run_model,
            Some(ModelPreference {
                provider: DEFAULT_NEW_RUN_MODEL_PROVIDER.to_owned(),
                model: DEFAULT_NEW_RUN_MODEL_ID.to_owned(),
            })
        );

        let chosen = ModelPreference {
            provider: "openai".to_owned(),
            model: "gpt-5.6".to_owned(),
        };
        preferences
            .set_new_run_model(Some(chosen.clone()))
            .expect("save last model");
        preferences
            .set_model_favorite(chosen.clone(), true)
            .expect("favorite model");
        preferences
            .set_model_favorite(
                ModelPreference {
                    provider: DEFAULT_NEW_RUN_MODEL_PROVIDER.to_owned(),
                    model: DEFAULT_NEW_RUN_MODEL_ID.to_owned(),
                },
                true,
            )
            .expect("favorite Muse");
        drop(preferences);

        let reopened = PreferencesStore::open(&root, limits).expect("reopen");
        let snapshot = reopened.model_preferences();
        assert_eq!(snapshot.new_run_model, Some(chosen));
        assert_eq!(snapshot.favorite_models.len(), 2);
        assert!(snapshot.favorite_models.iter().any(|model| {
            model.provider == DEFAULT_NEW_RUN_MODEL_PROVIDER
                && model.model == DEFAULT_NEW_RUN_MODEL_ID
        }));
        assert!(
            snapshot
                .favorite_models
                .iter()
                .any(|model| { model.provider == "openai" && model.model == "gpt-5.6" })
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn explicit_pi_default_new_run_model_round_trips_as_null() {
        let root = fixture("pi-default-model");
        let limits = RuntimeLimits::default();
        let mut preferences = PreferencesStore::open(&root, limits).expect("preferences");
        preferences
            .set_new_run_model(None)
            .expect("remember Pi default");
        drop(preferences);

        let reopened = PreferencesStore::open(&root, limits).expect("reopen");
        assert!(reopened.model_preferences().new_run_model.is_none());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn schema_one_preferences_migrate_in_memory_and_write_schema_two() {
        let root = fixture("schema-one-model-default");
        fs::write(
            root.join("preferences.json"),
            br#"{"schemaVersion":1,"liveRunLimit":3}"#,
        )
        .expect("schema one preferences");
        let limits = RuntimeLimits::default();
        let mut preferences = PreferencesStore::open(&root, limits).expect("migrate");
        assert_eq!(preferences.live_run_limit(), 3);
        assert_eq!(
            preferences
                .model_preferences()
                .new_run_model
                .as_ref()
                .map(|model| (model.provider.as_str(), model.model.as_str())),
            Some((DEFAULT_NEW_RUN_MODEL_PROVIDER, DEFAULT_NEW_RUN_MODEL_ID))
        );
        preferences
            .set_live_run_limit(4)
            .expect("write current schema");
        let persisted: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join("preferences.json")).expect("read migrated preferences"),
        )
        .expect("decode migrated preferences");
        assert_eq!(
            persisted
                .get("schemaVersion")
                .and_then(|value| value.as_u64()),
            Some(2)
        );
        assert_eq!(
            persisted
                .get("liveRunLimit")
                .and_then(|value| value.as_u64()),
            Some(4)
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn failed_model_preference_write_does_not_mutate_in_memory_value() {
        let root = fixture("model-write-failure");
        let mut preferences =
            PreferencesStore::open(&root, RuntimeLimits::default()).expect("open");
        let original = preferences.model_preferences();
        fs::create_dir_all(root.join("preferences.json")).expect("block preference file");
        assert!(
            preferences
                .set_new_run_model(Some(ModelPreference {
                    provider: "openai".to_owned(),
                    model: "gpt-5.6".to_owned(),
                }))
                .is_err()
        );
        assert_eq!(preferences.model_preferences(), original);
        fs::remove_dir_all(root).expect("cleanup");
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
