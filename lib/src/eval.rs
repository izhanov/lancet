//! Search quality against golden sets: real queries and the files that count
//! as a correct answer.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::store::SearchResult;
use crate::{Error, Result};

/// One golden-set entry. Expectations are file-level path suffixes — chunk
/// ordinals shift whenever the chunking changes, "the right file was found"
/// does not.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Case {
    pub query: String,
    pub expect: Vec<String>,
}

pub fn load_golden(path: &Path) -> Result<Vec<Case>> {
    let golden_error = |message: String| Error::Golden {
        path: path.display().to_string(),
        message,
    };
    let data = std::fs::read_to_string(path).map_err(|e| Error::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let cases: Vec<Case> = serde_norway::from_str::<Option<Vec<Case>>>(&data)
        .map_err(|e| golden_error(e.to_string()))?
        .unwrap_or_default();
    if cases.is_empty() {
        return Err(golden_error("no cases".into()));
    }
    if let Some(i) = cases
        .iter()
        .position(|c| c.query.is_empty() || c.expect.is_empty())
    {
        return Err(golden_error(format!(
            "case {} needs both query and expect",
            i + 1
        )));
    }
    Ok(cases)
}

/// The golden sets (`*.yaml`, `*.yml`) in a directory, sorted.
pub fn golden_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml" || e == "yml"))
        .collect();
    if files.is_empty() {
        return Err(Error::Golden {
            path: dir.display().to_string(),
            message:
                "no golden sets (*.yaml) — put one there, point --evals (or LANCET_EVALS) at your \
                      directory, or pass a file: lt eval <golden.yaml>"
                    .into(),
        });
    }
    files.sort();
    Ok(files)
}

/// The 1-based rank of the first result whose path ends with one of the
/// expected suffixes, with that path. Results are chunks, so the first chunk
/// of an expected file sets the file's rank.
pub fn first_expected_rank<'a>(
    results: &'a [SearchResult],
    expect: &[String],
) -> Option<(usize, &'a str)> {
    results
        .iter()
        .position(|r| expect.iter().any(|want| r.path.ends_with(want.as_str())))
        .map(|i| (i + 1, results[i].path.as_str()))
}

/// Recall@limit (an expected file made the top results) and MRR (mean of
/// 1/rank of the first correct file, 0 for a miss).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Score {
    pub cases: usize,
    pub hits: usize,
    reciprocal_ranks: f64,
}

impl Score {
    pub fn add(&mut self, rank: Option<usize>) {
        self.cases += 1;
        if let Some(rank) = rank {
            self.hits += 1;
            self.reciprocal_ranks += 1.0 / rank as f64;
        }
    }

    pub fn merge(&mut self, other: Score) {
        self.cases += other.cases;
        self.hits += other.hits;
        self.reciprocal_ranks += other.reciprocal_ranks;
    }

    pub fn recall(&self) -> f64 {
        ratio(self.hits as f64, self.cases)
    }

    pub fn mrr(&self) -> f64 {
        ratio(self.reciprocal_ranks, self.cases)
    }
}

fn ratio(x: f64, n: usize) -> f64 {
    if n == 0 { 0.0 } else { x / n as f64 }
}

#[cfg(test)]
mod tests;
