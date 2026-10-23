# Lancet

Local semantic search over your files: notes, docs, PDFs, spreadsheets.
Lancet indexes them into a [LanceDB](https://lancedb.com) database on your
machine and searches by meaning, by exact words, or both. The `lt` command
works in the terminal and serves the same search to AI agents (Claude Code)
over MCP. Nothing leaves your machine: embeddings are computed by a local
Ollama model.

## Install

macOS and Linux, arm64 and amd64:

```sh
curl -fsSL https://raw.githubusercontent.com/izhanov/lancet/main/install.sh | sh
```

The script downloads the latest `lt` from
[GitHub Releases](https://github.com/izhanov/lancet/releases) into
`~/.local/bin` (override with `LANCET_INSTALL_DIR`). Make sure that directory
is in your `PATH`. Run it again to update.

## Dependencies

| What | Needed for | Install |
|------|------------|---------|
| [Ollama](https://ollama.com/download) with the `bge-m3` model | Indexing, `vector` and `hybrid` search (the default): turns text into vectors, locally | `brew install ollama` or the installer from the site, then `ollama pull bge-m3` (~1.2 GB) |
| Poppler (`pdftotext`) | Indexing PDFs only; other formats do not need it | `brew install poppler` / `apt install poppler-utils` |
| [Claude Code](https://claude.com/claude-code) | Optional: search from AI agents (MCP server and skill) | — |
| [GitHub CLI](https://cli.github.com) (`gh`) | Optional: `install.sh` downloads through it when logged in, avoiding the anonymous API rate limit; falls back to `curl` | `brew install gh` |

Ollama must be running (`ollama serve`, or the desktop app) whenever you
index or search in `vector` / `hybrid` mode. `--mode text` works without it.

`bge-m3` is multilingual, so Russian and English notes live in one index. A
different model works too (`--embed-model`), but an index is bound to the
model that built it: after switching, run `lt reindex <path>`.

## Quick start

```sh
ollama pull bge-m3
lt index ~/notes                      # again later: only changed files are re-embedded
lt search "how do we deploy to preprod"
lt search "deploy -staging" --mode text --json
```

Supported formats: Markdown, PDF, XML, CSV/TSV, DOCX. Hidden directories
(`.git` and the like) are skipped. Files deleted from disk drop out of the
index on the next `lt index` of their directory.

`-word` and `-"a phrase"` in a query exclude results containing them;
`--exclude-path <fragment>` drops results by file path.

## Use from Claude Code

```sh
claude mcp add -s user lancet -- "$(command -v lt)" mcp
lt skill install
```

The first command registers the MCP server: agents get `search_notes`,
`list_files` and `index_status` (`mcp__lancet__*`) in every new session. The
server is read-only; indexing stays in the terminal. The second installs the
`lancet-search` skill into `~/.claude/skills`, which teaches agents to search
the index before reading files blindly. To make them reach for it on their
own, add to `~/.claude/CLAUDE.md`:

```
Before tasks involving my notes, docs or style guides, use the lancet-search skill.
```

## Where data lives

- database: `~/.local/share/lancet/db.lance` (`--db` / `LANCET_DB`)
- golden sets for `lt eval`: `~/.local/share/lancet/evals`
  (`--evals` / `LANCET_EVALS`)

`$XDG_DATA_HOME` is honored. Every flag has a `LANCET_*` environment twin;
see `lt --help`.

## Build from source

Needs Rust 1.91+ and `protoc` (Lance's build scripts compile protobuf
definitions): `brew install protobuf` / `apt install protobuf-compiler`.

```sh
cargo build --release -p lancet-cli   # target/release/lt
cargo test --workspace                # uses a fake embedder, no Ollama needed
```

## License

MIT
