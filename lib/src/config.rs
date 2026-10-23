//! Where Lancet keeps its data by default.

use std::path::PathBuf;

/// The XDG data location, `~/.local/share/lancet`, so an installed binary
/// finds the same data from any working directory. `$XDG_DATA_HOME` is
/// honored. `None` when no home directory can be determined.
pub fn data_dir() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(base) => PathBuf::from(base),
        None => std::env::home_dir()?.join(".local").join("share"),
    };
    Some(base.join("lancet"))
}

/// `~/.local/share/lancet/db.lance`.
pub fn default_db_path() -> PathBuf {
    data_dir().map_or_else(
        || PathBuf::from("data/lancet.lance"),
        |d| d.join("db.lance"),
    )
}

/// Language plugins for source code (see [`crate::lang`]):
/// `~/.local/share/lancet/languages`.
pub fn default_languages_dir() -> PathBuf {
    data_dir().map_or_else(|| PathBuf::from("data/languages"), |d| d.join("languages"))
}

/// Golden sets for `lt eval` live next to the database:
/// `~/.local/share/lancet/evals`.
pub fn default_evals_dir() -> PathBuf {
    data_dir().map_or_else(|| PathBuf::from("data/evals"), |d| d.join("evals"))
}
