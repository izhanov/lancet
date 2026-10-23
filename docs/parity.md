# Parity with qazyna (Go)

Everything qazyna does, Lancet must do. Source of truth:
[izhanov/qazyna@35da608](https://github.com/izhanov/qazyna/tree/35da608) (the
local clone and the installed qazyna are gone). Tick a box when the feature is
ported **and** covered by tests.

The command, flags, modes, `--json` output and MCP tool names keep the same
contract. Search internals are allowed to change (native FTS instead of an
`ILIKE` scan, prefilter instead of post-filtering) as long as `eval` does not
get worse.

## Status — 2026-10-23

Released [v0.1.1](https://github.com/izhanov/lancet/releases/tag/v0.1.1):
core library, all parsers, incremental indexing and `lt index / reindex /
search / eval`; install with `install.sh`.
[v0.1.2](https://github.com/izhanov/lancet/releases/tag/v0.1.2) adds `lt mcp`
and `lt skill`: registered in Claude Code as `lancet` (user scope), skill
`lancet-search` installed, so agents have notes search again.
v0.2.0 goes beyond qazyna: source code indexed by symbol with languages as
plugins (`lt lang add`, any tree-sitter grammar), `.gitignore` / `.ignore`
honored, batched writes (uscreen_2 without embedding: 16 min → 9 s), the
embedder told query from document.

Code index of uscreen_2 `app` + `lib` (4.8k files): 25k chunks, 216 MB,
11.7 min with `bge-m3` on an M5 (~40 chunks/s, Ollama-bound); search
0.01 s text, 0.13 s vector/hybrid (brute force, no vector index).

Eval on qazyna's corpus (113 files) and golden set (5 cases), recall@5 / MRR:

| mode   | qazyna      | lancet      |
|--------|-------------|-------------|
| vector | 0.80 / 0.600 | 0.80 / 0.600 |
| text   | 0.00 / 0.000 | 0.40 / 0.167 |
| hybrid | 0.80 / 0.417 | 0.80 / 0.567 |

Next, in order:

1. Grow the golden set (5 cases is too few to decide anything; a 37-case
   draft is waiting for review), then tune FTS for mixed Russian/English:
   stemming, stop words, n-grams, path and section in the FTS index. On the
   draft hybrid already beats vector (0.92 vs 0.86 recall@5).
2. `--tenant` / `tenants`, then picking the database by directory: a
   `.lancet.toml` (`db = "uscreen_2"`) found walking up from the working
   directory, like git finds `.git`; precedence flag > env > file > default.
   With it `lt mcp --about "code of the uscreen_2 Rails app"`, put into the
   tool descriptions: today every server introduces itself as "the user's
   notes", so an agent tells `lancet` from `lancet-uscreen` only by name;
   the `lancet-search` skill should learn about extra `lancet-*` indexes.
3. A golden set for code (15–20 real questions about uscreen_2): does
   semantic search beat an agent with grep? Decides how far code goes.
   It also settles the 600-byte packs for code between definitions, tried
   on routes.rb only ("cancel membership route in storefront account":
   vector rank 36 → 7, hybrid 8 → 2). They have a cost: data-heavy Ruby
   (seeds, big hashes) is all "code between definitions", and uscreen_2
   `app` + `lib` went from 25k chunks to over 55k texts embedded. Vocabulary gaps ("cancel
   subscription" for `cancel_membership`) stay: chunking cannot close them.
   And a chunker version in `meta`: indexing is incremental by mtime, so a
   new chunking in a new `lt` never reaches unchanged files — warn and
   suggest `lt index --force` when the stored version differs.
4. Images, in two steps. First text from the image, so it lands in the
   same `chunks` table and the same results as notes: OCR (macOS Vision or
   Tesseract) always, captions from a local vision model (Ollama) as an
   option, being seconds per image. Then, if visual search is wanted,
   multimodal embeddings (CLIP/SigLIP family): a different vector space, so
   a table of its own (`images`: path, mtime, thumbnail blob, vector) with
   its model under its own `meta` key — each space re-indexed on its own.
   Lance suits this (blobs next to vectors, several tables, S3), which also
   settles keeping LanceDB over SQLite + FTS5 + sqlite-vec.
   Before step two: a schema version in `meta` (`schema = 1`), so a newer
   `lt` recognizes an older database and migrates it (Lance can add
   columns without rewriting data) or asks for `reindex` instead of
   failing on an unexpected layout. Columns worth adding then: `kind`
   (text/code/image) and `language`.
5. Embedding throughput. Measured on the M5 with `bge-m3` (2026-10-23):
   61–64 texts/s whatever `OLLAMA_NUM_PARALLEL` (1/4/8), request
   concurrency (1/4/8) or batch size (16/32/64) — the GPU is saturated by
   one request, so local tuning gives nothing; `lt` itself idles (parsing
   ~5 ms a file). Indexing time is texts × tokens × model, so, in order:
   - **Embedding cache by content**: store a hash of each chunk's embedding
     text with the model id and reuse the stored vector when it matches.
     Today `--force`, a new chunking or an edit to one method re-embeds
     every chunk of the file; with the cache only changed chunks hit the
     GPU (an edit in the 113 KB stripe_provider.rb: one embedding, not a
     hundred). The 2026-10-24 re-chunking of uscreen_2 took 1 h 20 min.
   - **Progress and streaming batches**: `N of M files` (stderr, survives
     `--quiet` as a final line only), and flushing a batch inside a huge
     file instead of after it — a seed file or a CSV dump of IDs held
     ~10k texts without a commit for 20 minutes.
   - Fewer and shorter texts: noise out of the index (seeds, ID dumps,
     one-off tasks: tips in the README), the cost of long chunks and of
     the path/section prefix per text.
   - A `.lancetignore` (gitignore syntax, read by `lt` only), after
     tenants. `.ignore` is shared with ripgrep and often tracked — it is in
     uscreen_2 — so adding `lib/tasks/one_time/` there would hide the files
     from the whole team's `rg`; `.git/info/exclude` hides them from the
     user's own `rg` too.
   - A faster model, only by eval: quantized `bge-m3` (Q8 instead of F16),
     a smaller multilingual model; MLX / Core ML instead of llama.cpp is
     a hypothesis to measure; several Ollama URLs for remote GPUs.
6. The rest of the CLI: `list`, `flush`, `evals`, `setup`, `update`.
7. `lt-server` and tenant databases on S3.

Backlog, not decided: a vector index (IVF-PQ / HNSW with refine) once a
database passes a few hundred thousand chunks or ~100 ms per vector search;
enriching chunks at index time with an LLM (contextual retrieval) through
the user's agent CLI (`claude -p`, `codex exec`) or a local model; a
reference graph from the tags queries' `@reference.*` (`find_references`);
smaller binaries (LTO; `lt` is ~210 MB); the `lancet-search` skill telling
the agent to restate a question in the code's own terms, in English, before
searching, or to send both the original and the restated query ("как
пользователи меняют о себе инфу" misses both profile controllers in
uscreen_2, "user edits own profile name avatar password" puts them first);
search results saying which rankers matched (in hybrid a Russian query over
code is vector only and tops out at 0.5, which reads as a weak match);
rewriting queries inside `lt` (an LLM or synonyms), only if the skill rule
is not enough; text mode ranking a whole identifier above its split words
(`cancel_subscription_immediately` puts `cancel_immediately!` above the
method's own definition).

## CLI (`lt`)

- [x] `index <path>` — incremental by mtime, `--force` / `-f`
- [x] `reindex <path>`
- [x] `search <query>` — `--mode hybrid|vector|text`, `--limit`, `--json`, `--exclude-path` (repeatable)
- [x] query exclusion operators: `-word`, `-"a phrase"`; hyphen inside a word is not an operator
- [x] `eval [files...]` — `--mode`, `--limit`; recall@limit and MRR, per set and overall
- [ ] `evals` — list golden sets; `--edit` opens them in `$VISUAL`/`$EDITOR`, seeding a starter file
- [ ] `list` — indexed files with mtimes (`--json`)
- [ ] `flush` — remove all indexed data, asks for confirmation (`--force` / `-f`); refuses on a non-TTY stdin without `--force`
- [ ] `tenants` — list tenants that have a database in the tenants directory
- [ ] global `--tenant`: one isolated database per account
- [x] `mcp` — stdio MCP server (see below)
- [x] `skill install | uninstall | show` — Claude Code skill `lancet-search`
- [ ] `setup`
- [ ] `update` (`--check`) — self-update from GitHub Releases
- [x] colored output, `NO_COLOR` / non-TTY handling

Global flags, each with an env twin (`QAZYNA_*` in qazyna, `LANCET_*` in Lancet):

- [x] `--db` (`LANCET_DB`) — default `~/.local/share/lancet/db.lance`, `$XDG_DATA_HOME` honored
- [x] `--store` (`LANCET_STORE`) — default `lance`
- [x] `--embedder` (`LANCET_EMBEDDER`) — default `ollama`; `fake` for tests
- [x] `--ollama-url` (`LANCET_OLLAMA_URL`) — default `http://localhost:11434`
- [x] `--embed-model` (`LANCET_EMBED_MODEL`) — default `bge-m3`
- [x] `--fresh-reads` (`LANCET_FRESH_READS`) — default on; off pins the table version current at startup
- [ ] `--tenant` (`LANCET_TENANT`) — overrides `--db` with `<tenants-dir>/<tenant>/db.lance`
- [ ] `--tenants-dir` (`LANCET_TENANTS_DIR`) — local directory or `s3://bucket/prefix`
- [x] `--evals` (`LANCET_EVALS`) — golden sets directory, default `evals/` next to the database
- [x] `--verbose` (debug logging), `--quiet` (errors and the final summary only; wins over `--verbose`); logs go to stderr, stdout stays clean
- [ ] tenant name validation before anything touches the disk
- [x] Ctrl-C / SIGTERM stop gracefully

## MCP tools

- [x] `search_notes` — same parameters as `search` (mode, limit, exclude, exclude_path)
- [x] `list_files` — indexed files with mtimes
- [x] `index_status` — chunk count, embedder (tenant once tenants land)

## Parsers

- [x] Markdown — chunks by headings/paragraphs, section trail
- [x] PDF — text layer via `pdftotext`, section `p. N`
- [x] XML — subtree chunks, tag names as sections
- [x] CSV/TSV — rows with headers woven in, packed up to chunk size
- [x] DOCX — heading hierarchy

## Indexing and storage

- [x] embed the path tail together with the chunk
- [x] Ollama embedder: batching, retries with backoff, per-request timeout
- [x] refuse to search with a different embedder than the index was built with
- [x] refuse vectors of a different dimension
- [x] remove chunks of deleted files; clean up when a directory empties
- [x] readers see writes from other processes (MCP server stays fresh)

## HTTP server (`lt-server`)

- [ ] `GET /healthz`
- [ ] `GET /tenants`, `POST /tenants/{tenant}`
- [ ] `GET /tenants/{tenant}/files`
- [ ] `PUT /tenants/{tenant}/files/{path...}`, `DELETE /tenants/{tenant}/files/{path...}`
- [ ] `POST /tenants/{tenant}/index`
- [ ] `GET /tenants/{tenant}/search`
- [ ] tenant databases on S3 (credentials from the environment)

## Distribution

- [x] release binaries: darwin arm64/amd64, linux amd64/arm64 (Windows not planned for now)
- [x] `install.sh`

## Open decisions (rename)

- [x] data directory: `~/.local/share/lancet` (`$XDG_DATA_HOME` honored), golden sets in `evals/` next to the database
- [x] skill name: `lancet-search`
- [x] MCP server name in the Claude config: `lancet` (tools are `mcp__lancet__*`)
- [x] environment variable prefix: `LANCET_`
- [x] existing data: reindex from scratch, no migration
- [ ] `tenant` → `namespace`? (`--tenant` / `LANCET_TENANT` / `lt tenants`
  in qazyna). For: the local use is separate indexes of one person (notes,
  the uscreen_2 code), not accounts, and "tenant" reads as multi-account
  SaaS. Against: `tenant` is the qazyna contract and the right word for
  `lt-server` with one database per account on S3; LanceDB has its own
  "namespace" API (the `lance-namespace` crates), so the word would mean two
  things in one codebase. A third option: a neutral `--index <name>`
  locally, `tenant` kept for the server. Decide before implementing.
