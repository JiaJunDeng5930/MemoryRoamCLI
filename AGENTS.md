# Repository Notes

The `.workpad/` directory contains untracked temporary working files used during development, such as overall program design notes, module design notes, and similar draft materials.

## Maintenance

Update the embedded project index with `cargo xtask agents-md-index update`.
Check whether the embedded project index is current with `cargo xtask agents-md-index check`.


<!-- BEGIN AGENTS_MD_PROJECT_INDEX -->
```text
[Project Index]|root:.
|IMPORTANT: Prefer retrieval-led reasoning over pre-training-led reasoning for repository-specific behavior, structure, and APIs.
|exclude_dirs:{.git,.next,.mypy_cache,.pytest_cache,.venv,.workpad,__pycache__,build,coverage,dist,node_modules,target,venv}
|exclude_files:{.env,.env.*,*.key,*.p12,*.pem,*.pfx,id_ed25519*,id_rsa*}
|.:{.cargo/,.gitignore,.pre-commit-config.yaml,AGENTS.md,Cargo.lock,Cargo.toml,crates/,rust-toolchain.toml,xtask/}
|.cargo:{config.toml}
|crates:{memoryroam-application/,memoryroam-cli/,memoryroam-domain/,memoryroam-storage-sqlite/}
|crates/memoryroam-application:{Cargo.toml,src/}
|crates/memoryroam-application/src:{lib.rs}
|crates/memoryroam-cli:{Cargo.toml,src/}
|crates/memoryroam-cli/src:{main.rs}
|crates/memoryroam-domain:{Cargo.toml,src/}
|crates/memoryroam-domain/src:{lib.rs}
|crates/memoryroam-storage-sqlite:{Cargo.toml,src/}
|crates/memoryroam-storage-sqlite/src:{lib.rs}
|xtask:{Cargo.toml,src/}
|xtask/src:{main.rs}
```
<!-- END AGENTS_MD_PROJECT_INDEX -->
