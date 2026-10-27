---
name: lancet-search
description: Use Lancet semantic search (MCP) as the first research step for any task. Search the local indexes for relevant notes, docs, and code before reading or grepping blindly. Works in any repo.
---

# Lancet search in your work loop

The user runs **Lancet** (`lt`) — a local semantic search over their files
(notes, docs, style guides, source code). Each index is a separate MCP server
with the same three tools:

- `search_notes` — hybrid vector + full-text search
- `list_files` — what is indexed, with mtimes
- `index_status` — index health (chunk count, embed model)

The server's name and its instructions (which name its database) tell the
indexes apart:

- `lancet` (`mcp__lancet__*`) — the user's notes and documents
- `lancet-<repo>` (e.g. `mcp__lancet-my-app__*`) — the code of one repository

These are deferred tools: find the servers with `ToolSearch "lancet"`, then
load what you need, e.g.
`ToolSearch "select:mcp__lancet__search_notes,mcp__lancet-my-app__search_notes"`.

## The loop

For any task that might touch knowledge the user has indexed (their notes,
style guides, project docs, reference material, the code of a repository):

1. **Pick the index.** Notes and docs → `lancet`. A question about how or
   where something works in a repository → that repository's `lancet-<repo>`
   server, if there is one. When unsure, search both.

2. **Search before reading.** Query the index with the task's key concepts.
   One or two searches cost almost nothing and often surface the exact file
   and section you need.

   For code, phrase the query the way the code would: in English, in the
   domain's own terms. "как пользователи меняют о себе инфу" misses what
   "user edits own profile name avatar password" finds first. When the
   user's words differ from the code's, send both queries.

3. **Pick the mode:**
   - default (hybrid) — best for most queries
   - `vector` — conceptual questions ("how does billing work")
   - `text` — exact identifiers, names, error strings

   For "not about X" intents, never put the negation in the query text —
   embeddings ignore "not" and mentioning X attracts it. Use the `exclude`
   parameter (words/phrases to drop) or `exclude_path` (path fragments)
   instead.

4. **Map results to files.** Each hit returns a file path + Section trail.
   Path = what to read, Section = where in the file. The trail is a heading
   breadcrumb in documents ("Install > macOS"), the enclosing definitions in
   code ("module Billing > class Invoice > method total"), the keys in YAML
   ("en > account > profiles"). Read only what search surfaced.

5. **Verify freshness.** The index reflects files at index time. If a result
   names a function, flag, or fact — verify against the live file before
   relying on it. `list_files` shows mtimes if in doubt.

6. **Suggest reindexing.** If you created or edited files worth searching
   later, remind the user to reindex in the terminal — MCP is read-only by
   design. Notes: `lt index <path>`. Code goes to its own database, never
   the notes one: `lt --db <repo database> index <roots>`, with the database
   named in that repository's server instructions.

## When to reach for it

- The user mentions "my notes", "the style guide", "that document I have"
- Writing/editing content that must follow an indexed style guide
- A question about material that isn't in the current repo
- Before answering "where did I write about X?"
- A question about how a feature works, or where it lives, in an indexed
  repository

## When NOT to use it

- Exact symbols, strings, or error messages in the current repo — Grep/Glob
  are faster and complete
- The user gave you the exact file path already
- Nothing relevant could plausibly be indexed (check `list_files` if unsure)

## Repo-specific knowledge

If the current repo has its own `.claude/skills/` or `CLAUDE.md` with a
task-to-file map, prefer that map and use this skill to find what the map
does not cover.
