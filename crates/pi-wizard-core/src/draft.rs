use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bounded::BoundedText;
use crate::rpc::{AttachmentError, ImageContent};
use crate::{DraftImageId, RunId, RuntimeLimits};

/// Durability state for one session-scoped composer draft.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftDurability {
    Saved,
    Dirty,
    Saving,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftSnapshot {
    pub text: String,
    pub images: Vec<DraftImageSnapshot>,
    pub generation: u64,
    pub durability: DraftDurability,
    pub persistence_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftImageSnapshot {
    pub id: DraftImageId,
    pub file_name: String,
    pub mime_type: String,
    pub decoded_bytes: usize,
}

/// Raw image content owned by the backend draft and persistence layers. This
/// type is never included in runtime hydration; only DraftImageSnapshot is.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftImageData {
    pub id: DraftImageId,
    pub file_name: String,
    pub mime_type: String,
    pub data: String,
}

impl DraftImageData {
    pub fn try_new(
        file_name: String,
        mime_type: String,
        data: String,
        limits: RuntimeLimits,
    ) -> Result<Self, DraftError> {
        let file_name = normalize_file_name(file_name);
        validate_file_name(&file_name, limits)?;
        ImageContent::try_new(data.clone(), mime_type.clone(), limits)?;
        Ok(Self {
            id: DraftImageId::new(),
            file_name,
            mime_type,
            data,
        })
    }

    pub fn validate(&self, limits: RuntimeLimits) -> Result<usize, DraftError> {
        validate_file_name(&self.file_name, limits)?;
        Ok(
            ImageContent::try_new(self.data.clone(), self.mime_type.clone(), limits)?
                .decoded_bytes(),
        )
    }

    pub fn to_rpc(&self, limits: RuntimeLimits) -> Result<ImageContent, DraftError> {
        validate_file_name(&self.file_name, limits)?;
        ImageContent::try_new(self.data.clone(), self.mime_type.clone(), limits).map_err(Into::into)
    }

    pub fn snapshot(&self, limits: RuntimeLimits) -> Result<DraftImageSnapshot, DraftError> {
        Ok(DraftImageSnapshot {
            id: self.id,
            file_name: self.file_name.clone(),
            mime_type: self.mime_type.clone(),
            decoded_bytes: self.validate(limits)?,
        })
    }
}

fn normalize_file_name(file_name: String) -> String {
    let trimmed = file_name.trim();
    if trimmed.is_empty() {
        "image".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn validate_file_name(file_name: &str, limits: RuntimeLimits) -> Result<(), DraftError> {
    if file_name.len() > limits.max_attachment_name_bytes {
        return Err(DraftError::AttachmentNameTooLarge {
            actual: file_name.len(),
            limit: limits.max_attachment_name_bytes,
        });
    }
    Ok(())
}

fn validate_draft_content(
    text: &str,
    images: &[DraftImageData],
    limits: RuntimeLimits,
) -> Result<(), DraftError> {
    if text.len() > limits.max_draft_bytes_per_session {
        return Err(DraftError::TooLarge {
            actual: text.len(),
            limit: limits.max_draft_bytes_per_session,
        });
    }
    if images.len() > limits.max_attachments_per_prompt {
        return Err(DraftError::Attachment(AttachmentError::TooMany {
            actual: images.len(),
            limit: limits.max_attachments_per_prompt,
        }));
    }
    let mut ids = std::collections::HashSet::with_capacity(images.len());
    let mut total = 0usize;
    for image in images {
        if !ids.insert(image.id) {
            return Err(DraftError::DuplicateAttachment { id: image.id });
        }
        total = total.saturating_add(image.validate(limits)?);
        if total > limits.max_attachment_bytes_per_prompt {
            return Err(DraftError::Attachment(AttachmentError::PromptTooLarge {
                actual: total,
                limit: limits.max_attachment_bytes_per_prompt,
            }));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DraftLoaded {
    pub text: String,
    pub images: Vec<DraftImageData>,
}

impl DraftLoaded {
    pub fn validate(&self, limits: RuntimeLimits) -> Result<(), DraftError> {
        validate_draft_content(&self.text, &self.images, limits)
    }
}

/// Immutable save payload carrying the generation that produced it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DraftSave {
    pub generation: u64,
    pub text: String,
    pub images: Vec<DraftImageData>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DraftSubmission {
    pub generation: u64,
    pub text: String,
    pub images: Vec<ImageContent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DraftClearOutcome {
    Cleared,
    Superseded { current_generation: u64 },
}

/// Session-scoped draft state with generation-safe asynchronous persistence.
///
/// Only one write may be in flight. If the user edits while that write is
/// running, completion of the older generation cannot mark the newer text as
/// durable. A subsequent save always snapshots the newest generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DraftRecord {
    text: String,
    images: Vec<DraftImageData>,
    generation: u64,
    saved_generation: u64,
    in_flight_generation: Option<u64>,
    failed_generation: Option<u64>,
    max_bytes: usize,
    limits: RuntimeLimits,
    max_failure_detail_bytes: usize,
    failure_detail: Option<String>,
}

impl DraftRecord {
    #[must_use]
    pub fn new(limits: RuntimeLimits) -> Self {
        Self {
            text: String::new(),
            images: Vec::new(),
            generation: 0,
            saved_generation: 0,
            in_flight_generation: None,
            failed_generation: None,
            max_bytes: limits.max_draft_bytes_per_session,
            limits,
            max_failure_detail_bytes: limits.max_failure_detail_bytes,
            failure_detail: None,
        }
    }

    pub fn from_saved(text: String, limits: RuntimeLimits) -> Result<Self, DraftError> {
        Self::from_loaded(
            DraftLoaded {
                text,
                images: Vec::new(),
            },
            limits,
        )
    }

    pub fn from_loaded(loaded: DraftLoaded, limits: RuntimeLimits) -> Result<Self, DraftError> {
        loaded.validate(limits)?;
        let generation = usize::from(!loaded.text.is_empty() || !loaded.images.is_empty()) as u64;
        Ok(Self {
            text: loaded.text,
            images: loaded.images,
            generation,
            saved_generation: generation,
            in_flight_generation: None,
            failed_generation: None,
            max_bytes: limits.max_draft_bytes_per_session,
            limits,
            max_failure_detail_bytes: limits.max_failure_detail_bytes,
            failure_detail: None,
        })
    }

    pub fn edit(&mut self, text: String) -> Result<u64, DraftError> {
        if text.len() > self.max_bytes {
            return Err(DraftError::TooLarge {
                actual: text.len(),
                limit: self.max_bytes,
            });
        }
        if text == self.text {
            return Ok(self.generation);
        }

        self.text = text;
        self.generation = self.generation.saturating_add(1);
        Ok(self.generation)
    }

    pub fn attach_image(&mut self, image: DraftImageData) -> Result<u64, DraftError> {
        if self.images.iter().any(|existing| existing.id == image.id) {
            return Err(DraftError::DuplicateAttachment { id: image.id });
        }
        let mut prospective = self.images.clone();
        prospective.push(image);
        validate_draft_content(&self.text, &prospective, self.limits)?;
        self.images = prospective;
        self.generation = self.generation.saturating_add(1);
        Ok(self.generation)
    }

    pub fn remove_image(&mut self, image_id: DraftImageId) -> Result<u64, DraftError> {
        let Some(index) = self.images.iter().position(|image| image.id == image_id) else {
            return Err(DraftError::UnknownAttachment { id: image_id });
        };
        self.images.remove(index);
        self.generation = self.generation.saturating_add(1);
        Ok(self.generation)
    }

    pub fn submission(&self) -> Result<DraftSubmission, DraftError> {
        validate_draft_content(&self.text, &self.images, self.limits)?;
        let images = self
            .images
            .iter()
            .map(|image| image.to_rpc(self.limits))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(DraftSubmission {
            generation: self.generation,
            text: self.text.clone(),
            images,
        })
    }

    pub fn begin_save(&mut self) -> Result<DraftSave, DraftError> {
        if let Some(generation) = self.in_flight_generation {
            return Err(DraftError::SaveInFlight { generation });
        }
        if self.saved_generation == self.generation {
            return Err(DraftError::AlreadySaved {
                generation: self.generation,
            });
        }

        let generation = self.generation;
        self.in_flight_generation = Some(generation);
        Ok(DraftSave {
            generation,
            text: self.text.clone(),
            images: self.images.clone(),
        })
    }

    pub fn complete_save(&mut self, generation: u64, success: bool) -> Result<(), DraftError> {
        self.complete_save_with_detail(
            generation,
            success.then_some(()).ok_or("draft persistence failed"),
        )
    }

    pub fn complete_save_with_detail(
        &mut self,
        generation: u64,
        result: Result<(), &str>,
    ) -> Result<(), DraftError> {
        if self.in_flight_generation != Some(generation) {
            return Err(DraftError::StaleCompletion { generation });
        }
        self.in_flight_generation = None;

        match result {
            Ok(()) => {
                self.saved_generation = self.saved_generation.max(generation);
                if self.failed_generation == Some(generation) {
                    self.failed_generation = None;
                    self.failure_detail = None;
                }
            }
            Err(detail) => {
                self.failed_generation = Some(generation);
                self.set_failure_detail(detail);
            }
        }
        Ok(())
    }

    pub fn mark_current_persistence_failed(&mut self, detail: &str) {
        self.in_flight_generation = None;
        self.failed_generation = Some(self.generation);
        self.set_failure_detail(detail);
    }

    fn set_failure_detail(&mut self, detail: &str) {
        let mut bounded = BoundedText::new(self.max_failure_detail_bytes);
        bounded.replace(detail);
        self.failure_detail = Some(bounded.as_str().to_owned());
    }

    #[must_use]
    pub fn has_save_in_flight(&self) -> bool {
        self.in_flight_generation.is_some()
    }

    #[must_use]
    pub fn is_unsaved(&self) -> bool {
        self.saved_generation != self.generation || self.failed_generation == Some(self.generation)
    }

    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.failed_generation == Some(self.generation)
    }

    #[must_use]
    pub fn durability(&self) -> DraftDurability {
        if self.in_flight_generation == Some(self.generation) {
            DraftDurability::Saving
        } else if self.failed_generation == Some(self.generation) {
            DraftDurability::Failed
        } else if self.saved_generation == self.generation {
            DraftDurability::Saved
        } else {
            DraftDurability::Dirty
        }
    }

    #[must_use]
    pub fn persistence_error(&self) -> Option<&str> {
        if self.failed_generation == Some(self.generation) {
            self.failure_detail.as_deref()
        } else {
            None
        }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn images(&self) -> &[DraftImageData] {
        &self.images
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn snapshot(&self) -> DraftSnapshot {
        DraftSnapshot {
            text: self.text.clone(),
            images: self
                .images
                .iter()
                .map(|image| {
                    image
                        .snapshot(self.limits)
                        .expect("draft image invariant validated at mutation/restore boundary")
                })
                .collect(),
            generation: self.generation,
            durability: self.durability(),
            persistence_error: self.persistence_error().map(str::to_owned),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DraftRestoreOutcome {
    Restored,
    LocalStateWins,
}

/*
 * DraftRecord methods above intentionally keep persistence generations local
 * to one process. A restart restores text as a new Saved baseline instead of
 * pretending an old process generation can correlate with a new worker.
 */

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum DraftOwner {
    PendingRun(RunId),
    Session(String),
}

/// In-memory ownership map joining a live run to the draft for Pi's current
/// session. Filesystem persistence is a separate owner and is intentionally not
/// implied by this type.
#[derive(Debug)]
pub struct SessionDraftStore {
    limits: RuntimeLimits,
    records: HashMap<DraftOwner, DraftRecord>,
    run_owners: HashMap<RunId, DraftOwner>,
}

impl SessionDraftStore {
    #[must_use]
    pub fn new(limits: RuntimeLimits) -> Self {
        Self {
            limits,
            records: HashMap::new(),
            run_owners: HashMap::new(),
        }
    }

    pub fn register_run(
        &mut self,
        run_id: RunId,
        initial_session_id: Option<String>,
    ) -> Result<Option<String>, DraftStoreError> {
        if self.run_owners.contains_key(&run_id) {
            return Err(DraftStoreError::DuplicateRun { run_id });
        }
        let owner = initial_session_id
            .map(DraftOwner::Session)
            .unwrap_or(DraftOwner::PendingRun(run_id));
        let evicted = self.make_room_for(&owner, None)?;
        self.records
            .entry(owner.clone())
            .or_insert_with(|| DraftRecord::new(self.limits));
        self.run_owners.insert(run_id, owner);
        Ok(evicted)
    }

    /// Rebinds a run to Pi's authoritative current session. A draft created
    /// before the first session identity is known migrates into that session.
    /// A later session switch never migrates the previous session's text.
    pub fn reconcile_session(
        &mut self,
        run_id: RunId,
        session_id: String,
    ) -> Result<Option<String>, DraftStoreError> {
        let current = self
            .run_owners
            .get(&run_id)
            .cloned()
            .ok_or(DraftStoreError::UnknownRun { run_id })?;
        let next = DraftOwner::Session(session_id);
        if current == next {
            return Ok(None);
        }

        if matches!(current, DraftOwner::PendingRun(_)) {
            let pending = self
                .records
                .remove(&current)
                .unwrap_or_else(|| DraftRecord::new(self.limits));
            match self.records.entry(next.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(pending);
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    if pending.generation() > 0 && entry.get().generation() == 0 {
                        entry.insert(pending);
                    }
                }
            }
            self.run_owners.insert(run_id, next);
            Ok(None)
        } else {
            let evicted = self.make_room_for(&next, Some(run_id))?;
            self.records
                .entry(next.clone())
                .or_insert_with(|| DraftRecord::new(self.limits));
            self.run_owners.insert(run_id, next);
            Ok(evicted)
        }
    }

    fn make_room_for(
        &mut self,
        target: &DraftOwner,
        releasing_run: Option<RunId>,
    ) -> Result<Option<String>, DraftStoreError> {
        if self.records.contains_key(target)
            || self.records.len() < self.limits.max_cached_draft_records
        {
            return Ok(None);
        }

        let candidate = self.records.iter().find_map(|(owner, record)| {
            let DraftOwner::Session(session_id) = owner else {
                return None;
            };
            if owner == target || record.durability() != DraftDurability::Saved {
                return None;
            }
            let owned_by_non_releasing_run = self
                .run_owners
                .iter()
                .any(|(run_id, run_owner)| run_owner == owner && Some(*run_id) != releasing_run);
            (!owned_by_non_releasing_run).then(|| (owner.clone(), session_id.clone()))
        });

        let Some((owner, session_id)) = candidate else {
            return Err(DraftStoreError::Capacity {
                limit: self.limits.max_cached_draft_records,
            });
        };
        self.records.remove(&owner);
        Ok(Some(session_id))
    }

    /// Releases only the run-to-draft ownership edge. Session-scoped draft
    /// records remain available for another live run or durable persistence;
    /// a pre-session pending record is run-scoped and can be discarded once
    /// that terminal run is no longer retained.
    pub fn release_run(&mut self, run_id: RunId) -> Result<(), DraftStoreError> {
        let owner = self
            .run_owners
            .remove(&run_id)
            .ok_or(DraftStoreError::UnknownRun { run_id })?;
        if matches!(owner, DraftOwner::PendingRun(_)) {
            self.records.remove(&owner);
        }
        Ok(())
    }

    pub fn edit_run(&mut self, run_id: RunId, text: String) -> Result<u64, DraftStoreError> {
        let owner = self
            .run_owners
            .get(&run_id)
            .cloned()
            .ok_or(DraftStoreError::UnknownRun { run_id })?;
        self.records
            .get_mut(&owner)
            .ok_or(DraftStoreError::MissingRecord { run_id })?
            .edit(text)
            .map_err(Into::into)
    }

    pub fn attach_image_run(
        &mut self,
        run_id: RunId,
        image: DraftImageData,
    ) -> Result<u64, DraftStoreError> {
        let owner = self
            .run_owners
            .get(&run_id)
            .cloned()
            .ok_or(DraftStoreError::UnknownRun { run_id })?;
        self.records
            .get_mut(&owner)
            .ok_or(DraftStoreError::MissingRecord { run_id })?
            .attach_image(image)
            .map_err(Into::into)
    }

    pub fn remove_image_run(
        &mut self,
        run_id: RunId,
        image_id: DraftImageId,
    ) -> Result<u64, DraftStoreError> {
        let owner = self
            .run_owners
            .get(&run_id)
            .cloned()
            .ok_or(DraftStoreError::UnknownRun { run_id })?;
        self.records
            .get_mut(&owner)
            .ok_or(DraftStoreError::MissingRecord { run_id })?
            .remove_image(image_id)
            .map_err(Into::into)
    }

    pub fn submission_run(&self, run_id: RunId) -> Result<DraftSubmission, DraftStoreError> {
        let owner = self
            .run_owners
            .get(&run_id)
            .ok_or(DraftStoreError::UnknownRun { run_id })?;
        self.records
            .get(owner)
            .ok_or(DraftStoreError::MissingRecord { run_id })?
            .submission()
            .map_err(Into::into)
    }

    #[must_use]
    pub fn current_session_id(&self, run_id: RunId) -> Option<&str> {
        match self.run_owners.get(&run_id)? {
            DraftOwner::Session(session_id) => Some(session_id),
            DraftOwner::PendingRun(_) => None,
        }
    }

    pub fn begin_save_session(&mut self, session_id: &str) -> Result<DraftSave, DraftStoreError> {
        self.records
            .get_mut(&DraftOwner::Session(session_id.to_owned()))
            .ok_or_else(|| DraftStoreError::UnknownSession {
                session_id: session_id.to_owned(),
            })?
            .begin_save()
            .map_err(Into::into)
    }

    pub fn complete_save_session(
        &mut self,
        session_id: &str,
        generation: u64,
        result: Result<(), &str>,
    ) -> Result<(), DraftStoreError> {
        self.records
            .get_mut(&DraftOwner::Session(session_id.to_owned()))
            .ok_or_else(|| DraftStoreError::UnknownSession {
                session_id: session_id.to_owned(),
            })?
            .complete_save_with_detail(generation, result)
            .map_err(Into::into)
    }

    pub fn restore_session_if_unedited(
        &mut self,
        session_id: &str,
        loaded: DraftLoaded,
    ) -> Result<DraftRestoreOutcome, DraftStoreError> {
        let owner = DraftOwner::Session(session_id.to_owned());
        let Some(record) = self.records.get_mut(&owner) else {
            // A bounded cache may evict an unowned saved record while an older
            // load completion is still queued. Never let stale I/O recreate an
            // unowned record outside the cache ceiling.
            return Ok(DraftRestoreOutcome::LocalStateWins);
        };
        if record.generation() != 0 || !record.text().is_empty() || !record.images().is_empty() {
            return Ok(DraftRestoreOutcome::LocalStateWins);
        }
        *record = DraftRecord::from_loaded(loaded, self.limits)?;
        Ok(DraftRestoreOutcome::Restored)
    }

    pub fn mark_session_persistence_failed(
        &mut self,
        session_id: &str,
        detail: &str,
    ) -> Result<(), DraftStoreError> {
        self.records
            .get_mut(&DraftOwner::Session(session_id.to_owned()))
            .ok_or_else(|| DraftStoreError::UnknownSession {
                session_id: session_id.to_owned(),
            })?
            .mark_current_persistence_failed(detail);
        Ok(())
    }

    #[must_use]
    pub fn unsaved_sessions(&self) -> Vec<String> {
        self.records
            .iter()
            .filter_map(|(owner, record)| match owner {
                DraftOwner::Session(session_id)
                    if record.is_unsaved() && !record.has_save_in_flight() =>
                {
                    Some(session_id.clone())
                }
                _ => None,
            })
            .collect()
    }

    #[must_use]
    pub fn has_saves_in_flight(&self) -> bool {
        self.records.values().any(DraftRecord::has_save_in_flight)
    }

    #[must_use]
    pub fn failed_session_count(&self) -> usize {
        self.records
            .values()
            .filter(|record| record.is_failed())
            .count()
    }

    pub fn fail_in_flight_saves(&mut self, detail: &str) -> Vec<String> {
        let mut failed = Vec::new();
        for (owner, record) in &mut self.records {
            let DraftOwner::Session(session_id) = owner else {
                continue;
            };
            if record.has_save_in_flight() {
                record.mark_current_persistence_failed(detail);
                failed.push(session_id.clone());
            }
        }
        failed
    }

    #[must_use]
    pub fn run_ids_for_session(&self, session_id: &str) -> Vec<RunId> {
        self.run_owners
            .iter()
            .filter_map(|(run_id, owner)| {
                (owner == &DraftOwner::Session(session_id.to_owned())).then_some(*run_id)
            })
            .collect()
    }

    /// Clears only the draft generation that was actually submitted. If the
    /// user edited while the request was in flight, the newer text remains
    /// untouched and the caller is told that the submitted generation was
    /// superseded.
    pub fn clear_run_if_generation(
        &mut self,
        run_id: RunId,
        submitted_generation: u64,
    ) -> Result<DraftClearOutcome, DraftStoreError> {
        let owner = self
            .run_owners
            .get(&run_id)
            .cloned()
            .ok_or(DraftStoreError::UnknownRun { run_id })?;
        let record = self
            .records
            .get_mut(&owner)
            .ok_or(DraftStoreError::MissingRecord { run_id })?;
        if record.generation() != submitted_generation {
            return Ok(DraftClearOutcome::Superseded {
                current_generation: record.generation(),
            });
        }
        if !record.text.is_empty() || !record.images.is_empty() {
            record.text.clear();
            record.images.clear();
            record.generation = record.generation.saturating_add(1);
        }
        Ok(DraftClearOutcome::Cleared)
    }

    #[must_use]
    pub fn snapshot_run(&self, run_id: RunId) -> Option<DraftSnapshot> {
        self.run_owners
            .get(&run_id)
            .and_then(|owner| self.records.get(owner))
            .map(DraftRecord::snapshot)
    }

    #[must_use]
    pub fn snapshot_session(&self, session_id: &str) -> Option<DraftSnapshot> {
        self.records
            .get(&DraftOwner::Session(session_id.to_owned()))
            .map(DraftRecord::snapshot)
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum DraftError {
    #[error("draft is {actual} bytes, exceeding limit {limit}")]
    TooLarge { actual: usize, limit: usize },
    #[error("attachment file name is {actual} bytes, exceeding limit {limit}")]
    AttachmentNameTooLarge { actual: usize, limit: usize },
    #[error("draft attachment id {id} is duplicated")]
    DuplicateAttachment { id: DraftImageId },
    #[error("draft attachment id {id} is not present")]
    UnknownAttachment { id: DraftImageId },
    #[error(transparent)]
    Attachment(#[from] AttachmentError),
    #[error("draft generation {generation} is already being saved")]
    SaveInFlight { generation: u64 },
    #[error("draft generation {generation} is already durable")]
    AlreadySaved { generation: u64 },
    #[error("draft save completion for generation {generation} is not current")]
    StaleCompletion { generation: u64 },
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum DraftStoreError {
    #[error("draft owner already exists for run {run_id}")]
    DuplicateRun { run_id: RunId },
    #[error("draft owner is missing for run {run_id}")]
    UnknownRun { run_id: RunId },
    #[error("draft record is missing for run {run_id}")]
    MissingRecord { run_id: RunId },
    #[error("draft record is missing for session {session_id}")]
    UnknownSession { session_id: String },
    #[error("draft cache limit {limit} reached with no safely evictable saved session")]
    Capacity { limit: usize },
    #[error(transparent)]
    Draft(#[from] DraftError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(name: &str, data: &str, limits: RuntimeLimits) -> DraftImageData {
        DraftImageData::try_new(
            name.to_owned(),
            "image/png".to_owned(),
            data.to_owned(),
            limits,
        )
        .expect("valid image")
    }

    #[test]
    fn cached_drafts_evict_only_unowned_saved_sessions_at_capacity() {
        let limits = RuntimeLimits {
            max_live_runs: 1,
            max_cached_draft_records: 2,
            ..RuntimeLimits::default()
        };
        let run_id = RunId::new();
        let mut drafts = SessionDraftStore::new(limits);
        drafts
            .register_run(run_id, Some("session-a".to_owned()))
            .expect("register a");
        drafts
            .reconcile_session(run_id, "session-b".to_owned())
            .expect("switch b");
        let evicted = drafts
            .reconcile_session(run_id, "session-c".to_owned())
            .expect("switch c with bounded eviction");

        assert!(matches!(
            evicted.as_deref(),
            Some("session-a" | "session-b")
        ));
        assert!(drafts.snapshot_session("session-c").is_some());
        let retained_old = usize::from(drafts.snapshot_session("session-a").is_some())
            + usize::from(drafts.snapshot_session("session-b").is_some());
        assert_eq!(retained_old, 1);
    }

    #[test]
    fn draft_cache_capacity_never_evicts_dirty_or_failed_session_state() {
        let limits = RuntimeLimits {
            max_live_runs: 1,
            max_cached_draft_records: 2,
            ..RuntimeLimits::default()
        };
        let run_id = RunId::new();
        let mut drafts = SessionDraftStore::new(limits);
        drafts
            .register_run(run_id, Some("session-a".to_owned()))
            .expect("register a");
        drafts
            .edit_run(run_id, "draft a".to_owned())
            .expect("dirty a");
        drafts
            .reconcile_session(run_id, "session-b".to_owned())
            .expect("switch b");
        drafts
            .edit_run(run_id, "draft b".to_owned())
            .expect("dirty b");

        assert_eq!(
            drafts.reconcile_session(run_id, "session-c".to_owned()),
            Err(DraftStoreError::Capacity { limit: 2 })
        );
        assert_eq!(drafts.current_session_id(run_id), Some("session-b"));
        assert_eq!(
            drafts
                .snapshot_session("session-a")
                .expect("a retained")
                .text,
            "draft a"
        );
        assert_eq!(
            drafts
                .snapshot_session("session-b")
                .expect("b retained")
                .text,
            "draft b"
        );
        assert!(drafts.snapshot_session("session-c").is_none());
    }

    #[test]
    fn stale_draft_load_cannot_recreate_an_evicted_unowned_record() {
        let limits = RuntimeLimits {
            max_live_runs: 1,
            max_cached_draft_records: 1,
            ..RuntimeLimits::default()
        };
        let run_id = RunId::new();
        let mut drafts = SessionDraftStore::new(limits);
        drafts
            .register_run(run_id, Some("session-a".to_owned()))
            .expect("register a");
        assert_eq!(
            drafts
                .reconcile_session(run_id, "session-b".to_owned())
                .expect("evict a"),
            Some("session-a".to_owned())
        );
        assert_eq!(
            drafts
                .restore_session_if_unedited(
                    "session-a",
                    DraftLoaded {
                        text: "stale load".to_owned(),
                        images: Vec::new(),
                    },
                )
                .expect("ignore stale load"),
            DraftRestoreOutcome::LocalStateWins
        );
        assert!(drafts.snapshot_session("session-a").is_none());
        assert!(drafts.snapshot_session("session-b").is_some());
    }

    #[test]
    fn old_save_completion_cannot_mark_newer_edit_durable() {
        let mut draft = DraftRecord::new(RuntimeLimits::default());
        draft.edit("first".to_owned()).expect("first edit");
        let first = draft.begin_save().expect("start first save");

        draft.edit("second".to_owned()).expect("second edit");
        assert_eq!(draft.durability(), DraftDurability::Dirty);
        draft
            .complete_save(first.generation, true)
            .expect("complete older save");
        assert_eq!(draft.durability(), DraftDurability::Dirty);

        let second = draft.begin_save().expect("save newest generation");
        assert_eq!(second.text, "second");
        assert!(second.generation > first.generation);
    }

    #[test]
    fn failed_current_save_is_visible_and_retry_uses_same_current_text() {
        let mut draft = DraftRecord::new(RuntimeLimits::default());
        draft.edit("important".to_owned()).expect("edit");
        let save = draft.begin_save().expect("save");
        draft
            .complete_save(save.generation, false)
            .expect("record failure");
        assert_eq!(draft.durability(), DraftDurability::Failed);

        let retry = draft.begin_save().expect("retry");
        assert_eq!(retry.generation, save.generation);
        assert_eq!(retry.text, "important");
    }

    #[test]
    fn oversized_edit_is_rejected_without_destroying_current_draft() {
        let limits = RuntimeLimits {
            max_draft_bytes_per_session: 4,
            ..RuntimeLimits::default()
        };
        let mut draft = DraftRecord::new(limits);
        draft.edit("safe".to_owned()).expect("bounded edit");

        assert_eq!(
            draft.edit("too large".to_owned()),
            Err(DraftError::TooLarge {
                actual: 9,
                limit: 4,
            })
        );
        assert_eq!(draft.text(), "safe");
    }

    #[test]
    fn pending_run_draft_migrates_when_first_pi_session_identity_arrives() {
        let run_id = RunId::new();
        let mut drafts = SessionDraftStore::new(RuntimeLimits::default());
        drafts.register_run(run_id, None).expect("register run");
        drafts
            .edit_run(run_id, "before state".to_owned())
            .expect("pending edit");
        drafts
            .reconcile_session(run_id, "session-a".to_owned())
            .expect("bind session");
        assert_eq!(
            drafts.snapshot_run(run_id).expect("draft").text,
            "before state"
        );
    }

    #[test]
    fn session_switch_does_not_carry_draft_and_switching_back_restores_it() {
        let run_id = RunId::new();
        let mut drafts = SessionDraftStore::new(RuntimeLimits::default());
        drafts
            .register_run(run_id, Some("session-a".to_owned()))
            .expect("register run");
        drafts
            .edit_run(run_id, "draft a".to_owned())
            .expect("edit a");
        drafts
            .reconcile_session(run_id, "session-b".to_owned())
            .expect("switch to b");
        assert_eq!(drafts.snapshot_run(run_id).expect("draft b").text, "");
        drafts
            .edit_run(run_id, "draft b".to_owned())
            .expect("edit b");
        drafts
            .reconcile_session(run_id, "session-a".to_owned())
            .expect("switch back");
        assert_eq!(
            drafts.snapshot_run(run_id).expect("draft a").text,
            "draft a"
        );
    }

    #[test]
    fn releasing_run_keeps_session_draft_but_drops_run_owner() {
        let run_id = RunId::new();
        let mut drafts = SessionDraftStore::new(RuntimeLimits::default());
        drafts
            .register_run(run_id, Some("session-a".to_owned()))
            .expect("register run");
        drafts
            .edit_run(run_id, "keep by session".to_owned())
            .expect("edit session draft");

        drafts.release_run(run_id).expect("release run owner");
        assert!(drafts.snapshot_run(run_id).is_none());
        assert_eq!(
            drafts
                .snapshot_session("session-a")
                .expect("session draft retained")
                .text,
            "keep by session"
        );
    }

    #[test]
    fn releasing_pre_session_run_discards_only_pending_record() {
        let run_id = RunId::new();
        let mut drafts = SessionDraftStore::new(RuntimeLimits::default());
        drafts
            .register_run(run_id, None)
            .expect("register pending run");
        drafts.release_run(run_id).expect("release pending run");
        assert!(drafts.snapshot_run(run_id).is_none());
        assert_eq!(
            drafts.release_run(run_id),
            Err(DraftStoreError::UnknownRun { run_id })
        );
    }

    #[test]
    fn accepted_submission_cannot_clear_a_newer_edit() {
        let run_id = RunId::new();
        let mut drafts = SessionDraftStore::new(RuntimeLimits::default());
        drafts
            .register_run(run_id, Some("session-a".to_owned()))
            .expect("register run");
        let submitted = drafts
            .edit_run(run_id, "submit me".to_owned())
            .expect("submitted edit");
        drafts
            .edit_run(run_id, "typed while sending".to_owned())
            .expect("newer edit");

        assert_eq!(
            drafts
                .clear_run_if_generation(run_id, submitted)
                .expect("conditional clear"),
            DraftClearOutcome::Superseded {
                current_generation: submitted + 1
            }
        );
        assert_eq!(
            drafts.snapshot_run(run_id).expect("current draft").text,
            "typed while sending"
        );
    }

    #[test]
    fn accepted_current_generation_clears_the_draft() {
        let run_id = RunId::new();
        let mut drafts = SessionDraftStore::new(RuntimeLimits::default());
        drafts
            .register_run(run_id, Some("session-a".to_owned()))
            .expect("register run");
        let submitted = drafts
            .edit_run(run_id, "submit me".to_owned())
            .expect("submitted edit");

        assert_eq!(
            drafts
                .clear_run_if_generation(run_id, submitted)
                .expect("conditional clear"),
            DraftClearOutcome::Cleared
        );
        assert_eq!(drafts.snapshot_run(run_id).expect("draft").text, "");
    }

    #[test]
    fn old_session_save_completion_cannot_mutate_newly_selected_session() {
        let run_id = RunId::new();
        let mut drafts = SessionDraftStore::new(RuntimeLimits::default());
        drafts
            .register_run(run_id, Some("session-a".to_owned()))
            .expect("register");
        drafts
            .edit_run(run_id, "draft a".to_owned())
            .expect("edit a");
        let save_a = drafts.begin_save_session("session-a").expect("save a");

        drafts
            .reconcile_session(run_id, "session-b".to_owned())
            .expect("switch b");
        drafts
            .edit_run(run_id, "draft b".to_owned())
            .expect("edit b");
        drafts
            .complete_save_session("session-a", save_a.generation, Ok(()))
            .expect("complete a after switch");

        let active = drafts.snapshot_run(run_id).expect("active b");
        assert_eq!(active.text, "draft b");
        assert_eq!(active.durability, DraftDurability::Dirty);
    }

    #[test]
    fn attachment_limits_reject_new_image_without_mutating_existing_draft() {
        let limits = RuntimeLimits {
            max_attachments_per_prompt: 2,
            max_attachment_bytes_per_image: 4,
            max_attachment_bytes_per_prompt: 4,
            ..RuntimeLimits::default()
        };
        let mut draft = DraftRecord::new(limits);
        draft
            .attach_image(image("first.png", "YWJj", limits))
            .expect("first image");
        let first = draft.snapshot();
        assert_eq!(first.images.len(), 1);

        assert!(matches!(
            draft.attach_image(image("second.png", "ZGVm", limits)),
            Err(DraftError::Attachment(AttachmentError::PromptTooLarge {
                actual: 6,
                limit: 4,
            }))
        ));
        assert_eq!(draft.snapshot(), first);
    }

    #[test]
    fn accepted_current_generation_clears_text_and_images_together() {
        let limits = RuntimeLimits::default();
        let run_id = RunId::new();
        let mut drafts = SessionDraftStore::new(limits);
        drafts
            .register_run(run_id, Some("session-a".to_owned()))
            .expect("register");
        drafts.edit_run(run_id, "inspect".to_owned()).expect("text");
        let submitted = drafts
            .attach_image_run(run_id, image("screen.png", "aGVsbG8=", limits))
            .expect("image");

        assert_eq!(
            drafts
                .clear_run_if_generation(run_id, submitted)
                .expect("clear"),
            DraftClearOutcome::Cleared
        );
        let after = drafts.snapshot_run(run_id).expect("draft");
        assert!(after.text.is_empty());
        assert!(after.images.is_empty());
    }

    #[test]
    fn newer_attachment_prevents_submitted_generation_from_being_cleared() {
        let limits = RuntimeLimits::default();
        let run_id = RunId::new();
        let mut drafts = SessionDraftStore::new(limits);
        drafts
            .register_run(run_id, Some("session-a".to_owned()))
            .expect("register");
        let submitted = drafts
            .attach_image_run(run_id, image("first.png", "YWJj", limits))
            .expect("first image");
        drafts
            .attach_image_run(run_id, image("second.png", "ZGVm", limits))
            .expect("newer image");

        assert!(matches!(
            drafts
                .clear_run_if_generation(run_id, submitted)
                .expect("conditional clear"),
            DraftClearOutcome::Superseded { .. }
        ));
        assert_eq!(drafts.snapshot_run(run_id).expect("draft").images.len(), 2);
    }

    #[test]
    fn session_switch_keeps_image_drafts_scoped_to_their_pi_session() {
        let limits = RuntimeLimits::default();
        let run_id = RunId::new();
        let mut drafts = SessionDraftStore::new(limits);
        drafts
            .register_run(run_id, Some("session-a".to_owned()))
            .expect("register");
        drafts
            .attach_image_run(run_id, image("a.png", "YWJj", limits))
            .expect("session a image");
        drafts
            .reconcile_session(run_id, "session-b".to_owned())
            .expect("switch b");
        assert!(drafts.snapshot_run(run_id).expect("b").images.is_empty());
        drafts
            .attach_image_run(run_id, image("b.png", "ZGVm", limits))
            .expect("session b image");
        drafts
            .reconcile_session(run_id, "session-a".to_owned())
            .expect("switch a");
        let restored = drafts.snapshot_run(run_id).expect("a restored");
        assert_eq!(restored.images.len(), 1);
        assert_eq!(restored.images[0].file_name, "a.png");
    }
}
