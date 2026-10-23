# Parity with qazyna (Go)

Everything qazyna does, Lancet must do. Source of truth:
[izhanov/qazyna@35da608](https://github.com/izhanov/qazyna/tree/35da608) (the
local clone and the installed qazyna are gone). Tick a box when the feature is
ported **and** covered by tests.

The command, flags, modes, `--json` output and MCP tool names keep the same
contract. Search internals are allowed to change (native FTS instead of an
`ILIKE` scan, prefilter instead of post-filtering) as long as `eval` does not
get worse.

## Status — 2026-10-22

Released [v0.1.1](https://github.com/izhanov/lancet/releases/tag/v0.1.1):
core library, all parsers, incremental indexing and `lt index / reindex /
search / eval`; install with `install.sh`. qazyna is removed from the machine,
so **agents have no notes search until `lt mcp` lands**.

Eval on qazyna's corpus (113 files) and golden set (5 cases), recall@5 / MRR:

| mode   | qazyna      | lancet      |
|--------|-------------|-------------|
| vector | 0.80 / 0.600 | 0.80 / 0.600 |
| text   | 0.00 / 0.000 | 0.40 / 0.167 |
| hybrid | 0.80 / 0.417 | 0.80 / 0.567 |

Next, in order:

1. Release `lt mcp` and `lt skill` (done in code, not released yet), then
   `claude mcp add -s user lancet -- lt mcp` and `lt skill install` — gives
   agents search back.
2. Grow the golden set (5 cases is too few to decide anything), then tune FTS
   for mixed Russian/English: stemming, stop words, n-grams. Hybrid must beat
   plain vector; today it does not, mostly on Russian queries.
3. The rest of the CLI: `list`, `flush`, `evals`, `tenants` / `--tenant`,
   `setup`, `update`.
4. `lt-server` and tenant databases on S3.

Backlog, not decided: ignoring paths while indexing (e.g. `.claude/worktrees`,
`.gitignore`); smaller binaries (LTO; `lt` is ~200 MB, ~75 MB packed).

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
