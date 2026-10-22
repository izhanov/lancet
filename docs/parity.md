# Parity with qazyna (Go)

Everything qazyna does, Lancet must do before qazyna is archived.
Source of truth: `~/myway/qazyna` at `35da608`. Tick a box when the feature
is ported **and** covered by tests.

The command, flags, modes, `--json` output and MCP tool names keep the same
contract. Search internals are allowed to change (native FTS instead of an
`ILIKE` scan, prefilter instead of post-filtering) as long as `eval` does not
get worse.

## CLI (`lt`)

- [ ] `index <path>` — incremental by mtime, `--force` / `-f`
- [ ] `reindex <path>`
- [ ] `search <query>` — `--mode hybrid|vector|text`, `--limit`, `--json`, `--exclude-path` (repeatable)
- [ ] query exclusion operators: `-word`, `-"a phrase"`; hyphen inside a word is not an operator
- [ ] `eval [files...]` — `--mode`, `--limit`; recall@limit and MRR, per set and overall
- [ ] `evals` — list golden sets; `--edit` opens them in `$VISUAL`/`$EDITOR`, seeding a starter file
- [ ] `list` — indexed files with mtimes (`--json`)
- [ ] `flush` — remove all indexed data, asks for confirmation (`--force` / `-f`); refuses on a non-TTY stdin without `--force`
- [ ] `tenants` — list tenants that have a database in the tenants directory
- [ ] global `--tenant`: one isolated database per account
- [ ] `mcp` — stdio MCP server (see below)
- [ ] `skill install | uninstall | show` — Claude Code skill
- [ ] `setup`
- [ ] `update` (`--check`) — self-update from GitHub Releases
- [ ] colored output, `NO_COLOR` / non-TTY handling

Global flags, each with an env twin (`QAZYNA_*` in qazyna, `LANCET_*` in Lancet):

- [ ] `--db` (`QAZYNA_DB`) — default `~/.local/share/lancet/db.lance`, `$XDG_DATA_HOME` honored
- [ ] `--store` (`QAZYNA_STORE`) — default `lance`
- [ ] `--embedder` (`QAZYNA_EMBEDDER`) — default `ollama`
- [ ] `--ollama-url` (`QAZYNA_OLLAMA_URL`) — default `http://localhost:11434`
- [ ] `--embed-model` (`QAZYNA_EMBED_MODEL`) — default `bge-m3`
- [ ] `--fresh-reads` (`QAZYNA_FRESH_READS`) — default on; off pins the table version current at startup
- [ ] `--tenant` (`QAZYNA_TENANT`) — overrides `--db` with `<tenants-dir>/<tenant>/db.lance`
- [ ] `--tenants-dir` (`QAZYNA_TENANTS_DIR`) — local directory or `s3://bucket/prefix`
- [ ] `--evals` (`QAZYNA_EVALS`) — golden sets directory, default `evals/` next to the database
- [ ] `--verbose` (debug logging), `--quiet` (errors and the final summary only; wins over `--verbose`); logs go to stderr, stdout stays clean
- [ ] tenant name validation before anything touches the disk
- [ ] Ctrl-C / SIGTERM stop gracefully

## MCP tools

- [ ] `search_notes` — same parameters as `search` (mode, limit, exclude, exclude_path)
- [ ] `list_files` — indexed files with mtimes
- [ ] `index_status` — chunk count, embedder

## Parsers

- [ ] Markdown — chunks by headings/paragraphs, section trail
- [ ] PDF — text layer via `pdftotext`, section `p. N`
- [ ] XML — subtree chunks, tag names as sections
- [ ] CSV/TSV — rows with headers woven in, packed up to chunk size
- [ ] DOCX — heading hierarchy

## Indexing and storage

- [ ] embed the path tail together with the chunk
- [ ] Ollama embedder: batching, retries with backoff, per-request timeout
- [ ] refuse to search with a different embedder than the index was built with
- [ ] refuse vectors of a different dimension
- [ ] remove chunks of deleted files; clean up when a directory empties
- [ ] readers see writes from other processes (MCP server stays fresh)

## HTTP server (`lt-server`)

- [ ] `GET /healthz`
- [ ] `GET /tenants`, `POST /tenants/{tenant}`
- [ ] `GET /tenants/{tenant}/files`
- [ ] `PUT /tenants/{tenant}/files/{path...}`, `DELETE /tenants/{tenant}/files/{path...}`
- [ ] `POST /tenants/{tenant}/index`
- [ ] `GET /tenants/{tenant}/search`
- [ ] tenant databases on S3 (credentials from the environment)

## Distribution

- [ ] release binaries: darwin arm64/amd64, linux amd64/arm64, windows
- [ ] `install.sh`

## Open decisions (rename)

- [x] data directory: `~/.local/share/lancet` (`$XDG_DATA_HOME` honored), golden sets in `evals/` next to the database
- [ ] skill name: `qazyna-search` → `lancet-search`?
- [ ] MCP server name in the Claude config
- [x] environment variable prefix: `LANCET_`
- [ ] migrating existing data or reindexing from scratch (reindex is acceptable)
