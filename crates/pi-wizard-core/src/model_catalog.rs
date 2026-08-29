use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use atomic_write_file::AtomicWriteFile;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::RuntimeLimits;

pub const MODEL_CATALOG_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomModelProfile {
    pub provider: String,
    pub model: String,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogSnapshot {
    pub models: Vec<CustomModelProfile>,
    pub recovery_notice: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedModelCatalog {
    schema_version: u32,
    models: Vec<CustomModelProfile>,
}

#[derive(Debug)]
pub struct ModelCatalogStore {
    path: Option<PathBuf>,
    quarantine_dir: Option<PathBuf>,
    limits: RuntimeLimits,
    models: BTreeMap<(String, String), CustomModelProfile>,
    recovery_notice: Option<String>,
}

impl ModelCatalogStore {
    #[must_use]
    pub fn ephemeral(limits: RuntimeLimits) -> Self {
        Self {
            path: None,
            quarantine_dir: None,
            limits,
            models: BTreeMap::new(),
            recovery_notice: None,
        }
    }

    pub fn open(root: impl AsRef<Path>, limits: RuntimeLimits) -> Result<Self, ModelCatalogError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|source| ModelCatalogError::CreateDirectory {
            path: root.to_path_buf(),
            source,
        })?;
        let mut store = Self {
            path: Some(root.join("model-profiles.json")),
            quarantine_dir: Some(root.join("model-profiles-quarantine")),
            limits,
            models: BTreeMap::new(),
            recovery_notice: None,
        };
        match store.load_from_disk() {
            Ok(()) => Ok(store),
            Err(ModelCatalogError::Read { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(store)
            }
            Err(error) if error.is_recoverable_corruption() => {
                let notice = error.to_string();
                store.quarantine_corrupt_file(error)?;
                store.models.clear();
                store.recovery_notice = Some(notice);
                Ok(store)
            }
            Err(error) => Err(error),
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> ModelCatalogSnapshot {
        ModelCatalogSnapshot {
            models: self.models.values().cloned().collect(),
            recovery_notice: self.recovery_notice.clone(),
        }
    }

    pub fn upsert(
        &mut self,
        mut profile: CustomModelProfile,
    ) -> Result<CustomModelProfile, ModelCatalogError> {
        normalize_profile(&mut profile);
        self.validate_profile(&profile)?;
        let key = (profile.provider.clone(), profile.model.clone());
        if !self.models.contains_key(&key)
            && self.models.len() >= self.limits.max_custom_model_profiles
        {
            return Err(ModelCatalogError::ProfileLimit {
                limit: self.limits.max_custom_model_profiles,
            });
        }
        let mut candidate = self.models.clone();
        candidate.insert(key, profile.clone());
        self.persist(&candidate)?;
        self.models = candidate;
        self.recovery_notice = None;
        Ok(profile)
    }

    pub fn remove(&mut self, provider: &str, model: &str) -> Result<bool, ModelCatalogError> {
        let key = (provider.trim().to_owned(), model.trim().to_owned());
        if !self.models.contains_key(&key) {
            return Ok(false);
        }
        let mut candidate = self.models.clone();
        candidate.remove(&key);
        self.persist(&candidate)?;
        self.models = candidate;
        self.recovery_notice = None;
        Ok(true)
    }

    fn validate_profile(&self, profile: &CustomModelProfile) -> Result<(), ModelCatalogError> {
        if profile.provider.is_empty() {
            return Err(ModelCatalogError::EmptyProvider);
        }
        if profile.model.is_empty() {
            return Err(ModelCatalogError::EmptyModel);
        }
        validate_field(
            "provider",
            &profile.provider,
            self.limits.max_custom_model_field_bytes,
        )?;
        validate_field(
            "model",
            &profile.model,
            self.limits.max_custom_model_field_bytes,
        )?;
        if let Some(name) = &profile.name {
            validate_field("name", name, self.limits.max_custom_model_field_bytes)?;
        }
        Ok(())
    }

    fn load_from_disk(&mut self) -> Result<(), ModelCatalogError> {
        let path = self.path.as_ref().expect("persistent model catalog path");
        let metadata = fs::metadata(path).map_err(|source| ModelCatalogError::Read {
            path: path.clone(),
            source,
        })?;
        let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if size > self.limits.max_custom_model_state_bytes {
            return Err(ModelCatalogError::ByteLimit {
                attempted: size,
                limit: self.limits.max_custom_model_state_bytes,
            });
        }
        let bytes = fs::read(path).map_err(|source| ModelCatalogError::Read {
            path: path.clone(),
            source,
        })?;
        let persisted: PersistedModelCatalog =
            serde_json::from_slice(&bytes).map_err(ModelCatalogError::Decode)?;
        if persisted.schema_version != MODEL_CATALOG_SCHEMA_VERSION {
            return Err(ModelCatalogError::UnsupportedSchema {
                actual: persisted.schema_version,
                supported: MODEL_CATALOG_SCHEMA_VERSION,
            });
        }
        if persisted.models.len() > self.limits.max_custom_model_profiles {
            return Err(ModelCatalogError::ProfileLimit {
                limit: self.limits.max_custom_model_profiles,
            });
        }
        let mut models = BTreeMap::new();
        for mut profile in persisted.models {
            normalize_profile(&mut profile);
            self.validate_profile(&profile)?;
            let key = (profile.provider.clone(), profile.model.clone());
            if models.insert(key, profile).is_some() {
                return Err(ModelCatalogError::DuplicateIdentity);
            }
        }
        self.models = models;
        Ok(())
    }

    fn persist(
        &self,
        candidate: &BTreeMap<(String, String), CustomModelProfile>,
    ) -> Result<(), ModelCatalogError> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let encoded = serde_json::to_vec(&PersistedModelCatalog {
            schema_version: MODEL_CATALOG_SCHEMA_VERSION,
            models: candidate.values().cloned().collect(),
        })
        .map_err(ModelCatalogError::Encode)?;
        if encoded.len() > self.limits.max_custom_model_state_bytes {
            return Err(ModelCatalogError::ByteLimit {
                attempted: encoded.len(),
                limit: self.limits.max_custom_model_state_bytes,
            });
        }
        let mut file = AtomicWriteFile::options().open(path).map_err(|source| {
            ModelCatalogError::OpenAtomic {
                path: path.clone(),
                source,
            }
        })?;
        file.write_all(&encoded)
            .map_err(|source| ModelCatalogError::Write {
                path: path.clone(),
                source,
            })?;
        file.commit().map_err(|source| ModelCatalogError::Commit {
            path: path.clone(),
            source,
        })
    }

    fn quarantine_corrupt_file(&self, cause: ModelCatalogError) -> Result<(), ModelCatalogError> {
        let path = self.path.as_ref().expect("persistent model catalog path");
        let quarantine_dir = self
            .quarantine_dir
            .as_ref()
            .expect("persistent model catalog quarantine directory");
        if let Err(source) = fs::create_dir_all(quarantine_dir) {
            return Err(ModelCatalogError::QuarantineFailed {
                path: path.clone(),
                cause: Box::new(cause),
                source,
            });
        }
        let quarantine_path = quarantine_dir.join(format!(
            "{}-model-profiles.json",
            crate::AutomationExecutionId::new()
        ));
        fs::rename(path, quarantine_path).map_err(|source| ModelCatalogError::QuarantineFailed {
            path: path.clone(),
            cause: Box::new(cause),
            source,
        })
    }
}

fn normalize_profile(profile: &mut CustomModelProfile) {
    profile.provider = profile.provider.trim().to_owned();
    profile.model = profile.model.trim().to_owned();
    profile.name = profile
        .name
        .take()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty());
}

fn validate_field(field: &'static str, value: &str, limit: usize) -> Result<(), ModelCatalogError> {
    if value.len() > limit {
        return Err(ModelCatalogError::FieldTooLarge {
            field,
            actual: value.len(),
            limit,
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ModelCatalogError {
    #[error("custom model provider cannot be empty")]
    EmptyProvider,
    #[error("custom model id cannot be empty")]
    EmptyModel,
    #[error("custom model {field} uses {actual} bytes, exceeding limit {limit}")]
    FieldTooLarge {
        field: &'static str,
        actual: usize,
        limit: usize,
    },
    #[error("custom model profile limit {limit} reached")]
    ProfileLimit { limit: usize },
    #[error("custom model catalog uses {attempted} bytes, exceeding limit {limit}")]
    ByteLimit { attempted: usize, limit: usize },
    #[error("custom model catalog uses schema {actual}; supported schema is {supported}")]
    UnsupportedSchema { actual: u32, supported: u32 },
    #[error("custom model catalog contains a duplicate provider/model identity")]
    DuplicateIdentity,
    #[error("could not create custom model state directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not read custom model catalog {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not decode custom model catalog: {0}")]
    Decode(serde_json::Error),
    #[error("could not encode custom model catalog: {0}")]
    Encode(serde_json::Error),
    #[error("could not open atomic custom model file {path}: {source}")]
    OpenAtomic {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write custom model catalog {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not commit custom model catalog {path}: {source}")]
    Commit {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid custom model catalog {path} could not be quarantined: {cause}; {source}")]
    QuarantineFailed {
        path: PathBuf,
        cause: Box<ModelCatalogError>,
        source: std::io::Error,
    },
}

impl ModelCatalogError {
    fn is_recoverable_corruption(&self) -> bool {
        matches!(
            self,
            Self::EmptyProvider
                | Self::EmptyModel
                | Self::FieldTooLarge { .. }
                | Self::ProfileLimit { .. }
                | Self::ByteLimit { .. }
                | Self::UnsupportedSchema { .. }
                | Self::DuplicateIdentity
                | Self::Decode(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pi-wizard-model-catalog-{name}-{}",
            crate::AutomationExecutionId::new()
        ));
        fs::create_dir_all(&root).expect("fixture root");
        root
    }

    fn profile(provider: &str, model: &str, name: Option<&str>) -> CustomModelProfile {
        CustomModelProfile {
            provider: provider.to_owned(),
            model: model.to_owned(),
            name: name.map(str::to_owned),
        }
    }

    #[test]
    fn custom_models_round_trip_and_same_identity_replaces_metadata() {
        let root = fixture("round-trip");
        let limits = RuntimeLimits::default();
        let mut store = ModelCatalogStore::open(&root, limits).expect("open");
        store
            .upsert(profile(" provider ", " model ", Some(" First ")))
            .expect("save");
        store
            .upsert(profile("provider", "model", Some("Second")))
            .expect("replace");
        assert_eq!(
            store.snapshot().models,
            [profile("provider", "model", Some("Second"))]
        );
        drop(store);
        let reopened = ModelCatalogStore::open(&root, limits).expect("reopen");
        assert_eq!(
            reopened.snapshot().models,
            [profile("provider", "model", Some("Second"))]
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn custom_model_validation_is_non_destructive_and_bounded() {
        let limits = RuntimeLimits {
            max_custom_model_field_bytes: 4,
            ..RuntimeLimits::default()
        };
        let mut store = ModelCatalogStore::ephemeral(limits);
        store.upsert(profile("good", "m", None)).expect("baseline");
        assert!(matches!(
            store.upsert(profile("oversized", "m", None)),
            Err(ModelCatalogError::FieldTooLarge {
                field: "provider",
                ..
            })
        ));
        assert_eq!(store.snapshot().models, [profile("good", "m", None)]);
    }

    #[test]
    fn corrupt_custom_model_catalog_is_quarantined_independently() {
        let root = fixture("quarantine");
        fs::write(root.join("model-profiles.json"), b"{broken").expect("corrupt");
        let recovered = ModelCatalogStore::open(&root, RuntimeLimits::default()).expect("recover");
        assert!(recovered.snapshot().models.is_empty());
        assert!(recovered.snapshot().recovery_notice.is_some());
        assert!(root.join("model-profiles-quarantine").is_dir());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn aggregate_custom_model_state_limit_is_enforced() {
        let root = fixture("aggregate");
        let limits = RuntimeLimits {
            max_custom_model_state_bytes: 100,
            ..RuntimeLimits::default()
        };
        let mut store = ModelCatalogStore::open(&root, limits).expect("open");
        let result = store.upsert(profile("provider", "model", Some(&"x".repeat(64))));
        assert!(matches!(result, Err(ModelCatalogError::ByteLimit { .. })));
        assert!(store.snapshot().models.is_empty());
        fs::remove_dir_all(root).expect("cleanup");
    }
}
