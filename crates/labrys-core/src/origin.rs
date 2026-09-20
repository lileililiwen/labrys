use serde::{Deserialize, Serialize};

use crate::ids::ApplicationId;

/// Origin of an application.
///
/// Generated, imported, templated, forked, and external projects share one
/// [`crate::Application`] model; only this metadata differs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Origin {
    /// Created from a natural-language prompt inside Labrys.
    Generated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_summary: Option<String>,
    },
    /// Imported from a Git repository.
    ImportedGit {
        repository_url: String,
        #[serde(default = "default_branch")]
        branch: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit_sha: Option<String>,
    },
    /// Imported from a local directory.
    ImportedLocal { path: String },
    /// Created from a Labrys template.
    Template {
        template_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template_version: Option<String>,
    },
    /// Forked from another Labrys application.
    Fork {
        parent_application_id: ApplicationId,
    },
    /// References a project managed outside Labrys.
    External { external_ref: String },
}

fn default_branch() -> String {
    "main".to_string()
}

impl Origin {
    /// Stable discriminant used for logging and audit metadata.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Generated { .. } => "generated",
            Self::ImportedGit { .. } => "imported_git",
            Self::ImportedLocal { .. } => "imported_local",
            Self::Template { .. } => "template",
            Self::Fork { .. } => "fork",
            Self::External { .. } => "external",
        }
    }
}
