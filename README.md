# Unity LS

A Language Server Protocol (LSP) implementation focused on Unity-specific features for C# scripts.

## Features

- **Asset References**: Shows where a script component is referenced in scenes (`.unity`), prefabs (`.prefab`), and assets (`.asset`) files using CodeLens.

## Architecture

- `src/main.rs` bootstraps logging and starts the server runtime.
- `src/server.rs` owns LSP transport loop, initialization, request/notification dispatch, and response writing.
- `src/document_storage.rs` stores open document text snapshots by URI.
- `src/capabilities/codelens.rs` implements Unity-specific CodeLens:
  - parse class line from C# file (tree-sitter query)
  - read script GUID from `.cs.meta`
  - scan `Assets/` for GUID references
  - resolve final `showUnityReferences` command arguments

## Local development

- Build: `cargo build`
- Check: `cargo check`
- Test: `cargo test`
- Single test: `cargo test <name>`
