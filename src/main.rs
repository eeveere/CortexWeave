use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::{Args, Parser, Subcommand, ValueEnum};
use cortexweave::{
    AppConfig, CortexError, CortexWeaveService, Result,
    adapters::mcp::{McpServer, WorkspaceHint},
    domain::{MemoryKind, MemoryRecord},
    workspace::WorkspaceSelector,
};
use serde::Serialize;
use serde_json::{Value, json};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "cortexweave", version, about)]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve(ServeArgs),
    Doctor,
    Status {
        workspace_id: Option<String>,
    },
    Metrics {
        workspace_id: Option<String>,
    },
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    Search(SearchArgs),
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    Reindex {
        workspace_id: String,
    },
}

#[derive(Debug, Args)]
struct ServeArgs {
    #[arg(long)]
    workspace_root: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum WorkspaceCommand {
    Add {
        root_path: String,
        #[arg(long)]
        name: Option<String>,
    },
    List,
}

#[derive(Debug, Args)]
struct SearchArgs {
    workspace_id: String,
    query: String,
    #[arg(long, value_enum, default_value_t = SearchMode::Hybrid)]
    mode: SearchMode,
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SearchMode {
    Semantic,
    Lexical,
    Hybrid,
}

#[derive(Debug, Subcommand)]
enum MemoryCommand {
    Add(MemoryAddArgs),
}

#[derive(Debug, Args)]
struct MemoryAddArgs {
    workspace_id: String,
    content: String,
    #[arg(long, value_enum, default_value_t = MemoryKindArg::Note)]
    kind: MemoryKindArg,
    #[arg(long)]
    session_id: Option<String>,
    #[arg(long)]
    task_id: Option<String>,
    #[arg(long = "related-path")]
    related_paths: Vec<String>,
    #[arg(long)]
    metadata: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum MemoryKindArg {
    Decision,
    Observation,
    Failure,
    Solution,
    Todo,
    Note,
    Checkpoint,
}

impl From<MemoryKindArg> for MemoryKind {
    fn from(value: MemoryKindArg) -> Self {
        match value {
            MemoryKindArg::Decision => Self::Decision,
            MemoryKindArg::Observation => Self::Observation,
            MemoryKindArg::Failure => Self::Failure,
            MemoryKindArg::Solution => Self::Solution,
            MemoryKindArg::Todo => Self::Todo,
            MemoryKindArg::Note => Self::Note,
            MemoryKindArg::Checkpoint => Self::Checkpoint,
        }
    }
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    config: Check,
    database: Check,
    migrations: Check,
    fts: Check,
    embedding: Check,
    embedding_capacity: Check,
    workspace_resolution: Check,
    registered_analyzers: Vec<String>,
    grammar_initialization: Check,
    watcher: Check,
    workspaces: Vec<WorkspaceCheck>,
}

#[derive(Debug, Serialize)]
struct Check {
    ok: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
struct WorkspaceCheck {
    id: String,
    root_path: String,
    root_exists: bool,
    root_is_directory: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::load(cli.config.as_deref())?;
    init_logging(&config.logging.level)?;
    run(CortexWeaveService::open(config).await?, cli.command).await
}

async fn run(service: CortexWeaveService, command: Command) -> Result<()> {
    match command {
        Command::Serve(args) => {
            info!("CortexWeave MCP server started");
            McpServer::with_workspace_hint(
                Arc::new(service),
                serve_workspace_hint(args.workspace_root),
            )
            .serve_stdio()
            .await?;
            info!("MCP client disconnected");
        }
        Command::Doctor => {
            let report = doctor(&service).await;
            let healthy = report.config.ok
                && report.database.ok
                && report.migrations.ok
                && report.fts.ok
                && report.embedding.ok
                && report.workspace_resolution.ok
                && report.grammar_initialization.ok
                && report.watcher.ok
                && report
                    .workspaces
                    .iter()
                    .all(|workspace| workspace.root_is_directory);
            print_json(&report)?;
            if !healthy {
                return Err(CortexError::Configuration(
                    "doctor found one or more unhealthy checks".into(),
                ));
            }
        }
        Command::Status { workspace_id } => {
            if let Some(workspace_id) = workspace_id {
                print_json(service.workspace_status(&workspace_id).await?)?;
            } else {
                let mut statuses = Vec::new();
                for workspace in service.list_workspaces().await? {
                    statuses.push(service.workspace_status(&workspace.id).await?);
                }
                print_json(statuses)?;
            }
        }
        Command::Metrics { workspace_id } => {
            print_json(service.instrumentation(workspace_id.as_deref()).await?)?;
        }
        Command::Workspace { command } => match command {
            WorkspaceCommand::Add { root_path, name } => {
                let name = name.unwrap_or_else(|| workspace_name(&root_path));
                print_json(service.register_workspace(root_path, name).await?)?;
            }
            WorkspaceCommand::List => print_json(service.list_workspaces().await?)?,
        },
        Command::Search(args) => {
            let limit = args.limit.unwrap_or(service.config().retrieval.default_k);
            let results = match args.mode {
                SearchMode::Semantic => {
                    service
                        .semantic_search(&args.workspace_id, &args.query, limit)
                        .await?
                }
                SearchMode::Lexical => {
                    service
                        .lexical_search(&args.workspace_id, &args.query, limit)
                        .await?
                }
                SearchMode::Hybrid => {
                    service
                        .hybrid_search(&args.workspace_id, &args.query, limit)
                        .await?
                }
            };
            print_json(results)?;
        }
        Command::Memory { command } => match command {
            MemoryCommand::Add(args) => {
                let metadata: Value = args
                    .metadata
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?
                    .unwrap_or_else(|| json!({}));
                let mut memory =
                    MemoryRecord::new(args.workspace_id, args.kind.into(), args.content);
                memory.session_id = args.session_id;
                memory.task_id = args.task_id;
                memory.related_paths = args.related_paths;
                memory.metadata = metadata;
                print_json(service.record_memory(memory).await?)?;
            }
        },
        Command::Reindex { workspace_id } => {
            print_json(service.workspace_reindex(&workspace_id).await?)?
        }
    }
    Ok(())
}

fn serve_workspace_hint(argument: Option<PathBuf>) -> Option<WorkspaceHint> {
    argument
        .or_else(|| std::env::var_os("CORTEXWEAVE_WORKSPACE_ROOT").map(PathBuf::from))
        .filter(|path| !path.as_os_str().is_empty())
        .map(WorkspaceHint::RootPath)
}

async fn doctor(service: &CortexWeaveService) -> DoctorReport {
    let database = check(
        service.storage().health_check().await,
        "SQLite connection is available",
    );
    let migrations = Check {
        ok: database.ok,
        detail: if database.ok {
            "migrations opened successfully".into()
        } else {
            "database could not be opened to run migrations".into()
        },
    };
    let fts = check(
        service.storage().fts_health_check().await,
        "chunk and memory FTS tables are available",
    );
    let embedding = match service
        .embeddings()
        .embed_queries(&["cortexweave doctor".into()])
        .await
    {
        Ok(vectors) if vectors.len() == 1 && !vectors[0].is_empty() => Check {
            ok: true,
            detail: format!(
                "{} returned dimension {}",
                service.embeddings().request_model_name(),
                vectors[0].len()
            ),
        },
        Ok(_) => Check {
            ok: false,
            detail: "embedding endpoint returned an empty or malformed test vector".into(),
        },
        Err(error) => Check {
            ok: false,
            detail: error.to_string(),
        },
    };
    let limits = service.embeddings().limits();
    let embedding_capacity = Check {
        ok: true,
        detail: format!(
            "source=provider_contract input={:?} usable={:?} batch={:?} items={} reserved={} counter={} accuracy={:?}",
            limits.max_input_tokens,
            limits.input_budget(),
            limits.max_batch_tokens,
            limits.max_batch_items,
            limits.reserved_tokens,
            service.embeddings().token_counter_id(),
            service.embeddings().token_counter_accuracy(),
        ),
    };
    let workspaces: Vec<WorkspaceCheck> = service
        .list_workspaces()
        .await
        .map(|workspaces| {
            workspaces
                .into_iter()
                .map(|workspace| {
                    let root = Path::new(&workspace.root_path);
                    let root_exists = root.exists();
                    let root_is_directory = root.is_dir();
                    WorkspaceCheck {
                        id: workspace.id,
                        root_path: workspace.root_path,
                        root_exists,
                        root_is_directory,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let configured_workspace_hint = std::env::var_os("CORTEXWEAVE_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty());
    let workspace_resolution = match configured_workspace_hint {
        Some(root) => match service
            .resolve_workspace(
                WorkspaceSelector::Default,
                Some(WorkspaceSelector::RootPath(root.clone())),
            )
            .await
        {
            Ok(workspace) => Check {
                ok: true,
                detail: format!(
                    "configured root {:?} resolves to workspace {} ({})",
                    root.display().to_string(),
                    workspace.name,
                    workspace.id
                ),
            },
            Err(error) => Check {
                ok: false,
                detail: format!(
                    "configured root {:?} does not resolve: {error}",
                    root.display().to_string()
                ),
            },
        },
        None => match workspaces.as_slice() {
            [] => Check {
                ok: true,
                detail: "no default root is configured and no workspaces are registered".into(),
            },
            [workspace] => Check {
                ok: true,
                detail: format!(
                    "no default root is configured; singleton fallback selects workspace {}",
                    workspace.id
                ),
            },
            _ => Check {
                ok: true,
                detail: "no default root is configured; MCP calls need an explicit selector".into(),
            },
        },
    };
    let registered_analyzers = service.analyzers().registered_languages();
    let grammar_initialization = match registered_analyzers.iter().try_for_each(|language| {
        service
            .analyzers()
            .for_language(language)
            .ok_or_else(|| CortexError::Analysis(format!("missing analyzer {language}")))?
            .analyze(Path::new(doctor_path(language)), doctor_source(language))
            .map(|_| ())
    }) {
        Ok(()) => Check {
            ok: true,
            detail: "all registered analyzer grammars initialized".into(),
        },
        Err(error) => Check {
            ok: false,
            detail: error.to_string(),
        },
    };
    let watcher = Check {
        ok: workspaces
            .iter()
            .all(|workspace| workspace.root_is_directory),
        detail: "all registered workspace roots are available for watcher startup".into(),
    };
    DoctorReport {
        config: Check {
            ok: true,
            detail: "configuration loaded".into(),
        },
        database,
        migrations,
        fts,
        embedding,
        embedding_capacity,
        workspace_resolution,
        registered_analyzers,
        grammar_initialization,
        watcher,
        workspaces,
    }
}

fn doctor_path(language: &str) -> &'static str {
    match language {
        "rust" => "doctor.rs",
        "python" => "doctor.py",
        "javascript" => "doctor.js",
        "typescript" => "doctor.ts",
        "csharp" => "Doctor.cs",
        "go" => "doctor.go",
        _ => "doctor.txt",
    }
}

fn doctor_source(language: &str) -> &'static str {
    match language {
        "rust" => "fn doctor() {}\n",
        "python" => "def doctor():\n    pass\n",
        "javascript" | "typescript" => "function doctor() {}\n",
        "csharp" => "class Doctor {}\n",
        "go" => "package doctor\nfunc Doctor() {}\n",
        _ => "doctor\n",
    }
}

fn check(result: Result<()>, success: &str) -> Check {
    match result {
        Ok(()) => Check {
            ok: true,
            detail: success.into(),
        },
        Err(error) => Check {
            ok: false,
            detail: error.to_string(),
        },
    }
}

fn workspace_name(root_path: &str) -> String {
    Path::new(root_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(root_path)
        .to_owned()
}

fn print_json(value: impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn init_logging(level: &str) -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(level))
        .map_err(|error| CortexError::Configuration(error.to_string()))?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|error| CortexError::Configuration(error.to_string()))
}
