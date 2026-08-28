use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::RuntimeLimits;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WidgetPlacement {
    #[default]
    AboveEditor,
    BelowEditor,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionUiSnapshot {
    pub statuses: Vec<ExtensionStatusSnapshot>,
    pub widgets: Vec<ExtensionWidgetSnapshot>,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionStatusSnapshot {
    pub key: String,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionWidgetSnapshot {
    pub key: String,
    pub widget: ExtensionWidget,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionWidget {
    pub lines: Vec<String>,
    pub placement: WidgetPlacement,
}

/// Bounded persistent projection for RPC fire-and-forget extension UI state.
///
/// Dialogs are owned separately by request ID. Notifications are transient.
/// `set_editor_text` belongs to the session draft owner.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExtensionUiState {
    statuses: HashMap<String, String>,
    widgets: HashMap<String, ExtensionWidget>,
    title: Option<String>,
    max_entries: usize,
    max_bytes: usize,
    used_bytes: usize,
}

impl ExtensionUiState {
    #[must_use]
    pub fn new(limits: RuntimeLimits) -> Self {
        Self {
            statuses: HashMap::new(),
            widgets: HashMap::new(),
            title: None,
            max_entries: limits.max_extension_ui_entries_per_run,
            max_bytes: limits.max_extension_ui_bytes_per_run,
            used_bytes: 0,
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> ExtensionUiSnapshot {
        let mut statuses: Vec<_> = self
            .statuses
            .iter()
            .map(|(key, text)| ExtensionStatusSnapshot {
                key: key.clone(),
                text: text.clone(),
            })
            .collect();
        statuses.sort_by(|left, right| left.key.cmp(&right.key));
        let mut widgets: Vec<_> = self
            .widgets
            .iter()
            .map(|(key, widget)| ExtensionWidgetSnapshot {
                key: key.clone(),
                widget: widget.clone(),
            })
            .collect();
        widgets.sort_by(|left, right| left.key.cmp(&right.key));
        ExtensionUiSnapshot {
            statuses,
            widgets,
            title: self.title.clone(),
        }
    }

    pub fn set_status(
        &mut self,
        key: impl Into<String>,
        text: Option<String>,
    ) -> Result<(), ExtensionUiError> {
        let key = key.into();
        let old = self.statuses.get(&key).map(String::as_str);
        let old_bytes = old.map_or(0, |value| key.len() + value.len());
        let new_bytes = text.as_ref().map_or(0, |value| key.len() + value.len());
        let adds_entry = old.is_none() && text.is_some();
        self.ensure_capacity(adds_entry, old_bytes, new_bytes)?;

        match text {
            Some(text) => {
                self.statuses.insert(key, text);
            }
            None => {
                self.statuses.remove(&key);
            }
        }
        self.used_bytes = self.used_bytes - old_bytes + new_bytes;
        Ok(())
    }

    pub fn set_widget(
        &mut self,
        key: impl Into<String>,
        widget: Option<ExtensionWidget>,
    ) -> Result<(), ExtensionUiError> {
        let key = key.into();
        let old_bytes = self
            .widgets
            .get(&key)
            .map_or(0, |value| key.len() + widget_bytes(value));
        let new_bytes = widget
            .as_ref()
            .map_or(0, |value| key.len() + widget_bytes(value));
        let adds_entry = !self.widgets.contains_key(&key) && widget.is_some();
        self.ensure_capacity(adds_entry, old_bytes, new_bytes)?;

        match widget {
            Some(widget) => {
                self.widgets.insert(key, widget);
            }
            None => {
                self.widgets.remove(&key);
            }
        }
        self.used_bytes = self.used_bytes - old_bytes + new_bytes;
        Ok(())
    }

    pub fn set_title(&mut self, title: Option<String>) -> Result<(), ExtensionUiError> {
        let old_bytes = self.title.as_ref().map_or(0, String::len);
        let new_bytes = title.as_ref().map_or(0, String::len);
        self.ensure_byte_capacity(old_bytes, new_bytes)?;
        self.title = title;
        self.used_bytes = self.used_bytes - old_bytes + new_bytes;
        Ok(())
    }

    #[must_use]
    pub fn status_count(&self) -> usize {
        self.statuses.len()
    }

    #[must_use]
    pub fn widget_count(&self) -> usize {
        self.widgets.len()
    }

    #[must_use]
    pub const fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn statuses(&self) -> impl Iterator<Item = (&str, &str)> {
        self.statuses
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    pub fn widgets(&self) -> impl Iterator<Item = (&str, &ExtensionWidget)> {
        self.widgets
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }

    fn ensure_capacity(
        &self,
        adds_entry: bool,
        old_bytes: usize,
        new_bytes: usize,
    ) -> Result<(), ExtensionUiError> {
        if adds_entry && self.statuses.len() + self.widgets.len() >= self.max_entries {
            return Err(ExtensionUiError::EntryLimit {
                limit: self.max_entries,
            });
        }
        self.ensure_byte_capacity(old_bytes, new_bytes)
    }

    fn ensure_byte_capacity(
        &self,
        old_bytes: usize,
        new_bytes: usize,
    ) -> Result<(), ExtensionUiError> {
        let next = self
            .used_bytes
            .saturating_sub(old_bytes)
            .saturating_add(new_bytes);
        if next > self.max_bytes {
            return Err(ExtensionUiError::ByteLimit {
                attempted: next,
                limit: self.max_bytes,
            });
        }
        Ok(())
    }
}

fn widget_bytes(widget: &ExtensionWidget) -> usize {
    widget.lines.iter().map(String::len).sum()
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ExtensionUiError {
    #[error("extension UI entry limit {limit} reached")]
    EntryLimit { limit: usize },
    #[error("extension UI state would use {attempted} bytes, exceeding limit {limit}")]
    ByteLimit { attempted: usize, limit: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyed_status_and_widget_state_is_bounded_by_entry_count() {
        let limits = RuntimeLimits {
            max_extension_ui_entries_per_run: 1,
            ..RuntimeLimits::default()
        };
        let mut state = ExtensionUiState::new(limits);
        state
            .set_status("build", Some("running".to_owned()))
            .expect("first entry");

        assert_eq!(
            state.set_widget(
                "queue",
                Some(ExtensionWidget {
                    lines: vec!["waiting".to_owned()],
                    placement: WidgetPlacement::BelowEditor,
                }),
            ),
            Err(ExtensionUiError::EntryLimit { limit: 1 })
        );
        assert_eq!(state.status_count(), 1);
        assert_eq!(state.widget_count(), 0);
    }

    #[test]
    fn rejected_oversized_replacement_preserves_previous_state_and_accounting() {
        let limits = RuntimeLimits {
            max_extension_ui_bytes_per_run: 16,
            ..RuntimeLimits::default()
        };
        let mut state = ExtensionUiState::new(limits);
        state
            .set_status("k", Some("ok".to_owned()))
            .expect("small status");
        let before = state.used_bytes();

        assert!(matches!(
            state.set_status("k", Some("this is much too large".to_owned())),
            Err(ExtensionUiError::ByteLimit { .. })
        ));
        assert_eq!(state.used_bytes(), before);
        assert_eq!(state.status_count(), 1);
    }

    #[test]
    fn clearing_entries_releases_budget() {
        let limits = RuntimeLimits {
            max_extension_ui_entries_per_run: 1,
            ..RuntimeLimits::default()
        };
        let mut state = ExtensionUiState::new(limits);
        state
            .set_status("build", Some("running".to_owned()))
            .expect("status");
        state.set_status("build", None).expect("clear status");
        state
            .set_widget(
                "queue",
                Some(ExtensionWidget {
                    lines: vec!["waiting".to_owned()],
                    placement: WidgetPlacement::AboveEditor,
                }),
            )
            .expect("entry budget released");

        assert_eq!(state.status_count(), 0);
        assert_eq!(state.widget_count(), 1);
    }
}
