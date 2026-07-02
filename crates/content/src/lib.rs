//! Loaders for RON content. The single canonical place that touches the
//! filesystem for game data, so `core` can stay IO-free.

use std::fs;
use std::path::Path;

use thiserror::Error;

use bb_core::{Affix, Attachment, BaseItem, Enemy};

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

// ---- parse-from-string (embedded content) ----
//
// `include_str!`-friendly variants for builds with no filesystem at runtime
// (wasm, or a relocated native binary). The game embeds its content this way;
// `core`/`sim` keep using the path-based loaders for disk + hot-reload.

pub fn parse_affixes(name: &str, ron: &str) -> Result<Vec<Affix>, ContentError> {
    parse_ron(name, ron)
}

pub fn parse_bases(name: &str, ron: &str) -> Result<Vec<BaseItem>, ContentError> {
    parse_ron(name, ron)
}

pub fn parse_attachments(name: &str, ron: &str) -> Result<Vec<Attachment>, ContentError> {
    parse_ron(name, ron)
}

pub fn parse_enemies(name: &str, ron: &str) -> Result<Vec<Enemy>, ContentError> {
    parse_ron(name, ron)
}

fn parse_ron<T: serde::de::DeserializeOwned>(name: &str, ron: &str) -> Result<T, ContentError> {
    ron::de::from_str(ron).map_err(|source| ContentError::Parse {
        path: name.to_string(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The shipped content must parse through the embedded (`include_str!`)
    // path the game uses on wasm — guard against a malformed-RON regression
    // that the disk loaders (exercised only at runtime) wouldn't catch in CI.
    #[test]
    fn embedded_content_parses() {
        assert!(!parse_enemies("enemies.ron", include_str!("../data/enemies.ron"))
            .unwrap()
            .is_empty());
        assert!(!parse_bases("bases.ron", include_str!("../data/bases.ron"))
            .unwrap()
            .is_empty());
        assert!(!parse_affixes("affixes.ron", include_str!("../data/affixes.ron"))
            .unwrap()
            .is_empty());
        assert!(!parse_attachments("attachments.ron", include_str!("../data/attachments.ron"))
            .unwrap()
            .is_empty());
    }
}
