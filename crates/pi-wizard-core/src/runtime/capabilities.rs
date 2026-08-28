use serde::{Deserialize, Serialize};

use crate::rpc::{CommandSummary, ModelSummary, ThinkingLevel};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCapabilities {
    revision: u64,
    models: Option<Vec<ModelSummary>>,
    thinking_levels: Option<Vec<ThinkingLevel>>,
    commands: Option<Vec<CommandSummary>>,
}

impl RunCapabilities {
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn models(&self) -> Option<&[ModelSummary]> {
        self.models.as_deref()
    }

    #[must_use]
    pub fn thinking_levels(&self) -> Option<&[ThinkingLevel]> {
        self.thinking_levels.as_deref()
    }

    #[must_use]
    pub fn commands(&self) -> Option<&[CommandSummary]> {
        self.commands.as_deref()
    }

    pub fn replace_models(&mut self, models: Vec<ModelSummary>) {
        self.models = Some(models);
        self.revision = self.revision.saturating_add(1);
    }

    pub fn replace_thinking_levels(&mut self, levels: Vec<ThinkingLevel>) {
        self.thinking_levels = Some(levels);
        self.revision = self.revision.saturating_add(1);
    }

    pub fn replace_commands(&mut self, commands: Vec<CommandSummary>) {
        self.commands = Some(commands);
        self.revision = self.revision.saturating_add(1);
    }
}
