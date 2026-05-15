//! Loaders for RON content. The single canonical place that touches the
//! filesystem for game data, so `core` can stay IO-free.

use std::fs;
use std::path::Path;

use thiserror::Error;

use h2b_core::{Affix, Attachment, BaseItem, Enemy};

#[derive(Debug, Error)]
pub enum ContentError {
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: ron::error::SpannedError,
    },
}

pub fn load_affixes(path: &Path) -> Result<Vec<Affix>, ContentError> {
    load_ron(path)
}

pub fn load_bases(path: &Path) -> Result<Vec<BaseItem>, ContentError> {
    load_ron(path)
}

pub fn load_attachments(path: &Path) -> Result<Vec<Attachment>, ContentError> {
    load_ron(path)
}

pub fn load_enemies(path: &Path) -> Result<Vec<Enemy>, ContentError> {
    load_ron(path)
}

fn load_ron<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ContentError> {
    let bytes = fs::read(path).map_err(|source| ContentError::Io {
        path: path.display().to_string(),
        source,
    })?;
    ron::de::from_bytes(&bytes).map_err(|source| ContentError::Parse {
        path: path.display().to_string(),
        source,
    })
}
