use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::Local;
use clap::{Args, Parser, Subcommand};
use memoryroam_application::{
    DayView, NoteResult, ReadContextView, RootApplyResult, RootCreateResult, apply_root_link,
    create_root, note_today, open_day, read_node_context,
};
use memoryroam_domain::{KernelError, NodeId};
use memoryroam_storage_sqlite::SqliteStore;
use memoryroam_write::init;

#[derive(Debug, Parser)]
#[command(name = "memoryroam")]
#[command(about = "Note-taking oriented MemoryRoam CLI")]
struct Cli {
    #[arg(long, global = true, default_value = "memoryroam.sqlite3")]
    db: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init,
    Note(NoteCommand),
    Day(DayCommand),
    Read(ReadCommand),
    Root(RootCommand),
}

#[derive(Debug, Args)]
struct NoteCommand {
    #[arg(allow_hyphen_values = true)]
    content: Option<String>,
}

#[derive(Debug, Args)]
struct DayCommand {
    note_date: Option<String>,
}

#[derive(Debug, Args)]
struct ReadCommand {
    id: i64,
}

#[derive(Debug, Args)]
struct RootCommand {
    #[command(subcommand)]
    command: RootSubcommand,
}

#[derive(Debug, Subcommand)]
enum RootSubcommand {
    Create {
        #[arg(allow_hyphen_values = true)]
        content: Option<String>,
    },
    Apply {
        root_id: i64,
        #[arg(long)]
        #[arg(allow_hyphen_values = true)]
        text: Option<String>,
        #[arg(long)]
        node: Vec<i64>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<(), KernelError> {
    match cli.command {
        Command::Init => {
            let mut store = SqliteStore::open_or_create(&cli.db)?;
            init(&mut store)?;
            println!("initialized {}", store.database_path().display());
        }
        Command::Note(command) => {
            let mut store = SqliteStore::open_existing(&cli.db)?;
            let content = read_content_input(command.content)?;
            let note_date = today_date();
            let result = note_today(&mut store, &note_date, &content)?;
            print_note_result(&result);
        }
        Command::Day(command) => {
            let store = SqliteStore::open_existing(&cli.db)?;
            let note_date = command.note_date.unwrap_or_else(today_date);
            let result = open_day(&store, &note_date)?;
            print_day_view(&result, &cli.db);
        }
        Command::Read(command) => {
            let store = SqliteStore::open_existing(&cli.db)?;
            let result = read_node_context(&store, parse_node_id(command.id)?, 5500)?;
            print_read_context(&result);
        }
        Command::Root(root_command) => match root_command.command {
            RootSubcommand::Create { content } => {
                let mut store = SqliteStore::open_existing(&cli.db)?;
                let content = read_content_input(content)?;
                let result = create_root(&mut store, &content)?;
                print_root_create_result(&result);
            }
            RootSubcommand::Apply {
                root_id,
                text,
                node,
            } => {
                let mut store = SqliteStore::open_existing(&cli.db)?;
                if node.is_empty() {
                    return Err(KernelError::Input(String::from(
                        "root apply requires at least one --node",
                    )));
                }
                let node_ids = node
                    .into_iter()
                    .map(parse_node_id)
                    .collect::<Result<Vec<_>, _>>()?;
                let result = apply_root_link(
                    &mut store,
                    parse_node_id(root_id)?,
                    text.as_deref(),
                    &node_ids,
                )?;
                print_root_apply_result(&result);
            }
        },
    }

    Ok(())
}

fn today_date() -> String {
    Local::now().date_naive().format("%F").to_string()
}

fn parse_node_id(value: i64) -> Result<NodeId, KernelError> {
    NodeId::new(value).map_err(|error| KernelError::Input(error.to_string()))
}

fn read_content_input(explicit_content: Option<String>) -> Result<String, KernelError> {
    if let Some(content) = explicit_content {
        return Ok(content);
    }

    let mut buffer = String::new();
    io::stdin()
        .read_to_string(&mut buffer)
        .map_err(|error| KernelError::Input(error.to_string()))?;
    if buffer.is_empty() {
        return Err(KernelError::Input(String::from(
            "content must be provided as an argument or via stdin",
        )));
    }

    if buffer.ends_with('\n') {
        buffer.pop();
        if buffer.ends_with('\r') {
            buffer.pop();
        }
    }

    Ok(buffer)
}

fn print_note_result(result: &NoteResult) {
    println!("{}", result.note_date);
    println!("+[{}] {}", result.node.id, result.node.rendered_content);
}

fn print_day_view(view: &DayView, database_path: &Path) {
    println!("{}", view.note_date);
    println!();
    if view.entries.is_empty() {
        println!("empty");
        if view.note_date == today_date() {
            println!(
                "use: memoryroam --db {} note \"...\"",
                shell_quote_path(database_path)
            );
        }
        return;
    }

    for entry in &view.entries {
        println!("-[{}] {}", entry.id, entry.rendered_content);
    }
}

fn print_root_create_result(result: &RootCreateResult) {
    println!("root [{}] {}", result.root.id, result.root.rendered_content);
    println!();
    println!("matches:");
    for entry in &result.matches {
        println!("-[{}] {}", entry.id, entry.rendered_content);
    }
    if result.hidden_match_count > 0 {
        println!("...{} more matches...", result.hidden_match_count);
    }
}

fn print_root_apply_result(result: &RootApplyResult) {
    println!("updated:");
    for entry in &result.updated_nodes {
        println!("-[{}] {}", entry.id, entry.rendered_content);
    }
}

fn print_read_context(view: &ReadContextView) {
    if view.prev_hidden_count > 0 {
        println!("...prev {} sibling hiding...", view.prev_hidden_count);
    }
    for entry in &view.prev_siblings {
        println!("-[{}] {}", entry.id, entry.rendered_content);
    }
    println!("=[{}] {}", view.current.id, view.current.rendered_content);
    for backlink in &view.backlinks {
        println!(
            ">[bl:{}] {}",
            backlink.source.id, backlink.source.rendered_content
        );
    }
    if view.hidden_backlink_count > 0 {
        println!("...{} backlinks hiding...", view.hidden_backlink_count);
    }
    for child in &view.children {
        println!("--[{}] {}", child.node.id, child.node.rendered_content);
        for backlink in &child.backlinks {
            println!(
                ">>[bl:{}] {}",
                backlink.source.id, backlink.source.rendered_content
            );
        }
        if child.hidden_backlink_count > 0 {
            println!(
                "...{} child backlinks hiding...",
                child.hidden_backlink_count
            );
        }
    }
    if view.hidden_child_count > 0 {
        println!("...{} children hiding...", view.hidden_child_count);
    }
    for entry in &view.next_siblings {
        println!("-[{}] {}", entry.id, entry.rendered_content);
    }
    if view.next_hidden_count > 0 {
        println!("...next {} sibling hiding...", view.next_hidden_count);
    }
}

fn shell_quote_path(path: &Path) -> String {
    let raw = path.display().to_string();
    let escaped = raw.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}
