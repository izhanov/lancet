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
| [Ollama](https://ollama.com/download) with an embedding model, `nomic-embed-text` by default | Indexing, `vector` and `hybrid` search (the default): turns text into vectors, locally | `brew install ollama` or the installer from the site, then `ollama pull nomic-embed-text` (~274 MB) |
| Poppler (`pdftotext`) | Indexing PDFs only; other formats do not need it | `brew install poppler` / `apt install poppler-utils` |
| [Claude Code](https://claude.com/claude-code) | Optional: search from AI agents (MCP server and skill) | — |
| [GitHub CLI](https://cli.github.com) (`gh`) | Optional: `install.sh` downloads through it when logged in, avoiding the anonymous API rate limit; falls back to `curl` | `brew install gh` |

Ollama must be running (`ollama serve`, or the desktop app) whenever you
index or search in `vector` / `hybrid` mode. `--mode text` works without it.

`nomic-embed-text` is English. For notes in other languages build the
database with the multilingual `bge-m3` (~1.2 GB): `ollama pull bge-m3`, then
`lt --embed-model bge-m3 index ~/notes` (or `LANCET_EMBED_MODEL=bge-m3`). A
database remembers its model, so later commands need no flag; `--embed-model`
on a database built with another model is refused, and `lt reindex <path>`
rebuilds it with the new one. On English code and notes the light models
searched about as well as `bge-m3` (`docs/parity.md`, step 5);
`snowflake-arctic-embed:33m` (67 MB) is the smallest that kept up.

Many retrieval models expect a prompt before a query and before a passage
(`search_query: ` / `search_document: ` for `nomic-embed-text`); `lt` adds
them for the models listed in `lib/src/embed/models.rs` and embeds any other
model's texts as they are, with a warning.

## Quick start

```sh
ollama pull nomic-embed-text
lt index ~/notes                      # again later: only changed files are re-embedded
lt search "how do we deploy to preprod"
lt search "deploy -staging" --mode text --json
```

Supported formats: Markdown, PDF, XML, CSV/TSV, DOCX, plus source code in
any language you add (see below). Hidden directories (`.git` and the like)
are skipped; inside a git repository `.gitignore` is honored, and an
`.ignore` file excludes paths anywhere. Files deleted from disk drop out of
the index on the next `lt index` of their directory.

`-word` and `-"a phrase"` in a query exclude results containing them;
`--exclude-path <fragment>` drops results by file path.

## Source code

Code is chunked by symbol: a method, class or module that fits is one
chunk, a bigger one splits into its nested definitions, and each chunk's
section is the trail to it (`module Billing > class Invoice > method total`).
Lancet knows no languages itself: a language is a plugin, installed
straight from the tree-sitter grammar's GitHub repository.

```sh
lt lang add ruby --ext .rake --ext .gemspec   # tree-sitter/tree-sitter-ruby, latest release
lt lang add python
lt lang add someone/tree-sitter-elixir@v0.3.0  # any repository, pinned tag
lt lang add tree-sitter-grammars/tree-sitter-yaml
lt lang add embedded-template                  # ERB (.erb, also .ejs and .etlua)
lt lang list
lt index ~/code/my-app
```

A grammar without a tags query has no symbols to chunk by, so its files
follow the syntax tree alone. YAML comes out well: a block too big for one
chunk splits by key, and the keys are the trail (`en > account > profiles`).
ERB templates are packed in source order without a trail, since
embedded-template sees `<% %>` tags but not the HTML around them.

### Indexing a repository

Keep code in a database of its own (`--db` / `LANCET_DB`): tens of thousands
of code chunks would crowd your notes out of search results, and a code
index may want a different embedding model.

```sh
lt lang add ruby --ext .rake --ext .gemspec
cd ~/code/my-app
lt --db ~/.local/share/lancet/my-app.lance index app lib
lt --db ~/.local/share/lancet/my-app.lance search "where is the invoice total computed"
```

Index the directories you ask questions about, not the whole repository:

- **Skip what nobody searches by meaning.** Database migrations, generated
  code, fixtures and vendored libraries are noise in the results and cost
  embedding time. In a Rails app, `app` and `lib` are about half the Ruby
  files; `db/migrate` and `spec` are the other half.
- **Locales in one language.** A Rails app's `config/locales` holds every
  string once per language: in one app English was 4% of 5.8 MB. Pass the
  English files as roots (`$(find config/locales -name 'en.yml' -o -name
  '*.en.yml')`) rather than the directory.
- **Tests are a judgment call.** Specs answer "how is this used", but they
  repeat the code they test and dilute the results; add them later as a
  separate root if you miss them.
- **What git ignores is skipped already** (`vendor/bundle`, `node_modules`,
  build output). To leave out more without listing roots, put an `.ignore`
  file (gitignore syntax) in the directory you index, e.g. `db/migrate/` or
  one-off data scripts like `lib/tasks/one_time/`.
- **The first run is the slow one**: every chunk goes through the embedding
  model, roughly 40–50 chunks per second with `bge-m3` on an Apple M5, so a
  large app takes minutes. After that `lt index` re-embeds only files whose
  mtime changed — a `git pull` costs seconds.
- **Changing roots? Use `reindex`.** `lt index` drops vanished files only
  under the roots it was given, so files indexed earlier from another root
  stay; `lt reindex <roots>` starts the database over.

To let agents search the code, register a second MCP server pointed at the
code database next to the one for notes:

```sh
claude mcp add -s user lancet-my-app -- "$(command -v lt)" --db ~/.local/share/lancet/my-app.lance mcp
```

### Language plugins

A plugin is a directory in `~/.local/share/lancet/languages`
(`--languages` / `LANCET_LANGUAGES`) with three files: `language.toml`
(the grammar's name and file extensions), `grammar.wasm` (the grammar
compiled to WebAssembly, from the grammar's release) and `tags.scm` (the
grammar's own `queries/tags.scm`, which marks definitions). For a grammar
that publishes no `.wasm`, build one with `tree-sitter build --wasm` and
put the three files there by hand.

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
- language plugins: `~/.local/share/lancet/languages`
  (`--languages` / `LANCET_LANGUAGES`)

`$XDG_DATA_HOME` is honored. Every flag has a `LANCET_*` environment twin;
see `lt --help`.

### How a database is stored

A database is a [LanceDB](https://lancedb.com) directory: no server, `lt`
reads and writes the files itself. Every write is a commit that creates a
new table version, so an interrupted `lt index` leaves the previous version
intact, and an `lt mcp` running elsewhere sees new writes without locks.

The `chunks` table holds one row per chunk:

| column    | type                      | |
|-----------|---------------------------|---|
| `id`      | string                    | `<path>#<ordinal>`, stable across re-indexing |
| `path`    | string                    | absolute path of the file |
| `section` | string, nullable          | heading or symbol trail: `Install > macOS`, `class Invoice > method total` |
| `text`    | string                    | the chunk as written in the file |
| `vector`  | `FixedSizeList<Float32, N>` | the embedding, unit length |
| `mtime`   | int64                     | the file's mtime, for incremental indexing |

The path tail and the section are woven into the text the model embeds,
not into `text`. Full-text search runs on a BM25 index over `text`; vector
search is an exact scan (no approximate index), ~0.13 s on 25k chunks.
A `meta` table keeps key/value pairs, today the embedder.

**A database belongs to one embedding model.** The vector width `N` is
fixed when the table is created (768 for `nomic-embed-text`, 1024 for
`bge-m3`), and the model's id is stored in `meta`: vectors of different
models are incomparable even when their widths match, so `lt` refuses to
index into or search a database with another model, and without
`--embed-model` it uses the stored one. To switch models,
`lt --embed-model <model> reindex <path>` — it re-embeds everything. Everything else in a row is plain data.

Inspect a database outside `lt` with any Arrow tool, e.g. in Python:
`lancedb.connect("~/.local/share/lancet/db.lance").open_table("chunks").to_pandas()`.

## Build from source

Needs Rust 1.95+ and `protoc` (Lance's build scripts compile protobuf
definitions): `brew install protobuf` / `apt install protobuf-compiler`.

```sh
cargo build --release -p lancet-cli   # target/release/lt
cargo test --workspace                # uses a fake embedder, no Ollama needed
```

## License

MIT
