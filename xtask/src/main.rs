#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

const BEGIN_MARKER: &str = "<!-- BEGIN AGENTS_MD_PROJECT_INDEX -->";
const END_MARKER: &str = "<!-- END AGENTS_MD_PROJECT_INDEX -->";

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
    let index_state = collect_index_state(workspace_root)?;

    let mut block = String::new();
    writeln!(&mut block, "{BEGIN_MARKER}").expect("writing to string cannot fail");
    writeln!(&mut block, "```text").expect("writing to string cannot fail");
    writeln!(&mut block, "[Project Index]|root:.").expect("writing to string cannot fail");
    writeln!(
        &mut block,
        "|IMPORTANT: Prefer retrieval-led reasoning over pre-training-led reasoning for repository-specific behavior, structure, and APIs."
    )
    .expect("writing to string cannot fail");
    writeln!(
        &mut block,
        "|exclude_dirs:{{{}}}",
        index_state.exclude_dirs.join(",")
    )
    .expect("writing to string cannot fail");
    writeln!(
        &mut block,
        "|exclude_files:{{{}}}",
        index_state.exclude_files.join(",")
    )
    .expect("writing to string cannot fail");

    for (path, children) in index_state.entries {
        writeln!(&mut block, "|{path}:{{{}}}", children.join(","))
            .expect("writing to string cannot fail");
    }

    writeln!(&mut block, "```").expect("writing to string cannot fail");
    writeln!(&mut block, "{END_MARKER}").expect("writing to string cannot fail");

    Ok(block)
}

fn collect_index_state(workspace_root: &Path) -> Result<IndexState, String> {
    let walked_paths = walk_working_tree(workspace_root, workspace_root, ".")?;
    let ignore_patterns =
        collect_repo_ignore_patterns(workspace_root, &walked_paths.gitignore_files)?;

    Ok(IndexState {
        entries: build_directory_entries(&walked_paths.visible_files),
        exclude_dirs: ignore_patterns.exclude_dirs,
        exclude_files: ignore_patterns.exclude_files,
    })
}

fn walk_working_tree(
    workspace_root: &Path,
    absolute_dir: &Path,
    relative_dir: &str,
) -> Result<WalkedPaths, String> {
    let read_dir = fs::read_dir(absolute_dir).map_err(|error| {
        format!(
            "failed to read directory {}: {error}",
            absolute_dir.display()
        )
    })?;
    let mut children = read_dir
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to enumerate {}: {error}", absolute_dir.display()))?;
    children.sort_by_key(|entry| entry.file_name());

    let mut probes = Vec::new();
    for child in children {
        let name = child.file_name().to_string_lossy().into_owned();
        if relative_dir == "." && name == ".git" {
            continue;
        }

        let file_type = child
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", child.path().display()))?;
        let relative_path = join_relative_path(relative_dir, &name);

        if file_type.is_dir() {
            probes.push(PathProbe::directory(relative_path, child.path()));
            continue;
        }

        if file_type.is_file() {
            probes.push(PathProbe::file(relative_path));
        }
    }

    let ignored_matches = git_ignore_matches(workspace_root, &probes)?;
    let mut visible_files = Vec::new();
    let mut gitignore_files = Vec::new();

    for probe in probes {
        if probe.is_dir {
            if ignored_matches.contains(probe.git_path())
                && !has_visible_descendants(workspace_root, probe.relative_path.as_str())?
            {
                continue;
            }

            let child_paths = walk_working_tree(
                workspace_root,
                probe
                    .absolute_path
                    .as_deref()
                    .expect("directories should have an absolute path"),
                probe.relative_path.as_str(),
            )?;
            visible_files.extend(child_paths.visible_files);
            gitignore_files.extend(child_paths.gitignore_files);
            continue;
        }

        if ignored_matches.contains(probe.git_path()) {
            continue;
        }

        if Path::new(probe.relative_path.as_str())
            .file_name()
            .is_some_and(|name| name == ".gitignore")
        {
            gitignore_files.push(probe.relative_path.clone());
        }

        visible_files.push(probe.relative_path);
    }

    Ok(WalkedPaths {
        visible_files,
        gitignore_files,
    })
}

fn git_ignore_matches(
    workspace_root: &Path,
    probes: &[PathProbe],
) -> Result<BTreeSet<String>, String> {
    if probes.is_empty() {
        return Ok(BTreeSet::new());
    }

    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(workspace_root)
        .args(["check-ignore", "--stdin", "-z", "-v"]);
    clear_git_environment(&mut command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|error| {
        format!(
            "failed to run `git check-ignore --stdin -z -v` in {}: {error}",
            workspace_root.display()
        )
    })?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            format!(
                "failed to open stdin for `git check-ignore --stdin -z -v` in {}",
                workspace_root.display()
            )
        })?;
        for probe in probes {
            stdin
                .write_all(probe.git_path().as_bytes())
                .and_then(|()| stdin.write_all(b"\0"))
                .map_err(|error| {
                    format!(
                        "failed to write stdin for `git check-ignore --stdin -z -v` in {}: {error}",
                        workspace_root.display()
                    )
                })?;
        }
    }
    let output = child.wait_with_output().map_err(|error| {
        format!(
            "failed to wait for `git check-ignore --stdin -z -v` in {}: {error}",
            workspace_root.display()
        )
    })?;

    if !output.status.success() && output.status.code() != Some(1) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let detail = if stderr.is_empty() {
            String::from("no stderr output")
        } else {
            stderr.to_owned()
        };
        return Err(format!(
            "git command failed: `git check-ignore --stdin -z -v` in {}: {detail}",
            workspace_root.display()
        ));
    }

    parse_ignored_paths(&output.stdout)
}

fn parse_ignored_paths(output: &[u8]) -> Result<BTreeSet<String>, String> {
    let fields = output
        .split(|byte| *byte == b'\0')
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8(field.to_vec()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("git output was not valid UTF-8: {error}"))?;

    if fields.len() % 4 != 0 {
        return Err(String::from(
            "git check-ignore output had an unexpected number of fields",
        ));
    }

    let mut matches = BTreeSet::new();
    for chunk in fields.chunks_exact(4) {
        if chunk[2].starts_with('!') {
            continue;
        }
        matches.insert(chunk[3].clone());
    }

    Ok(matches)
}

fn clear_git_environment(command: &mut Command) {
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
    ] {
        command.env_remove(variable);
    }
}

fn has_visible_descendants(workspace_root: &Path, relative_dir: &str) -> Result<bool, String> {
    let visible_paths = git_path_list(
        workspace_root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "--full-name",
            "-z",
            "--",
            relative_dir,
        ],
        false,
    )?;

    Ok(visible_paths
        .into_iter()
        .any(|path| workspace_root.join(path).exists()))
}

fn collect_repo_ignore_patterns(
    workspace_root: &Path,
    gitignore_paths: &[String],
) -> Result<IgnorePatterns, String> {
    let mut exclude_dirs = BTreeSet::new();
    let mut exclude_files = BTreeSet::new();

    for gitignore_path in gitignore_paths {
        let absolute_path = workspace_root.join(gitignore_path);
        let content = fs::read_to_string(&absolute_path)
            .map_err(|error| format!("failed to read {}: {error}", absolute_path.display()))?;
        let parent = Path::new(gitignore_path).parent().unwrap_or(Path::new(""));

        for line in content.lines() {
            let Some(pattern) = normalize_ignore_pattern(parent, line) else {
                continue;
            };

            if pattern.ends_with('/') {
                exclude_dirs.insert(pattern);
            } else {
                exclude_files.insert(pattern);
            }
        }
    }

    Ok(IgnorePatterns {
        exclude_dirs: exclude_dirs.into_iter().collect(),
        exclude_files: exclude_files.into_iter().collect(),
    })
}

fn git_path_list(
    workspace_root: &Path,
    arguments: &[&str],
    allow_not_found: bool,
) -> Result<Vec<String>, String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(workspace_root).args(arguments);
    clear_git_environment(&mut command);

    let output = command.output().map_err(|error| {
        format!(
            "failed to run `git {}` in {}: {error}",
            arguments.join(" "),
            workspace_root.display()
        )
    })?;

    if !output.status.success() && (!allow_not_found || output.status.code() != Some(1)) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let detail = if stderr.is_empty() {
            String::from("no stderr output")
        } else {
            stderr.to_owned()
        };
        return Err(format!(
            "git command failed: `git {}` in {}: {detail}",
            arguments.join(" "),
            workspace_root.display()
        ));
    }

    output
        .stdout
        .split(|byte| *byte == b'\0')
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8(path.to_vec()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("git output was not valid UTF-8: {error}"))
}

fn normalize_ignore_pattern(parent: &Path, line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('!') {
        return None;
    }

    let unescaped = if let Some(stripped) = trimmed.strip_prefix("\\#") {
        stripped
    } else if let Some(stripped) = trimmed.strip_prefix("\\!") {
        stripped
    } else if trimmed.starts_with('#') {
        return None;
    } else {
        trimmed
    };

    let normalized = unescaped.strip_prefix('/').unwrap_or(unescaped);
    if normalized.is_empty() {
        return None;
    }

    let parent_prefix = parent.to_string_lossy();
    let has_explicit_path = normalized.trim_end_matches('/').contains('/');
    let anchored = unescaped.starts_with('/');
    if parent_prefix.is_empty() {
        if anchored || has_explicit_path {
            return Some(normalized.to_owned());
        }

        return Some(format!("**/{normalized}"));
    }

    if anchored || has_explicit_path {
        return Some(format!("{parent_prefix}/{normalized}"));
    }

    Some(format!("{parent_prefix}/**/{normalized}"))
}

fn build_directory_entries(visible_files: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut entries = BTreeMap::<String, BTreeSet<String>>::new();
    entries.entry(String::from(".")).or_default();

    for path in visible_files {
        let components = path.split('/').collect::<Vec<_>>();
        if components.is_empty() {
            continue;
        }

        let Some((file_name, parent_directories)) = components.split_last() else {
            continue;
        };

        let mut parent = String::from(".");
        for directory in parent_directories {
            entries
                .entry(parent.clone())
                .or_default()
                .insert(format!("{directory}/"));
            parent = join_relative_path(&parent, directory);
            entries.entry(parent.clone()).or_default();
        }

        entries
            .entry(parent)
            .or_default()
            .insert((*file_name).to_owned());
    }

    entries
        .into_iter()
        .map(|(path, children)| (path, children.into_iter().collect()))
        .collect()
}

fn join_relative_path(parent: &str, child: &str) -> String {
    if parent == "." {
        return child.to_owned();
    }

    format!("{parent}/{child}")
}

struct IndexState {
    entries: BTreeMap<String, Vec<String>>,
    exclude_dirs: Vec<String>,
    exclude_files: Vec<String>,
}

struct IgnorePatterns {
    exclude_dirs: Vec<String>,
    exclude_files: Vec<String>,
}

struct WalkedPaths {
    visible_files: Vec<String>,
    gitignore_files: Vec<String>,
}

struct PathProbe {
    relative_path: String,
    git_path: String,
    absolute_path: Option<PathBuf>,
    is_dir: bool,
}

impl PathProbe {
    fn directory(relative_path: String, absolute_path: PathBuf) -> Self {
        Self {
            git_path: format!("{relative_path}/"),
            relative_path,
            absolute_path: Some(absolute_path),
            is_dir: true,
        }
    }

    fn file(relative_path: String) -> Self {
        Self {
            git_path: relative_path.clone(),
            relative_path,
            absolute_path: None,
            is_dir: false,
        }
    }

    fn git_path(&self) -> &str {
        self.git_path.as_str()
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

#[cfg(test)]
mod tests {
    use super::render_index_block;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_TEST_WORKSPACE_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn render_index_block_uses_git_ignored_paths_for_exclude_sections() {
        let temp_dir = TestWorkspace::new();
        temp_dir.init_git_repository();
        temp_dir.write_file(".gitignore", "ignored-dir/\nignored-file.txt\n");
        temp_dir.git(["add", ".gitignore"]);
        temp_dir.write_file("visible.txt", "visible\n");
        temp_dir.write_file("ignored-dir/nested.txt", "ignored\n");
        temp_dir.write_file("ignored-file.txt", "ignored\n");
        temp_dir.write_file(".git/info/exclude", "local-only.txt\n");
        temp_dir.write_file("local-only.txt", "local\n");

        let block = render_index_block(temp_dir.path()).expect("render should succeed");
        let lines = IndexBlockLines::parse(&block);

        assert_eq!(lines.value("exclude_dirs"), "**/ignored-dir/");
        assert_eq!(lines.value("exclude_files"), "**/ignored-file.txt");
        assert_eq!(lines.value("."), ".gitignore,visible.txt");
        assert!(!lines.value(".").contains("ignored-dir/"));
        assert!(!lines.value(".").contains("ignored-file.txt"));
        assert!(!lines.value(".").contains("local-only.txt"));
        assert!(!block.contains("|ignored-dir:{nested.txt}"));
    }

    #[test]
    fn render_index_block_keeps_tracked_files_visible_even_after_ignore_rule_is_added() {
        let temp_dir = TestWorkspace::new();
        temp_dir.init_git_repository();
        temp_dir.write_file("tracked.txt", "tracked\n");
        temp_dir.git(["add", "tracked.txt"]);
        temp_dir.write_file(".gitignore", "tracked.txt\n");
        temp_dir.git(["add", ".gitignore"]);

        let block = render_index_block(temp_dir.path()).expect("render should succeed");
        let lines = IndexBlockLines::parse(&block);

        assert_eq!(lines.value("exclude_dirs"), "");
        assert_eq!(lines.value("exclude_files"), "**/tracked.txt");
        assert_eq!(lines.value("."), ".gitignore,tracked.txt");
    }

    #[test]
    fn render_index_block_drops_deleted_tracked_files_before_they_are_staged() {
        let temp_dir = TestWorkspace::new();
        temp_dir.init_git_repository();
        temp_dir.write_file("tracked.txt", "tracked\n");
        temp_dir.git(["add", "tracked.txt"]);
        temp_dir.git(["commit", "-m", "add tracked file"]);
        temp_dir.remove_file("tracked.txt");

        let block = render_index_block(temp_dir.path()).expect("render should succeed");
        let lines = IndexBlockLines::parse(&block);

        assert_eq!(lines.value("exclude_dirs"), "");
        assert_eq!(lines.value("exclude_files"), "");
        assert_eq!(lines.value("."), "");
        assert!(!block.contains("tracked.txt"));
    }

    #[test]
    fn render_index_block_keeps_repo_ignore_patterns_without_matching_paths() {
        let temp_dir = TestWorkspace::new();
        temp_dir.init_git_repository();
        temp_dir.write_file(".gitignore", "ignored-dir/\nignored-file.txt\n");
        temp_dir.git(["add", ".gitignore"]);

        let block = render_index_block(temp_dir.path()).expect("render should succeed");
        let lines = IndexBlockLines::parse(&block);

        assert_eq!(lines.value("exclude_dirs"), "**/ignored-dir/");
        assert_eq!(lines.value("exclude_files"), "**/ignored-file.txt");
        assert_eq!(lines.value("."), ".gitignore");
    }

    #[test]
    fn render_index_block_preserves_nested_gitignore_scope_for_plain_names() {
        let temp_dir = TestWorkspace::new();
        temp_dir.init_git_repository();
        temp_dir.write_file("nested/.gitignore", "foo\n");

        let block = render_index_block(temp_dir.path()).expect("render should succeed");
        let lines = IndexBlockLines::parse(&block);

        assert_eq!(lines.value("exclude_dirs"), "");
        assert_eq!(lines.value("exclude_files"), "nested/**/foo");
    }

    #[test]
    fn render_index_block_preserves_root_gitignore_scope_for_plain_names() {
        let temp_dir = TestWorkspace::new();
        temp_dir.init_git_repository();
        temp_dir.write_file(".gitignore", "target/\nfoo\n");
        temp_dir.git(["add", ".gitignore"]);

        let block = render_index_block(temp_dir.path()).expect("render should succeed");
        let lines = IndexBlockLines::parse(&block);

        assert_eq!(lines.value("exclude_dirs"), "**/target/");
        assert_eq!(lines.value("exclude_files"), "**/foo");
    }

    #[test]
    fn render_index_block_keeps_files_reincluded_by_negated_ignore_rules() {
        let temp_dir = TestWorkspace::new();
        temp_dir.init_git_repository();
        temp_dir.write_file(".gitignore", "*.txt\n!keep.txt\n");
        temp_dir.git(["add", ".gitignore"]);
        temp_dir.write_file("keep.txt", "visible\n");
        temp_dir.write_file("drop.txt", "ignored\n");

        let block = render_index_block(temp_dir.path()).expect("render should succeed");
        let lines = IndexBlockLines::parse(&block);

        assert_eq!(lines.value("exclude_files"), "**/*.txt");
        assert_eq!(lines.value("."), ".gitignore,keep.txt");
        assert!(!block.contains("drop.txt"));
    }

    #[test]
    fn render_index_block_keeps_visible_untracked_files_inside_ignored_directories() {
        let temp_dir = TestWorkspace::new();
        temp_dir.init_git_repository();
        temp_dir.write_file(".gitignore", "dist/*\n!dist/keep.txt\n");
        temp_dir.git(["add", ".gitignore"]);
        temp_dir.write_file("dist/keep.txt", "visible\n");
        temp_dir.write_file("dist/drop.txt", "ignored\n");

        let block = render_index_block(temp_dir.path()).expect("render should succeed");
        let lines = IndexBlockLines::parse(&block);

        assert_eq!(lines.value("exclude_files"), "dist/*");
        assert_eq!(lines.value("."), ".gitignore,dist/");
        assert_eq!(lines.value("dist"), "keep.txt");
        assert!(!block.contains("drop.txt"));
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let test_id = NEXT_TEST_WORKSPACE_ID.fetch_add(1, Ordering::Relaxed);
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after unix epoch")
                .as_nanos();
            let root = PathBuf::from("/tmp").join(format!(
                "memoryroam-xtask-test-{}-{test_id}-{unique}",
                std::process::id(),
            ));
            fs::create_dir_all(&root).expect("temp root should be created");
            Self { root }
        }

        fn path(&self) -> &Path {
            &self.root
        }

        fn init_git_repository(&self) {
            self.git(["init"]);
            self.git(["config", "user.name", "Test User"]);
            self.git(["config", "user.email", "test@example.com"]);
        }

        fn write_file(&self, relative_path: &str, contents: &str) {
            let path = self.root.join(relative_path);
            let parent = path.parent().expect("test path should have a parent");
            fs::create_dir_all(parent).expect("parent directories should be created");
            fs::write(path, contents).expect("test file should be written");
        }

        fn remove_file(&self, relative_path: &str) {
            let path = self.root.join(relative_path);
            fs::remove_file(path).expect("test file should be removed");
        }

        fn git<const N: usize>(&self, arguments: [&str; N]) {
            let mut command = Command::new("git");
            command.args(arguments).current_dir(&self.root);
            super::clear_git_environment(&mut command);

            let output = command.output().expect("git command should start");
            assert!(
                output.status.success(),
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    struct IndexBlockLines {
        entries: BTreeMap<String, String>,
    }

    impl IndexBlockLines {
        fn parse(block: &str) -> Self {
            let entries = block
                .lines()
                .filter_map(|line| {
                    let content = line.strip_prefix('|')?;
                    let (key, value) = content.split_once(":{")?;
                    let value = value.strip_suffix('}')?;
                    Some((key.to_owned(), value.to_owned()))
                })
                .collect();

            Self { entries }
        }

        fn value(&self, key: &str) -> &str {
            self.entries
                .get(key)
                .unwrap_or_else(|| panic!("missing block entry for {key}"))
        }
    }
}
