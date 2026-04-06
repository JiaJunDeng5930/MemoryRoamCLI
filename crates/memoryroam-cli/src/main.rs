use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgGroup, Args, Parser, Subcommand};
use memoryroam_domain::{DeleteMode, KernelError, NodeId, Placement, ReadNodeView};
use memoryroam_read::{list_aliases, list_children, list_top_level, read_node};
use memoryroam_storage_sqlite::SqliteStore;
use memoryroam_write::{
    add_aliases, create_nodes, delete_node, init, move_node, remove_alias, update_node,
};

#[derive(Debug, Parser)]
#[command(name = "memoryroam")]
#[command(about = "Structured note kernel CLI")]
struct Cli {
    #[arg(long, global = true, default_value = "memoryroam.sqlite3")]
    db: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init,
    Create(CreateCommand),
    Update(UpdateCommand),
    Read(NodeSelector),
    List(ListCommand),
    Move(MoveCommand),
    Delete(DeleteCommand),
    Alias(AliasCommand),
}

#[derive(Debug, Args)]
struct NodeSelector {
    #[arg(long)]
    id: i64,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("placement")
        .args([
            "top_level_first",
            "top_level_last",
            "before",
            "after",
            "first_child_of",
            "last_child_of"
        ])
        .multiple(false)
))]
struct PlacementArgs {
    #[arg(long)]
    top_level_first: bool,
    #[arg(long)]
    top_level_last: bool,
    #[arg(long)]
    before: Option<i64>,
    #[arg(long)]
    after: Option<i64>,
    #[arg(long)]
    first_child_of: Option<i64>,
    #[arg(long)]
    last_child_of: Option<i64>,
}

impl PlacementArgs {
    fn into_placement(self, default: Option<Placement>) -> Result<Placement, KernelError> {
        if self.top_level_first {
            return Ok(Placement::TopLevelFirst);
        }
        if self.top_level_last {
            return Ok(Placement::TopLevelLast);
        }
        if let Some(id) = self.before {
            return Ok(Placement::Before(parse_node_id(id)?));
        }
        if let Some(id) = self.after {
            return Ok(Placement::After(parse_node_id(id)?));
        }
        if let Some(id) = self.first_child_of {
            return Ok(Placement::FirstChildOf(parse_node_id(id)?));
        }
        if let Some(id) = self.last_child_of {
            return Ok(Placement::LastChildOf(parse_node_id(id)?));
        }

        default.ok_or(KernelError::Input(String::from(
            "a placement option is required",
        )))
    }
}

#[derive(Debug, Args)]
struct CreateCommand {
    #[arg(long)]
    content: Option<String>,
    #[arg(long)]
    alias: Vec<String>,
    #[command(flatten)]
    placement: PlacementArgs,
}

#[derive(Debug, Args)]
struct UpdateCommand {
    #[arg(long)]
    id: i64,
    #[arg(long)]
    content: Option<String>,
}

#[derive(Debug, Args)]
struct ListCommand {
    #[arg(long)]
    top_level: bool,
    #[arg(long)]
    children_of: Option<i64>,
}

#[derive(Debug, Args)]
struct MoveCommand {
    #[arg(long)]
    id: i64,
    #[command(flatten)]
    placement: PlacementArgs,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("delete_mode")
        .args([
            "cascade",
            "top_level_first",
            "top_level_last",
            "before",
            "after",
            "first_child_of",
            "last_child_of"
        ])
        .multiple(false)
        .required(true)
))]
struct DeleteCommand {
    #[arg(long)]
    id: i64,
    #[arg(long)]
    cascade: bool,
    #[command(flatten)]
    placement: PlacementArgs,
}

#[derive(Debug, Subcommand)]
enum AliasSubcommand {
    Add {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        text: Vec<String>,
    },
    Remove {
        #[arg(long)]
        id: i64,
        #[arg(long)]
        text: String,
    },
    List {
        #[arg(long)]
        id: i64,
    },
}

#[derive(Debug, Args)]
struct AliasCommand {
    #[command(subcommand)]
    command: AliasSubcommand,
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
    let mut store = SqliteStore::open(&cli.db)?;

    match cli.command {
        Command::Init => {
            init(&mut store)?;
            println!("initialized {}", store.database_path().display());
        }
        Command::Create(command) => {
            let placement = command
                .placement
                .into_placement(Some(Placement::TopLevelLast))?;
            let input = read_content_input(command.content)?;
            let node_ids = create_nodes(&mut store, &input, &command.alias, placement)?;
            println!("created:");
            for node_id in node_ids {
                println!("- {node_id}");
            }
        }
        Command::Update(command) => {
            let content = read_content_input(command.content)?;
            update_node(&mut store, parse_node_id(command.id)?, &content)?;
            println!("updated {}", command.id);
        }
        Command::Read(command) => {
            let view = read_node(&store, parse_node_id(command.id)?)?;
            print_read_view(&view);
        }
        Command::List(command) => {
            if command.top_level == command.children_of.is_some() {
                return Err(KernelError::Input(String::from(
                    "choose either --top-level or --children-of",
                )));
            }

            let entries = if command.top_level {
                list_top_level(&store)?
            } else {
                list_children(
                    &store,
                    parse_node_id(command.children_of.expect("children_of checked above"))?,
                )?
            };

            for entry in entries {
                println!("{}\t{}", entry.id, entry.rendered_content);
            }
        }
        Command::Move(command) => {
            let placement = command.placement.into_placement(None)?;
            move_node(&mut store, parse_node_id(command.id)?, placement)?;
            println!("moved {}", command.id);
        }
        Command::Delete(command) => {
            let mode = if command.cascade {
                DeleteMode::Cascade
            } else {
                DeleteMode::Reparent(command.placement.into_placement(None)?)
            };
            delete_node(&mut store, parse_node_id(command.id)?, mode)?;
            println!("deleted {}", command.id);
        }
        Command::Alias(alias_command) => match alias_command.command {
            AliasSubcommand::Add { id, text } => {
                add_aliases(&mut store, parse_node_id(id)?, &text)?;
                println!("alias-added {}", id);
            }
            AliasSubcommand::Remove { id, text } => {
                remove_alias(&mut store, parse_node_id(id)?, &text)?;
                println!("alias-removed {}", id);
            }
            AliasSubcommand::List { id } => {
                for alias in list_aliases(&store, parse_node_id(id)?)? {
                    println!("{}", alias.as_str());
                }
            }
        },
    }

    Ok(())
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
            "content must be provided via --content or stdin",
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

fn print_read_view(view: &ReadNodeView) {
    println!("Node: {}", view.node.id);
    println!("Content: {}", view.node.rendered_content);
    println!(
        "Parent: {}",
        view.parent
            .as_ref()
            .map(format_node_line)
            .unwrap_or_else(|| String::from("none"))
    );
    println!(
        "Prev sibling: {}",
        view.prev_sibling
            .as_ref()
            .map(format_node_line)
            .unwrap_or_else(|| String::from("none"))
    );
    println!(
        "Next sibling: {}",
        view.next_sibling
            .as_ref()
            .map(format_node_line)
            .unwrap_or_else(|| String::from("none"))
    );
    println!("Children:");
    if view.children.is_empty() {
        println!("- none");
    } else {
        for child in &view.children {
            println!("- {}", format_node_line(child));
        }
    }
    println!("Incoming links:");
    if view.incoming_links.is_empty() {
        println!("- none");
    } else {
        for incoming in &view.incoming_links {
            println!(
                "- {} [ordinal {}] ({})",
                format_node_line(&incoming.source),
                incoming.ordinal,
                incoming.path
            );
        }
    }
}

fn format_node_line(line: &memoryroam_domain::NodeLine) -> String {
    format!("{} -> {}", line.id, line.rendered_content)
}
