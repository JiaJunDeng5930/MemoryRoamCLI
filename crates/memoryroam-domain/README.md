# memoryroam-domain

`memoryroam-domain` defines the stable types and repository contracts for the MemoryRoam note kernel.

## Scope

This crate owns:

- validated line-based text types
- node IDs and lookup keys
- parsed and canonicalized link syntax
- rendered read models
- repository traits shared by the read, write, and storage crates
- shared error types

This crate does not own command parsing, database I/O, or text output formatting for the CLI.

## Quick Start

```rust
use memoryroam_domain::{ContentLine, LookupKey, NodeId};

let node_id = NodeId::new(42)?;
let content = ContentLine::parse("Topic")?;
let key = LookupKey::from_content(&content)?;

assert_eq!(node_id.value(), 42);
assert_eq!(key.as_str(), "Topic");
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Main Entry Points

- `NodeId`: positive, externally visible note ID.
- `ContentLine`: validated single-line node content.
- `LookupKey`: trimmed lookup key used for content and aliases, excluding purely numeric and reserved-syntax values.
- `parse_content`: parser for `{{...}}` link syntax.
- `canonicalize_content`: rewrite lookup links into canonical ID-bound storage form.
- `render_storage_content`: render stored content for display-time output.
- `ReadRepository` / `WriteRepository`: contracts implemented by storage backends.

## Error Model

- User-input problems are returned as `KernelError::Input` or `KernelError::Constraint`.
- Missing entities use `KernelError::NotFound`.
- Lookup resolution failures use `KernelError::LookupMiss` or `KernelError::LookupAmbiguous`.
- Invalid stored data surfaces as `KernelError::StorageCorruption`.

## Notes

- Lookup normalization trims surrounding whitespace and rejects purely numeric or reserved-syntax values.
- Rendering rejects unresolved lookup tokens in stored content.
- Rendering expands unlabeled links and escapes nested previews so the output remains valid MemoryRoam text.
