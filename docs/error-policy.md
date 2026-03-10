# Error Handling Policy

This document defines the error-handling direction for the Error Refactor track.

## Core Rule: Native Rust Only

New and migrated code must use only standard Rust error patterns:

- `Result<T, E>` with typed, module-local `E`
- Manual implementations of:
  - `std::fmt::Display`
  - `std::error::Error`
  - `From` where conversion is useful

Do not introduce `anyhow` or `thiserror` in modules that are migrated to this policy.

## Boundary Mapping Rule

- Internal modules return typed errors from their own `errors` module.
- Top-level boundaries (HTTP handlers, webhook entry points, and `main`) perform explicit mapping from typed internal errors into boundary responses/logging/exit behavior.

No implicit catch-all conversion should be used at boundaries.

## Module Layout Scaffolding

Each major area has a dedicated error module scaffold to support the migration:

- `src/forgejo/errors.rs`
- `src/session/errors.rs`
- `src/ui/errors.rs`
- `src/webhook/errors.rs`
- `src/errors.rs` for top-level boundary mapping helpers

These scaffolds are intentionally non-behavioral and exist to make follow-up issues incremental.

## Transitional Note

Current runtime behavior remains unchanged in this step. Existing `anyhow` usage is migrated in follow-up issues in this series.
