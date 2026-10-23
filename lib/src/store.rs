//! Chunk storage on LanceDB: one `chunks` table with vectors and a full-text
//! index, plus a small `meta` key/value table.

use std::collections::HashMap;
use std::sync::Arc;

use futures::TryStreamExt;
use lancedb::arrow::arrow_array::{
    Array, FixedSizeListArray, Float32Array, Int64Array, RecordBatch, RecordBatchIterator,
    StringArray,
};
use lancedb::arrow::arrow_schema::{DataType, Field, Schema, SchemaRef};
use lancedb::index::Index;
use lancedb::index::scalar::{FtsIndexBuilder, FullTextSearchQuery};
use lancedb::query::{ExecutableQuery, QueryBase, QueryExecutionOptions, Select};
use lancedb::rerankers::rrf::RRFReranker;
use lancedb::table::OptimizeAction;
use lancedb::{Connection, Table};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::parse::Chunk;
use crate::{Error, Result};

const CHUNKS: &str = "chunks";
const META: &str = "meta";

/// RRF constant: each list contributes 1/(K + rank), rank from 0.
const RRF_K: f32 = 60.0;

/// One indexed file: its chunks plus a vector per chunk.
#[derive(Debug, Clone)]
pub struct Document {
    pub path: String,
    pub mtime: i64,
    pub chunks: Vec<Chunk>,
    pub vectors: Vec<Vec<f32>>,
}

/// One found chunk with its score.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchResult {
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub section: String,
    pub text: String,
    pub ordinal: usize,
    /// Higher is better, 1 is the best possible: cosine similarity for
    /// vector search, BM25 relative to the top hit for text search, fused
    /// RRF scaled so that #1 in both lists gives 1 for hybrid search.
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct OpenOptions {
    /// Re-open the table on every read to see versions written by other
    /// processes (on by default). Without it a long-lived process (the MCP
    /// server) keeps serving the version that was current when it opened.
    pub fresh_reads: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self { fresh_reads: true }
    }
}

pub struct Store {
    db: Connection,
    fresh_reads: bool,
    /// The chunks table handle when reads are pinned (fresh_reads off).
    pinned: Mutex<Option<Table>>,
    /// Serializes this process's writes; concurrent merge-inserts into one
    /// table would conflict and be retried by Lance anyway.
    writes: Mutex<()>,
}

impl Store {
    pub async fn open(uri: &str) -> Result<Self> {
        Self::open_with(uri, OpenOptions::default()).await
    }

    pub async fn open_with(uri: &str, opts: OpenOptions) -> Result<Self> {
        // A tenant's database sits in a directory of its own that may not
        // exist yet. Object-store URIs (s3://...) have no parents to create.
        if !uri.contains("://")
            && let Some(parent) = std::path::Path::new(uri).parent()
        {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
        }

        let db = lancedb::connect(uri).execute().await?;
        let store = Self {
            db,
            fresh_reads: opts.fresh_reads,
            pinned: Mutex::new(None),
            writes: Mutex::new(()),
        };
        if !store.fresh_reads {
            *store.pinned.lock().await = store.open_table(CHUNKS).await?;
        }
        Ok(store)
    }

    /// Writes all chunks of one file in a single commit, replacing whatever
    /// was stored for the same path: re-indexing never duplicates, and
    /// chunks that disappeared from the file are removed.
    pub async fn add_document(&self, doc: &Document) -> Result<()> {
        self.add_documents(std::slice::from_ref(doc)).await
    }

    /// [`Store::add_document`] for many files in one commit. Every commit
    /// is a new table version with files of its own, and its cost grows
    /// with the number of versions: a commit per file made indexing a code
    /// repository take minutes of mostly filesystem work.
    pub async fn add_documents(&self, docs: &[Document]) -> Result<()> {
        let docs: Vec<&Document> = docs.iter().filter(|d| !d.chunks.is_empty()).collect();
        let Some(first) = docs.first() else {
            return Ok(());
        };
        for doc in &docs {
            if doc.chunks.len() != doc.vectors.len() {
                return Err(Error::ChunksVectorsMismatch {
                    chunks: doc.chunks.len(),
                    vectors: doc.vectors.len(),
                });
            }
        }
        let dim = first.vectors[0].len();

        let _write = self.writes.lock().await;
        let table = self.writable_chunks(dim).await?;
        let batches = docs
            .iter()
            .map(|doc| chunk_batch(doc, dim))
            .collect::<Result<Vec<_>>>()?;
        let schema = batches[0].schema();
        let paths: Vec<String> = docs.iter().map(|d| quote(&d.path)).collect();

        let mut merge = table.merge_insert(&["id"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all()
            .when_not_matched_by_source_delete(Some(format!("path IN ({})", paths.join(", "))));
        merge
            .execute(Box::new(RecordBatchIterator::new(
                batches.into_iter().map(Ok),
                schema,
            )))
            .await?;

        ensure_fts_index(&table).await
    }

    pub async fn delete_by_path(&self, path: &str) -> Result<()> {
        let _write = self.writes.lock().await;
        if let Some(table) = self.chunks().await? {
            table.delete(&path_filter(path)).await?;
        }
        Ok(())
    }

    pub async fn count(&self) -> Result<usize> {
        match self.chunks().await? {
            Some(table) => Ok(table.count_rows(None).await?),
            None => Ok(0),
        }
    }

    /// Every indexed file with its stored mtime, enabling incremental
    /// indexing: unchanged files are skipped entirely.
    pub async fn paths(&self) -> Result<HashMap<String, i64>> {
        let Some(table) = self.chunks().await? else {
            return Ok(HashMap::new());
        };
        let batches = collect(
            table
                .query()
                .select(Select::columns(&["path", "mtime"]))
                .execute()
                .await?,
        )
        .await?;

        let mut paths = HashMap::new();
        for b in &batches {
            let path = string_column(b, "path")?;
            let mtime = column::<Int64Array>(b, "mtime")?;
            for i in 0..b.num_rows() {
                paths.insert(path.value(i).to_string(), mtime.value(i));
            }
        }
        Ok(paths)
    }

    /// The `limit` nearest chunks to a unit-normalized query vector, best
    /// first. `filter` is an SQL predicate applied before the search.
    pub async fn search_vector(
        &self,
        vector: &[f32],
        limit: usize,
        filter: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let Some(table) = self.chunks().await? else {
            return Ok(Vec::new());
        };
        let mut query = table
            .query()
            .nearest_to(vector)?
            .select(result_columns(&["_distance"]))
            .limit(limit);
        if let Some(f) = filter {
            query = query.only_if(f);
        }
        let batches = collect(query.execute().await?).await?;

        // For unit vectors the L2 _distance d (squared euclidean) relates to
        // cosine similarity as cos = 1 - d/2.
        to_results(&batches, "_distance", |d| 1.0 - d / 2.0)
    }

    /// BM25 full-text search over chunk text, best first.
    pub async fn search_text(
        &self,
        query: &str,
        limit: usize,
        filter: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let Some(table) = self.chunks().await? else {
            return Ok(Vec::new());
        };
        let mut q = table
            .query()
            .full_text_search(FullTextSearchQuery::new(query.to_string()))
            .select(result_columns(&["_score"]))
            .limit(limit);
        if let Some(f) = filter {
            q = q.only_if(f);
        }
        let batches = collect(q.execute().await?).await?;

        let mut results = to_results(&batches, "_score", |s| s)?;
        // BM25 has no upper bound; scale to the top hit so scores stay 0..1.
        if let Some(top) = results.first().map(|r| r.score).filter(|&s| s > 0.0) {
            results.iter_mut().for_each(|r| r.score /= top);
        }
        Ok(results)
    }

    /// Vector and full-text search fused with Reciprocal Rank Fusion:
    /// agreement between the two beats a high position in either alone.
    pub async fn search_hybrid(
        &self,
        vector: &[f32],
        query: &str,
        limit: usize,
        filter: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let Some(table) = self.chunks().await? else {
            return Ok(Vec::new());
        };
        // Each half returns `fetch` rows, giving the fusion room to work with.
        // No projection: both halves share it, yet each has its own score
        // column; left alone, Lance adds the right one to each.
        let fetch = limit * 3;
        let mut q = table
            .query()
            .nearest_to(vector)?
            .full_text_search(FullTextSearchQuery::new(query.to_string()))
            .rerank(Arc::new(RRFReranker::new(RRF_K)))
            .limit(fetch);
        if let Some(f) = filter {
            q = q.only_if(f);
        }
        let batches = collect(q.execute_hybrid(QueryExecutionOptions::default()).await?).await?;

        // #1 in both lists earns 2/K: that is the 1.0 mark.
        let mut results = to_results(&batches, "_relevance_score", |s| {
            s / (2.0 / f64::from(RRF_K))
        })?;
        results.truncate(limit);
        Ok(results)
    }

    /// Database-level metadata, e.g. which embedder built the index. Empty
    /// for a fresh database.
    pub async fn meta(&self) -> Result<HashMap<String, String>> {
        let Some(table) = self.open_table(META).await? else {
            return Ok(HashMap::new());
        };
        let batches = collect(table.query().execute().await?).await?;

        let mut meta = HashMap::new();
        for b in &batches {
            let (keys, values) = (string_column(b, "key")?, string_column(b, "value")?);
            for i in 0..b.num_rows() {
                meta.insert(keys.value(i).to_string(), values.value(i).to_string());
            }
        }
        Ok(meta)
    }

    /// Replaces the whole metadata map in one commit.
    pub async fn set_meta(&self, meta: &HashMap<String, String>) -> Result<()> {
        let _write = self.writes.lock().await;
        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, false),
        ]));
        let mut keys: Vec<&String> = meta.keys().collect();
        keys.sort();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from_iter_values(
                    keys.iter().map(|k| k.as_str()),
                )),
                Arc::new(StringArray::from_iter_values(
                    keys.iter().map(|k| meta[*k].as_str()),
                )),
            ],
        )
        .map_err(lancedb::Error::from)?;

        let table = match self.open_table(META).await? {
            Some(t) => t,
            None => {
                self.db
                    .create_empty_table(META, schema.clone())
                    .execute()
                    .await?
            }
        };
        let mut merge = table.merge_insert(&["key"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all()
            .when_not_matched_by_source_delete(None);
        merge
            .execute(Box::new(RecordBatchIterator::new([Ok(batch)], schema)))
            .await?;
        Ok(())
    }

    /// Wipes all indexed data and metadata.
    pub async fn reset(&self) -> Result<()> {
        let _write = self.writes.lock().await;
        *self.pinned.lock().await = None;
        for name in [CHUNKS, META] {
            match self.db.drop_table(name, &[]).await {
                Ok(()) | Err(lancedb::Error::TableNotFound { .. }) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Compacts small fragments, prunes old versions and folds unindexed
    /// rows into the indices. Searches stay correct without it (unindexed
    /// rows are scanned), only slower; run it after a batch of writes.
    pub async fn optimize(&self) -> Result<()> {
        let _write = self.writes.lock().await;
        if let Some(table) = self.chunks().await? {
            table.optimize(OptimizeAction::All).await?;
        }
        Ok(())
    }

    /// The chunks table for reading: freshly opened, or the pinned handle.
    async fn chunks(&self) -> Result<Option<Table>> {
        if self.fresh_reads {
            return self.open_table(CHUNKS).await;
        }
        Ok(self.pinned.lock().await.clone())
    }

    /// The chunks table for writing, created on first use when the vector
    /// dimension becomes known. Must hold `writes`.
    async fn writable_chunks(&self, dim: usize) -> Result<Table> {
        let table = match self.chunks().await? {
            Some(table) => table,
            None => {
                let table = self
                    .db
                    .create_empty_table(CHUNKS, chunk_schema(dim))
                    .execute()
                    .await?;
                if !self.fresh_reads {
                    *self.pinned.lock().await = Some(table.clone());
                }
                table
            }
        };
        let stored = vector_dim(table.schema().await?.as_ref());
        if stored != Some(dim) {
            return Err(Error::DimensionMismatch {
                table: stored.unwrap_or(0),
                got: dim,
            });
        }
        Ok(table)
    }

    async fn open_table(&self, name: &str) -> Result<Option<Table>> {
        match self.db.open_table(name).execute().await {
            Ok(table) => Ok(Some(table)),
            Err(lancedb::Error::TableNotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Mixed Russian/English notes: Lance stems a single language per index, so
/// no stemming until the golden set shows a better setup. Lowercasing, ASCII
/// folding and English stop words stay at their defaults.
async fn ensure_fts_index(table: &Table) -> Result<()> {
    let indexed = table
        .list_indices()
        .await?
        .iter()
        .any(|idx| idx.columns.iter().any(|c| c == "text"));
    if !indexed {
        let params = FtsIndexBuilder::default().stem(false);
        table
            .create_index(&["text"], Index::FTS(params))
            .execute()
            .await?;
    }
    Ok(())
}

fn chunk_schema(dim: usize) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("section", DataType::Utf8, true),
        Field::new("text", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dim as i32,
            ),
            false,
        ),
        Field::new("mtime", DataType::Int64, false),
    ]))
}

fn vector_dim(schema: &Schema) -> Option<usize> {
    match schema.field_with_name("vector").ok()?.data_type() {
        DataType::FixedSizeList(_, n) => Some(*n as usize),
        _ => None,
    }
}

fn chunk_batch(doc: &Document, dim: usize) -> Result<RecordBatch> {
    if let Some(v) = doc.vectors.iter().find(|v| v.len() != dim) {
        return Err(Error::DimensionMismatch {
            table: dim,
            got: v.len(),
        });
    }
    let n = doc.chunks.len();
    let values = Float32Array::from_iter_values(doc.vectors.iter().flatten().copied());
    let vectors = FixedSizeListArray::try_new(
        Arc::new(Field::new("item", DataType::Float32, true)),
        dim as i32,
        Arc::new(values),
        None,
    )
    .map_err(lancedb::Error::from)?;

    let batch = RecordBatch::try_new(
        chunk_schema(dim),
        vec![
            Arc::new(StringArray::from_iter_values(
                doc.chunks
                    .iter()
                    .map(|c| format!("{}#{}", doc.path, c.ordinal)),
            )),
            Arc::new(StringArray::from_iter_values(std::iter::repeat_n(
                doc.path.as_str(),
                n,
            ))),
            Arc::new(StringArray::from_iter(
                doc.chunks.iter().map(|c| Some(c.section.as_str())),
            )),
            Arc::new(StringArray::from_iter_values(
                doc.chunks.iter().map(|c| c.text.as_str()),
            )),
            Arc::new(vectors),
            Arc::new(Int64Array::from(vec![doc.mtime; n])),
        ],
    )
    .map_err(lancedb::Error::from)?;
    Ok(batch)
}

/// The chunk columns plus the score columns the query produces. Naming the
/// score columns keeps Lance from auto-projecting them, a deprecated behavior.
fn result_columns(scores: &[&str]) -> Select {
    Select::columns(&[&["id", "path", "section", "text"], scores].concat())
}

/// Rows in the order the query returned them, scored from `score_col`.
fn to_results(
    batches: &[RecordBatch],
    score_col: &str,
    score: impl Fn(f64) -> f64,
) -> Result<Vec<SearchResult>> {
    let mut results = Vec::new();
    for b in batches {
        let ids = string_column(b, "id")?;
        let paths = string_column(b, "path")?;
        let sections = string_column(b, "section")?;
        let texts = string_column(b, "text")?;
        let scores = column::<Float32Array>(b, score_col)?;
        for i in 0..b.num_rows() {
            let id = ids.value(i);
            results.push(SearchResult {
                path: paths.value(i).to_string(),
                section: if sections.is_null(i) {
                    String::new()
                } else {
                    sections.value(i).to_string()
                },
                text: texts.value(i).to_string(),
                ordinal: id
                    .rsplit_once('#')
                    .and_then(|(_, n)| n.parse().ok())
                    .unwrap_or(0),
                score: score(f64::from(scores.value(i))),
            });
        }
    }
    Ok(results)
}

async fn collect(stream: lancedb::arrow::SendableRecordBatchStream) -> Result<Vec<RecordBatch>> {
    Ok(stream.try_collect().await?)
}

fn string_column<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    column(b, name)
}

fn column<'a, T: Array + 'static>(b: &'a RecordBatch, name: &str) -> Result<&'a T> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<T>())
        .ok_or_else(|| {
            Error::Schema(format!(
                "column {name:?} is missing or has an unexpected type"
            ))
        })
}

fn path_filter(path: &str) -> String {
    format!("path = {}", quote(path))
}

/// An SQL string literal.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(test)]
mod tests;
