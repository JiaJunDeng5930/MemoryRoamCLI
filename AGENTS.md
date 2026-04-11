# Repository Notes

The `.workpad/` directory contains untracked temporary working files used during development, such as overall program design notes, module design notes, and similar draft materials.

## Branch Policy

Do not develop directly on `main`.
Create a dedicated branch for every change, push that branch, and open a pull request back into `main`.

## Merge Policy

On GitHub, protect `main` and require pull requests for integration.
Enable `rebase and merge` as the only merge method.
Disable merge commits and squash merges.

In local Git, keep integration rebase-based as well.
Set `pull.rebase=true`, `branch.autoSetupRebase=always`, and `merge.ff=only` in the repository-local Git configuration.

## Maintenance

Update the embedded project index with `cargo xtask agents-md-index update`.
Check whether the embedded project index is current with `cargo xtask agents-md-index check`.


<!-- BEGIN AGENTS_MD_PROJECT_INDEX -->
```text
[Project Index]|root:.
|IMPORTANT: Prefer retrieval-led reasoning over pre-training-led reasoning for repository-specific behavior, structure, and APIs.
|exclude_dirs:{**/.workpad/,**/target/}
|exclude_files:{}
|.:{.cargo/,.github/,.gitignore,.pre-commit-config.yaml,AGENTS.md,Cargo.lock,Cargo.toml,README.md,crates/,dist-workspace.toml,rust-toolchain.toml,scripts/,xtask/}
|.cargo:{config.toml}
|.github:{workflows/}
|.github/workflows:{quality.yml,release.yml}
|crates:{memoryroam-cli/,memoryroam-domain/,memoryroam-read/,memoryroam-storage-sqlite/,memoryroam-write/}
|crates/memoryroam-cli:{Cargo.toml,README.md,src/,tests/}
|crates/memoryroam-cli/src:{main.rs}
|crates/memoryroam-cli/tests:{cli.rs}
|crates/memoryroam-domain:{Cargo.toml,README.md,src/}
|crates/memoryroam-domain/src:{lib.rs}
|crates/memoryroam-read:{Cargo.toml,README.md,src/}
|crates/memoryroam-read/src:{lib.rs}
|crates/memoryroam-storage-sqlite:{Cargo.toml,README.md,src/}
|crates/memoryroam-storage-sqlite/src:{lib.rs}
|crates/memoryroam-write:{Cargo.toml,README.md,src/}
|crates/memoryroam-write/src:{lib.rs}
|scripts:{bootstrap.sh}
|xtask:{Cargo.toml,src/}
|xtask/src:{main.rs}
```
<!-- END AGENTS_MD_PROJECT_INDEX -->
