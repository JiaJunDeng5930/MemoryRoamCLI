#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const BEGIN_MARKER: &str = "<!-- BEGIN AGENTS_MD_PROJECT_INDEX -->";
const END_MARKER: &str = "<!-- END AGENTS_MD_PROJECT_INDEX -->";
const EXCLUDE_DIRS: &[&str] = &[
    ".git",
    ".next",
    ".mypy_cache",
    ".pytest_cache",
    ".venv",
    ".workpad",
    "__pycache__",
    "build",
    "coverage",
    "dist",
    "node_modules",
    "target",
    "venv",
];
const EXCLUDE_FILES: &[&str] = &[
    ".env",
    ".env.*",
    "*.key",
    "*.p12",
    "*.pem",
    "*.pfx",
    "id_ed25519*",
    "id_rsa*",
];

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args().skip(1);
    let Some(command) = arguments.next() else {
        return Err(usage());
    };

    if command != "agents-md-index" {
        return Err(usage());
    }

    let action = arguments.next().unwrap_or_else(|| String::from("update"));
    let workspace_root = workspace_root()?;
    let agents_md_path = workspace_root.join("AGENTS.md");
    let rendered_block = render_index_block(&workspace_root)?;

    match action.as_str() {
        "print" => {
            print!("{rendered_block}");
            Ok(())
        }
        "update" => {
            let current = read_agents_md(&agents_md_path)?;
            let updated = upsert_index_block(&current, &rendered_block)?;

            if updated != current {
                fs::write(&agents_md_path, updated)
                    .map_err(|error| format!("failed to write AGENTS.md: {error}"))?;
                println!("Updated AGENTS.md project index.");
            } else {
                println!("AGENTS.md project index is already current.");
            }

            Ok(())
        }
        "check" => {
            let current = read_agents_md(&agents_md_path)?;
            let updated = upsert_index_block(&current, &rendered_block)?;

            if updated == current {
                println!("AGENTS.md project index is current.");
                Ok(())
            } else {
                Err(String::from(
                    "AGENTS.md project index is stale. Run `cargo xtask agents-md-index update`.",
                ))
            }
        }
        _ => Err(usage()),
    }
}

fn usage() -> String {
    String::from("usage: cargo xtask agents-md-index [update|check|print]")
}

fn workspace_root() -> Result<PathBuf, String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| String::from("failed to determine workspace root"))
}

fn read_agents_md(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("failed to read {}: {error}", path.display()))
}

fn render_index_block(workspace_root: &Path) -> Result<String, String> {
    let mut entries = BTreeMap::new();
    collect_directory_entries(workspace_root, ".", &mut entries)?;

    let mut block = String::new();
    writeln!(&mut block, "{BEGIN_MARKER}").expect("writing to string cannot fail");
    writeln!(&mut block, "```text").expect("writing to string cannot fail");
    writeln!(&mut block, "[Project Index]|root:.").expect("writing to string cannot fail");
    writeln!(
        &mut block,
        "|IMPORTANT: Prefer retrieval-led reasoning over pre-training-led reasoning for repository-specific behavior, structure, and APIs."
    )
    .expect("writing to string cannot fail");
    writeln!(&mut block, "|exclude_dirs:{{{}}}", EXCLUDE_DIRS.join(","))
        .expect("writing to string cannot fail");
    writeln!(&mut block, "|exclude_files:{{{}}}", EXCLUDE_FILES.join(","))
        .expect("writing to string cannot fail");

    for (path, children) in entries {
        writeln!(&mut block, "|{path}:{{{}}}", children.join(","))
            .expect("writing to string cannot fail");
    }

    writeln!(&mut block, "```").expect("writing to string cannot fail");
    writeln!(&mut block, "{END_MARKER}").expect("writing to string cannot fail");

    Ok(block)
}

fn collect_directory_entries(
    absolute_dir: &Path,
    relative_dir: &str,
    entries: &mut BTreeMap<String, Vec<String>>,
) -> Result<(), String> {
    let mut child_names = Vec::new();
    let read_dir = fs::read_dir(absolute_dir).map_err(|error| {
        format!(
            "failed to read directory {}: {error}",
            absolute_dir.display()
        )
    })?;

    let mut visible_entries = read_dir
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to enumerate {}: {error}", absolute_dir.display()))?;

    visible_entries.sort_by_key(|entry| entry.file_name());

    for entry in visible_entries {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry.path().display()))?;

        if file_type.is_dir() {
            if is_excluded_dir(&name) {
                continue;
            }

            child_names.push(format!("{name}/"));
            let child_relative_dir = join_relative_path(relative_dir, &name);
            collect_directory_entries(&entry.path(), &child_relative_dir, entries)?;
            continue;
        }

        if file_type.is_file() && !is_excluded_file(&name) {
            child_names.push(name.into_owned());
        }
    }

    entries.insert(String::from(relative_dir), child_names);
    Ok(())
}

fn join_relative_path(parent: &str, child: &str) -> String {
    if parent == "." {
        return child.to_owned();
    }

    format!("{parent}/{child}")
}

fn is_excluded_dir(name: &str) -> bool {
    EXCLUDE_DIRS.contains(&name)
}

fn is_excluded_file(name: &str) -> bool {
    EXCLUDE_FILES
        .iter()
        .any(|pattern| matches_file_pattern(name, pattern))
}

fn matches_file_pattern(name: &str, pattern: &str) -> bool {
    match pattern {
        ".env" => name == ".env",
        ".env.*" => name.starts_with(".env."),
        "*.key" => name.ends_with(".key"),
        "*.p12" => name.ends_with(".p12"),
        "*.pem" => name.ends_with(".pem"),
        "*.pfx" => name.ends_with(".pfx"),
        "id_ed25519*" => name.starts_with("id_ed25519"),
        "id_rsa*" => name.starts_with("id_rsa"),
        _ => name == pattern,
    }
}

fn upsert_index_block(current: &str, rendered_block: &str) -> Result<String, String> {
    let begin_matches = current.match_indices(BEGIN_MARKER).collect::<Vec<_>>();
    let end_matches = current.match_indices(END_MARKER).collect::<Vec<_>>();

    match (begin_matches.as_slice(), end_matches.as_slice()) {
        ([], []) => {
            let needs_separator = !current.is_empty() && !current.ends_with("\n\n");
            let separator = if needs_separator { "\n\n" } else { "" };
            Ok(format!("{current}{separator}{rendered_block}"))
        }
        ([begin_match], [end_match]) => {
            if begin_match.0 > end_match.0 {
                return Err(String::from("AGENTS.md index markers are out of order"));
            }

            let end_index = end_match.0 + END_MARKER.len();
            let prefix = &current[..begin_match.0];
            let suffix = current[end_index..].trim_start_matches('\n');

            Ok(format!("{prefix}{rendered_block}{suffix}"))
        }
        _ => Err(String::from(
            "AGENTS.md index markers must appear at most once each",
        )),
    }
}
