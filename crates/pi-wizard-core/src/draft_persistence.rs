use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread;

use atomic_write_file::AtomicWriteFile;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::draft::{DraftImageData, DraftLoaded};
use crate::{RunId, RuntimeLimits};

pub const DRAFT_FILE_SCHEMA_VERSION: u32 = 2;
const DRAFT_FILE_OVERHEAD_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct DraftFileStore {
    drafts_dir: PathBuf,
    quarantine_dir: PathBuf,
    limits: RuntimeLimits,
    max_session_id_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedDraft {
    schema_version: u32,
    session_id: String,
    text: String,
    #[serde(default)]
    images: Vec<DraftImageData>,
}

#[derive(Debug)]
pub enum DraftPersistenceEvent {
    Loaded {
        session_id: String,
        result: Result<Option<DraftLoaded>, String>,
    },
    Saved {
        session_id: String,
        generation: u64,
        result: Result<(), String>,
    },
}

#[derive(Debug)]
enum DraftPersistenceCommand {
    Load {
        session_id: String,
    },
    Save {
        session_id: String,
        generation: u64,
        text: String,
        images: Vec<DraftImageData>,
    },
}

#[derive(Clone, Debug)]
pub struct DraftPersistenceWorkerHandle {
    commands: SyncSender<DraftPersistenceCommand>,
}

impl DraftPersistenceWorkerHandle {
    pub fn try_load(&self, session_id: String) -> Result<(), DraftPersistenceWorkerError> {
        self.commands
            .try_send(DraftPersistenceCommand::Load { session_id })
            .map_err(map_try_send)
    }

    pub fn try_save(
        &self,
        session_id: String,
        generation: u64,
        text: String,
        images: Vec<DraftImageData>,
    ) -> Result<(), DraftPersistenceWorkerError> {
        self.commands
            .try_send(DraftPersistenceCommand::Save {
                session_id,
                generation,
                text,
                images,
            })
            .map_err(map_try_send)
    }
}

pub fn spawn_draft_persistence_worker(
    root: impl AsRef<Path>,
    limits: RuntimeLimits,
    events: tokio::sync::mpsc::Sender<DraftPersistenceEvent>,
) -> Result<DraftPersistenceWorkerHandle, DraftPersistenceError> {
    let store = DraftFileStore::open(root, limits)?;
    let (commands, receiver) = mpsc::sync_channel(limits.max_runtime_command_queue);
    thread::Builder::new()
        .name("pi-wizard-draft-persistence".to_owned())
        .spawn(move || {
            while let Ok(command) = receiver.recv() {
                let event = match command {
                    DraftPersistenceCommand::Load { session_id } => {
                        let result = store.load(&session_id).map_err(|error| error.to_string());
                        DraftPersistenceEvent::Loaded { session_id, result }
                    }
                    DraftPersistenceCommand::Save {
                        session_id,
                        generation,
                        text,
                        images,
                    } => {
                        let result = store
                            .save(&session_id, &DraftLoaded { text, images })
                            .map_err(|error| error.to_string());
                        DraftPersistenceEvent::Saved {
                            session_id,
                            generation,
                            result,
                        }
                    }
                };
                if events.blocking_send(event).is_err() {
                    return;
                }
            }
        })
        .map_err(DraftPersistenceError::ThreadSpawn)?;
    Ok(DraftPersistenceWorkerHandle { commands })
}

fn map_try_send<T>(error: TrySendError<T>) -> DraftPersistenceWorkerError {
    match error {
        TrySendError::Full(_) => DraftPersistenceWorkerError::QueueFull,
        TrySendError::Disconnected(_) => DraftPersistenceWorkerError::Closed,
    }
}

impl DraftFileStore {
    pub fn open(
        root: impl AsRef<Path>,
        limits: RuntimeLimits,
    ) -> Result<Self, DraftPersistenceError> {
        let drafts_dir = root.as_ref().join("drafts");
        let quarantine_dir = root.as_ref().join("draft-quarantine");
        fs::create_dir_all(&drafts_dir).map_err(|source| {
            DraftPersistenceError::CreateDirectory {
                path: drafts_dir.clone(),
                source,
            }
        })?;
        Ok(Self {
            drafts_dir,
            quarantine_dir,
            limits,
            max_session_id_bytes: limits.max_session_cursor_bytes,
        })
    }

    pub fn save(&self, session_id: &str, draft: &DraftLoaded) -> Result<(), DraftPersistenceError> {
        self.validate_session_id(session_id)?;
        draft
            .validate(self.limits)
            .map_err(|error| DraftPersistenceError::InvalidDraft(error.to_string()))?;
        let path = self.path_for_session(session_id);
        let encoded = serde_json::to_vec(&PersistedDraft {
            schema_version: DRAFT_FILE_SCHEMA_VERSION,
            session_id: session_id.to_owned(),
            text: draft.text.clone(),
            images: draft.images.clone(),
        })
        .map_err(DraftPersistenceError::Encode)?;
        if encoded.len() > self.max_file_bytes() {
            return Err(DraftPersistenceError::FileTooLarge {
                actual: encoded.len(),
                limit: self.max_file_bytes(),
            });
        }

        let mut file = AtomicWriteFile::options().open(&path).map_err(|source| {
            DraftPersistenceError::OpenAtomic {
                path: path.clone(),
                source,
            }
        })?;
        file.write_all(&encoded)
            .map_err(|source| DraftPersistenceError::Write {
                path: path.clone(),
                source,
            })?;
        // `commit` performs the file sync before replacement and current
        // atomic-write-file also syncs the containing directory on Unix.
        file.commit()
            .map_err(|source| DraftPersistenceError::Commit { path, source })
    }

    pub fn load(&self, session_id: &str) -> Result<Option<DraftLoaded>, DraftPersistenceError> {
        self.validate_session_id(session_id)?;
        let path = self.path_for_session(session_id);
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(DraftPersistenceError::Read {
                    path: path.clone(),
                    source,
                });
            }
        };
        let file_len = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if file_len > self.max_file_bytes() {
            return Err(self.quarantine(
                &path,
                DraftPersistenceError::FileTooLarge {
                    actual: file_len,
                    limit: self.max_file_bytes(),
                },
            ));
        }
        let bytes = fs::read(&path).map_err(|source| DraftPersistenceError::Read {
            path: path.clone(),
            source,
        })?;
        let persisted: PersistedDraft = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(source) => {
                return Err(self.quarantine(&path, DraftPersistenceError::Decode(source)));
            }
        };
        if !matches!(persisted.schema_version, 1 | DRAFT_FILE_SCHEMA_VERSION) {
            return Err(self.quarantine(
                &path,
                DraftPersistenceError::UnsupportedSchema {
                    actual: persisted.schema_version,
                    supported: DRAFT_FILE_SCHEMA_VERSION,
                },
            ));
        }
        if persisted.session_id != session_id {
            return Err(self.quarantine(&path, DraftPersistenceError::SessionIdentityMismatch));
        }
        let loaded = DraftLoaded {
            text: persisted.text,
            images: persisted.images,
        };
        if let Err(error) = loaded.validate(self.limits) {
            return Err(self.quarantine(
                &path,
                DraftPersistenceError::InvalidDraft(error.to_string()),
            ));
        }
        Ok(Some(loaded))
    }

    #[must_use]
    pub fn path_for_session(&self, session_id: &str) -> PathBuf {
        let digest = Sha256::digest(session_id.as_bytes());
        let mut name = String::with_capacity(digest.len() * 2 + 5);
        for byte in digest {
            use std::fmt::Write as _;
            let _ = write!(name, "{byte:02x}");
        }
        name.push_str(".json");
        self.drafts_dir.join(name)
    }

    fn validate_session_id(&self, session_id: &str) -> Result<(), DraftPersistenceError> {
        if session_id.is_empty() || session_id.len() > self.max_session_id_bytes {
            return Err(DraftPersistenceError::InvalidSessionId {
                actual: session_id.len(),
                limit: self.max_session_id_bytes,
            });
        }
        Ok(())
    }

    fn max_file_bytes(&self) -> usize {
        self.limits
            .max_draft_bytes_per_session
            .saturating_add(
                self.limits
                    .max_attachment_bytes_per_prompt
                    .saturating_mul(4)
                    .div_ceil(3),
            )
            .saturating_add(
                self.limits
                    .max_attachment_name_bytes
                    .saturating_mul(self.limits.max_attachments_per_prompt),
            )
            .saturating_add(self.max_session_id_bytes)
            .saturating_add(DRAFT_FILE_OVERHEAD_BYTES)
    }

    fn quarantine(&self, path: &Path, cause: DraftPersistenceError) -> DraftPersistenceError {
        let create = fs::create_dir_all(&self.quarantine_dir);
        if let Err(source) = create {
            return DraftPersistenceError::QuarantineFailed {
                path: path.to_path_buf(),
                cause: Box::new(cause),
                source,
            };
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("draft.json");
        let quarantine_path = self
            .quarantine_dir
            .join(format!("{}-{file_name}", RunId::new()));
        match fs::rename(path, &quarantine_path) {
            Ok(()) => DraftPersistenceError::Quarantined {
                path: path.to_path_buf(),
                quarantine_path,
                cause: Box::new(cause),
            },
            Err(source) => DraftPersistenceError::QuarantineFailed {
                path: path.to_path_buf(),
                cause: Box::new(cause),
                source,
            },
        }
    }
}

#[derive(Debug, Error)]
pub enum DraftPersistenceError {
    #[error("draft session id is {actual} bytes; expected 1..={limit}")]
    InvalidSessionId { actual: usize, limit: usize },
    #[error("draft is {actual} bytes, exceeding persistence limit {limit}")]
    DraftTooLarge { actual: usize, limit: usize },
    #[error("persisted draft content is invalid: {0}")]
    InvalidDraft(String),
    #[error("draft file is {actual} bytes, exceeding persistence limit {limit}")]
    FileTooLarge { actual: usize, limit: usize },
    #[error("draft file uses schema {actual}; supported schema is {supported}")]
    UnsupportedSchema { actual: u32, supported: u32 },
    #[error("draft file session identity does not match its requested session")]
    SessionIdentityMismatch,
    #[error("could not create draft directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not open atomic draft file {path}: {source}")]
    OpenAtomic {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write draft file {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not atomically commit draft file {path}: {source}")]
    Commit {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not read draft file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not encode draft file: {0}")]
    Encode(serde_json::Error),
    #[error("could not decode draft file: {0}")]
    Decode(serde_json::Error),
    #[error("draft file {path} was quarantined to {quarantine_path}: {cause}")]
    Quarantined {
        path: PathBuf,
        quarantine_path: PathBuf,
        cause: Box<DraftPersistenceError>,
    },
    #[error("draft file {path} was invalid and could not be quarantined: {cause}; {source}")]
    QuarantineFailed {
        path: PathBuf,
        cause: Box<DraftPersistenceError>,
        source: std::io::Error,
    },
    #[error("could not start draft persistence worker: {0}")]
    ThreadSpawn(std::io::Error),
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum DraftPersistenceWorkerError {
    #[error("bounded draft persistence queue is full")]
    QueueFull,
    #[error("draft persistence worker is closed")]
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("pi-wizard-drafts-{name}-{}", RunId::new()));
        fs::create_dir_all(&root).expect("fixture root");
        root
    }

    #[test]
    fn atomic_draft_round_trip_uses_bounded_hashed_filename() {
        let root = fixture("round-trip");
        let store = DraftFileStore::open(&root, RuntimeLimits::default()).expect("store");
        let session_id = "session-with-a-readable-identity";
        store
            .save(
                session_id,
                &DraftLoaded {
                    text: "important text".to_owned(),
                    images: Vec::new(),
                },
            )
            .expect("save");
        assert_eq!(
            store.load(session_id).expect("load"),
            Some(DraftLoaded {
                text: "important text".to_owned(),
                images: Vec::new(),
            })
        );
        let path = store.path_for_session(session_id);
        assert!(
            !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains(session_id)
        );
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".json")
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn corrupt_draft_is_quarantined_without_destroying_unrelated_drafts() {
        let root = fixture("quarantine");
        let store = DraftFileStore::open(&root, RuntimeLimits::default()).expect("store");
        store
            .save(
                "healthy",
                &DraftLoaded {
                    text: "keep me".to_owned(),
                    images: Vec::new(),
                },
            )
            .expect("healthy save");
        let corrupt_path = store.path_for_session("corrupt");
        fs::write(&corrupt_path, b"{not-json").expect("corrupt file");

        assert!(matches!(
            store.load("corrupt"),
            Err(DraftPersistenceError::Quarantined { .. })
        ));
        assert!(!corrupt_path.exists());
        assert_eq!(
            store.load("healthy").expect("healthy load"),
            Some(DraftLoaded {
                text: "keep me".to_owned(),
                images: Vec::new(),
            })
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn oversized_draft_is_rejected_before_atomic_write() {
        let root = fixture("limit");
        let limits = RuntimeLimits {
            max_draft_bytes_per_session: 4,
            ..RuntimeLimits::default()
        };
        let store = DraftFileStore::open(&root, limits).expect("store");
        assert!(matches!(
            store.save(
                "session",
                &DraftLoaded {
                    text: "too long".to_owned(),
                    images: Vec::new(),
                }
            ),
            Err(DraftPersistenceError::InvalidDraft(detail)) if detail.contains("draft is 8 bytes")
        ));
        assert!(!store.path_for_session("session").exists());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn schema_one_text_draft_loads_as_attachment_free_schema_two_state() {
        let root = fixture("schema-one");
        let store = DraftFileStore::open(&root, RuntimeLimits::default()).expect("store");
        let path = store.path_for_session("legacy");
        fs::write(
            &path,
            br#"{"schemaVersion":1,"sessionId":"legacy","text":"old text"}"#,
        )
        .expect("legacy draft");

        assert_eq!(
            store.load("legacy").expect("legacy load"),
            Some(DraftLoaded {
                text: "old text".to_owned(),
                images: Vec::new(),
            })
        );
        assert!(
            path.exists(),
            "valid schema-one draft must not be quarantined"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn unsupported_future_draft_schema_is_quarantined_instead_of_downgraded() {
        let root = fixture("future-schema");
        let store = DraftFileStore::open(&root, RuntimeLimits::default()).expect("store");
        let path = store.path_for_session("future");
        fs::write(
            &path,
            br#"{"schemaVersion":999,"sessionId":"future","text":"do not guess","images":[]}"#,
        )
        .expect("future draft");

        assert!(matches!(
            store.load("future"),
            Err(DraftPersistenceError::Quarantined { .. })
        ));
        assert!(
            !path.exists(),
            "unsupported future state must leave the active draft path"
        );
        assert!(
            root.join("draft-quarantine").is_dir(),
            "unsupported future state must be retained for diagnosis"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn attachment_round_trip_preserves_validated_image_data() {
        let root = fixture("attachment");
        let limits = RuntimeLimits::default();
        let store = DraftFileStore::open(&root, limits).expect("store");
        let image = DraftImageData::try_new(
            "screen.png".to_owned(),
            "image/png".to_owned(),
            "aGVsbG8=".to_owned(),
            limits,
        )
        .expect("image");
        let saved = DraftLoaded {
            text: "look here".to_owned(),
            images: vec![image],
        };
        store.save("session", &saved).expect("save");
        assert_eq!(store.load("session").expect("load"), Some(saved));
        fs::remove_dir_all(root).expect("cleanup");
    }
}
