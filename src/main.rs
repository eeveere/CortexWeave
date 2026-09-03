use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use cortexweave::{
    AppConfig, CortexError, CortexWeaveService, Result, WorkspaceGraphStatus,
    adapters::mcp::{McpServer, WorkspaceHint},
    domain::{
        Checkpoint, ConsolidationAcceptanceRequest, ConsolidationRequest, ContextRequest,
        ContextSourceType, EpisodeCreator, EpisodeEventAssociationRequest, EpisodeListRequest,
        EpisodeStartRequest, EpisodeTerminalRequest, EpisodeType, ExperienceAssessmentKind,
        ExperienceAssessmentReviewRequest, ExperienceDisputeProposalRequest,
        ExperienceSearchRequest, FailureSignature, MemoryKind, MemoryRecord, ResumeContextRequest,
        StructuralReadOptions,
    },
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
    Readiness {
        workspace_id: Option<String>,
    },
    Metrics {
        workspace_id: Option<String>,
    },
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    Graph {
        #[command(subcommand)]
        command: GraphCommand,
    },
    Search(SearchArgs),
    Context(ContextArgs),
    Resume(ResumeArgs),
    WorkingSet(WorkingSetArgs),
    ContextPin(ContextSourceArgs),
    ContextUnpin(ContextSourceArgs),
    Checkpoint {
        #[command(subcommand)]
        command: CheckpointCommand,
    },
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    Episode {
        #[command(subcommand)]
        command: EpisodeCommand,
    },
    Experience {
        #[command(subcommand)]
        command: ExperienceCommand,
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

#[derive(Debug, Subcommand)]
enum GraphCommand {
    Rebuild {
        workspace_id: String,
        #[arg(long)]
        force: bool,
    },
    Status {
        workspace_id: String,
    },
    Find {
        workspace_id: String,
        symbol_or_path: String,
        #[command(flatten)]
        options: GraphReadArgs,
    },
    Neighbors {
        workspace_id: String,
        node_id: String,
        #[command(flatten)]
        options: GraphReadArgs,
    },
    Callers {
        workspace_id: String,
        node_id: String,
        #[command(flatten)]
        options: GraphReadArgs,
    },
    Callees {
        workspace_id: String,
        node_id: String,
        #[command(flatten)]
        options: GraphReadArgs,
    },
    References {
        workspace_id: String,
        node_id: String,
        #[command(flatten)]
        options: GraphReadArgs,
    },
    Implementations {
        workspace_id: String,
        node_id: String,
        #[command(flatten)]
        options: GraphReadArgs,
    },
    Tests {
        workspace_id: String,
        node_id: String,
        #[command(flatten)]
        options: GraphReadArgs,
    },
    Dependencies {
        workspace_id: String,
        node_id: String,
        #[command(flatten)]
        options: GraphReadArgs,
    },
    Dependents {
        workspace_id: String,
        node_id: String,
        #[command(flatten)]
        options: GraphReadArgs,
    },
    ImpactSymbol {
        workspace_id: String,
        symbol: String,
        #[command(flatten)]
        options: GraphReadArgs,
    },
    ImpactPath {
        workspace_id: String,
        path: String,
        #[command(flatten)]
        options: GraphReadArgs,
    },
}

#[derive(Debug, Args)]
struct GraphReadArgs {
    #[arg(long)]
    allow_stale: bool,
    #[arg(long)]
    max_nodes: Option<usize>,
    #[arg(long)]
    max_edges: Option<usize>,
    #[arg(long)]
    max_depth: Option<usize>,
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

#[derive(Debug, Args)]
struct ContextArgs {
    workspace_id: String,
    query: String,
    #[arg(long)]
    session_id: Option<String>,
    #[arg(long)]
    task_id: Option<String>,
    #[arg(long)]
    token_budget: Option<usize>,
    #[arg(long)]
    no_code: bool,
    #[arg(long)]
    no_documents: bool,
    #[arg(long)]
    no_memories: bool,
    #[arg(long)]
    no_events: bool,
    #[arg(long = "path-scope")]
    path_scope: Vec<String>,
    #[arg(long = "language-scope")]
    language_scope: Vec<String>,
    #[arg(
        long,
        help = "Canonical FailureSignature JSON for optional historical Experience context"
    )]
    active_failure_signature: Option<String>,
    #[arg(long)]
    explain: bool,
}

#[derive(Debug, Args)]
struct ResumeArgs {
    workspace_id: String,
    #[arg(long)]
    session_id: Option<String>,
    #[arg(long)]
    task_id: Option<String>,
    #[arg(long)]
    token_budget: Option<usize>,
    #[arg(long)]
    explain: bool,
}

#[derive(Debug, Args)]
struct WorkingSetArgs {
    workspace_id: String,
    session_id: String,
    #[arg(long)]
    task_id: Option<String>,
}

#[derive(Debug, Args)]
struct ContextSourceArgs {
    workspace_id: String,
    session_id: String,
    source_id: String,
    source_type: String,
    #[arg(long)]
    task_id: Option<String>,
}

#[derive(Debug, Subcommand)]
enum CheckpointCommand {
    Create(CheckpointCreateArgs),
    Latest(CheckpointLatestArgs),
}

#[derive(Debug, Args)]
struct CheckpointCreateArgs {
    workspace_id: String,
    session_id: String,
    content: String,
    #[arg(long)]
    task_id: Option<String>,
    #[arg(long)]
    objective: Option<String>,
    #[arg(long)]
    completed: Vec<String>,
    #[arg(long)]
    decision_id: Vec<String>,
    #[arg(long)]
    open_problem: Vec<String>,
    #[arg(long)]
    related_path: Vec<String>,
    #[arg(long)]
    related_symbol: Vec<String>,
    #[arg(long)]
    next_action: Option<String>,
}

#[derive(Debug, Args)]
struct CheckpointLatestArgs {
    workspace_id: String,
    #[arg(long, conflicts_with = "task_id")]
    session_id: Option<String>,
    #[arg(long)]
    task_id: Option<String>,
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

#[derive(Debug, Subcommand)]
enum EpisodeCommand {
    Start(EpisodeStartArgs),
    AddEvents(EpisodeAddEventsArgs),
    Close(EpisodeTerminalArgs),
    Abandon(EpisodeTerminalArgs),
    Show(EpisodeShowArgs),
    List(EpisodeListArgs),
}

#[derive(Debug, Args)]
struct EpisodeStartArgs {
    workspace_id: String,
    #[arg(long)]
    session_id: String,
    #[arg(long)]
    task_id: Option<String>,
    #[arg(long = "type", value_enum)]
    episode_type: EpisodeTypeArg,
    #[arg(long)]
    title: Option<String>,
}

#[derive(Debug, Args)]
struct EpisodeAddEventsArgs {
    workspace_id: String,
    episode_id: String,
    #[arg(long)]
    expected_version: u64,
    #[arg(long)]
    request_key: String,
    #[arg(required = true, num_args = 1..=100)]
    event_ids: Vec<String>,
}

#[derive(Debug, Args)]
struct EpisodeTerminalArgs {
    workspace_id: String,
    episode_id: String,
    #[arg(long)]
    expected_version: u64,
    #[arg(long)]
    request_key: String,
}

#[derive(Debug, Args)]
struct EpisodeShowArgs {
    workspace_id: String,
    episode_id: String,
}

#[derive(Debug, Args)]
struct EpisodeListArgs {
    workspace_id: String,
    #[arg(long)]
    session_id: Option<String>,
    #[arg(long)]
    task_id: Option<String>,
    #[arg(long, default_value_t = 100, value_parser = parse_cli_episode_limit)]
    limit: usize,
}

#[derive(Debug, Subcommand)]
enum ExperienceCommand {
    Preview(ExperienceEpisodeArgs),
    Consolidate(ExperienceConsolidateArgs),
    Search(ExperienceSearchArgs),
    Show(ExperienceShowArgs),
    Explain(ExperienceShowArgs),
    History(ExperienceHistoryArgs),
    Assess(ExperienceAssessArgs),
    ProposeDispute(ExperienceProposeDisputeArgs),
}

#[derive(Debug, Args)]
struct ExperienceEpisodeArgs {
    workspace_id: String,
    #[arg(long)]
    episode_id: String,
    #[arg(long)]
    expected_version: u64,
}

#[derive(Debug, Args)]
struct ExperienceConsolidateArgs {
    #[command(flatten)]
    episode: ExperienceEpisodeArgs,
    #[arg(long)]
    expected_fingerprint: String,
    #[arg(long)]
    expected_proposal_hash: String,
}

#[derive(Debug, Args)]
struct ExperienceSearchArgs {
    workspace_id: String,
    #[arg(long)]
    query: Option<String>,
    #[arg(
        long,
        help = "Canonical FailureSignature JSON returned by a prior API response"
    )]
    failure_signature: Option<String>,
    #[arg(long)]
    include_historical: bool,
    #[arg(long, default_value_t = 20, value_parser = parse_cli_experience_limit)]
    limit: usize,
}

#[derive(Debug, Args)]
struct ExperienceShowArgs {
    workspace_id: String,
    experience_id: String,
}

#[derive(Debug, Args)]
struct ExperienceHistoryArgs {
    workspace_id: String,
    experience_id: String,
    #[arg(long, default_value_t = cortexweave::domain::DEFAULT_EXPERIENCE_ASSESSMENT_PAGE_LIMIT)]
    limit: usize,
    #[arg(long)]
    after_created_at: Option<String>,
    #[arg(long)]
    after_id: Option<String>,
}

#[derive(Debug, Args)]
struct ExperienceAssessArgs {
    workspace_id: String,
    experience_id: String,
    #[arg(long, value_enum)]
    kind: ExperienceAssessmentKindArg,
    #[arg(long)]
    reviewed_by: String,
    #[arg(long)]
    request_key: String,
    #[arg(long)]
    reason: String,
    #[arg(long)]
    replacement_experience_id: Option<String>,
    #[arg(long = "evidence-event-id", required = true, num_args = 1..=64)]
    evidence_event_ids: Vec<String>,
}

#[derive(Debug, Args)]
struct ExperienceProposeDisputeArgs {
    workspace_id: String,
    #[arg(long)]
    failure_signature: String,
    #[arg(long = "recurring-failure-event-id", required = true, num_args = 1..=64)]
    recurring_failure_event_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum EpisodeTypeArg {
    Implementation,
    Debugging,
    Verification,
    Investigation,
    Refactor,
    Configuration,
    DependencyChange,
    ArchitectureDecision,
    Documentation,
    Other,
}

impl From<EpisodeTypeArg> for EpisodeType {
    fn from(value: EpisodeTypeArg) -> Self {
        match value {
            EpisodeTypeArg::Implementation => Self::Implementation,
            EpisodeTypeArg::Debugging => Self::Debugging,
            EpisodeTypeArg::Verification => Self::Verification,
            EpisodeTypeArg::Investigation => Self::Investigation,
            EpisodeTypeArg::Refactor => Self::Refactor,
            EpisodeTypeArg::Configuration => Self::Configuration,
            EpisodeTypeArg::DependencyChange => Self::DependencyChange,
            EpisodeTypeArg::ArchitectureDecision => Self::ArchitectureDecision,
            EpisodeTypeArg::Documentation => Self::Documentation,
            EpisodeTypeArg::Other => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ExperienceAssessmentKindArg {
    Disputed,
    Refuted,
    Superseded,
    Confirmed,
}

impl From<ExperienceAssessmentKindArg> for ExperienceAssessmentKind {
    fn from(value: ExperienceAssessmentKindArg) -> Self {
        match value {
            ExperienceAssessmentKindArg::Disputed => Self::Disputed,
            ExperienceAssessmentKindArg::Refuted => Self::Refuted,
            ExperienceAssessmentKindArg::Superseded => Self::Superseded,
            ExperienceAssessmentKindArg::Confirmed => Self::Confirmed,
        }
    }
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
    experience_persistence: Check,
    workspace_inventory: Check,
    workspace_resolution: Check,
    registered_analyzers: Vec<String>,
    grammar_initialization: Check,
    watcher: Check,
    graph: Check,
    graph_workspaces: Vec<GraphWorkspaceCheck>,
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

#[derive(Debug, Serialize)]
struct GraphWorkspaceCheck {
    id: String,
    status: Option<WorkspaceGraphStatus>,
    error: Option<String>,
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
                && report.embedding_capacity.ok
                && report.experience_persistence.ok
                && report.workspace_inventory.ok
                && report.workspace_resolution.ok
                && report.grammar_initialization.ok
                && report.watcher.ok
                && report.graph.ok
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
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(service.workspace_status(&workspace_id).await?)?;
            } else {
                let mut statuses = Vec::new();
                for workspace in service.list_workspaces().await? {
                    statuses.push(service.workspace_status(&workspace.id).await?);
                }
                print_json(statuses)?;
            }
        }
        Command::Readiness { workspace_id } => {
            if let Some(workspace_id) = workspace_id {
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(service.workspace_readiness(&workspace_id).await?)?;
            } else {
                let mut reports = Vec::new();
                for workspace in service.list_workspaces().await? {
                    reports.push(service.workspace_readiness(&workspace.id).await?);
                }
                print_json(reports)?;
            }
        }
        Command::Metrics { workspace_id } => {
            let workspace_id = match workspace_id {
                Some(selector) => Some(cli_workspace_id(&service, &selector).await?),
                None => None,
            };
            print_json(service.instrumentation(workspace_id.as_deref()).await?)?;
        }
        Command::Workspace { command } => match command {
            WorkspaceCommand::Add { root_path, name } => {
                let name = name.unwrap_or_else(|| workspace_name(&root_path));
                print_json(service.register_workspace(root_path, name).await?)?;
            }
            WorkspaceCommand::List => print_json(service.list_workspaces().await?)?,
        },
        Command::Graph { command } => match command {
            GraphCommand::Rebuild {
                workspace_id,
                force,
            } => print_json(
                service
                    .workspace_graph_repair(
                        &cli_workspace_id(&service, &workspace_id).await?,
                        if force {
                            cortexweave::domain::GraphRepairMode::Force
                        } else {
                            cortexweave::domain::GraphRepairMode::IfNeeded
                        },
                    )
                    .await?,
            )?,
            GraphCommand::Status { workspace_id } => {
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(service.workspace_graph_status(&workspace_id).await?)?
            }
            GraphCommand::Find {
                workspace_id,
                symbol_or_path,
                options,
            } => {
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(
                    service
                        .graph_find_symbol(
                            &workspace_id,
                            &symbol_or_path,
                            &graph_read_options(options),
                        )
                        .await?,
                )?
            }
            GraphCommand::Neighbors {
                workspace_id,
                node_id,
                options,
            } => {
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(
                    service
                        .graph_neighbors(&workspace_id, &node_id, &graph_read_options(options))
                        .await?,
                )?
            }
            GraphCommand::Callers {
                workspace_id,
                node_id,
                options,
            } => {
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(
                    service
                        .graph_callers(&workspace_id, &node_id, &graph_read_options(options))
                        .await?,
                )?
            }
            GraphCommand::Callees {
                workspace_id,
                node_id,
                options,
            } => {
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(
                    service
                        .graph_callees(&workspace_id, &node_id, &graph_read_options(options))
                        .await?,
                )?
            }
            GraphCommand::References {
                workspace_id,
                node_id,
                options,
            } => {
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(
                    service
                        .graph_references(&workspace_id, &node_id, &graph_read_options(options))
                        .await?,
                )?
            }
            GraphCommand::Implementations {
                workspace_id,
                node_id,
                options,
            } => {
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(
                    service
                        .graph_implementations(
                            &workspace_id,
                            &node_id,
                            &graph_read_options(options),
                        )
                        .await?,
                )?
            }
            GraphCommand::Tests {
                workspace_id,
                node_id,
                options,
            } => {
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(
                    service
                        .graph_tests(&workspace_id, &node_id, &graph_read_options(options))
                        .await?,
                )?
            }
            GraphCommand::Dependencies {
                workspace_id,
                node_id,
                options,
            } => {
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(
                    service
                        .graph_dependencies(&workspace_id, &node_id, &graph_read_options(options))
                        .await?,
                )?
            }
            GraphCommand::Dependents {
                workspace_id,
                node_id,
                options,
            } => {
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(
                    service
                        .graph_dependents(&workspace_id, &node_id, &graph_read_options(options))
                        .await?,
                )?
            }
            GraphCommand::ImpactSymbol {
                workspace_id,
                symbol,
                options,
            } => {
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(
                    service
                        .graph_impact_symbol(&workspace_id, &symbol, &graph_read_options(options))
                        .await?,
                )?
            }
            GraphCommand::ImpactPath {
                workspace_id,
                path,
                options,
            } => {
                let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
                print_json(
                    service
                        .graph_impact_path(&workspace_id, &path, &graph_read_options(options))
                        .await?,
                )?
            }
        },
        Command::Search(args) => {
            let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
            let limit = args.limit.unwrap_or(service.config().retrieval.default_k);
            let results = match args.mode {
                SearchMode::Semantic => {
                    service
                        .semantic_search(&workspace_id, &args.query, limit)
                        .await?
                }
                SearchMode::Lexical => {
                    service
                        .lexical_search(&workspace_id, &args.query, limit)
                        .await?
                }
                SearchMode::Hybrid => {
                    service
                        .hybrid_search(&workspace_id, &args.query, limit)
                        .await?
                }
            };
            print_json(results)?;
        }
        Command::Context(args) => {
            let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
            let mut request = ContextRequest::new(workspace_id);
            request.query = Some(args.query);
            request.session_id = args.session_id;
            request.task_id = args.task_id;
            request.token_budget = args.token_budget.unwrap_or(request.token_budget);
            request.include_code = !args.no_code;
            request.include_documents = !args.no_documents;
            request.include_memories = !args.no_memories;
            request.include_events = !args.no_events;
            request.path_scope = args.path_scope;
            request.language_scope = args.language_scope;
            request.active_failure_signature =
                parse_cli_failure_signature(args.active_failure_signature.as_deref())?;
            request.include_explanation = args.explain;
            print_json(service.semantic_context(request).await?)?;
        }
        Command::Resume(args) => {
            let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
            let mut request = ResumeContextRequest::new(workspace_id);
            request.session_id = args.session_id;
            request.task_id = args.task_id;
            request.token_budget = args.token_budget.unwrap_or(request.token_budget);
            request.include_explanation = args.explain;
            print_json(service.resume_context(request).await?)?;
        }
        Command::WorkingSet(args) => {
            let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
            print_json(
                service
                    .inspect_working_set(&workspace_id, &args.session_id, args.task_id.as_deref())
                    .await?,
            )?;
        }
        Command::ContextPin(args) => {
            let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
            print_json(
                service
                    .pin_context(
                        &workspace_id,
                        &args.session_id,
                        args.task_id.as_deref(),
                        &args.source_id,
                        ContextSourceType::from_storage(&args.source_type),
                    )
                    .await?,
            )?;
        }
        Command::ContextUnpin(args) => {
            let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
            print_json(
                service
                    .unpin_context(
                        &workspace_id,
                        &args.session_id,
                        args.task_id.as_deref(),
                        &args.source_id,
                        ContextSourceType::from_storage(&args.source_type),
                    )
                    .await?,
            )?;
        }
        Command::Checkpoint { command } => match command {
            CheckpointCommand::Create(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                let mut checkpoint = Checkpoint::new(workspace_id, args.session_id, args.content);
                checkpoint.task_id = args.task_id;
                checkpoint.objective = args.objective;
                checkpoint.completed = args.completed;
                checkpoint.decision_ids = args.decision_id;
                checkpoint.open_problems = args.open_problem;
                checkpoint.related_paths = args.related_path;
                checkpoint.related_symbols = args.related_symbol;
                checkpoint.next_action = args.next_action;
                print_json(service.create_checkpoint(checkpoint).await?)?;
            }
            CheckpointCommand::Latest(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                let checkpoint = match (args.session_id, args.task_id) {
                    (Some(session_id), None) => {
                        service
                            .latest_checkpoint_for_session(&workspace_id, &session_id)
                            .await?
                    }
                    (None, Some(task_id)) => {
                        service
                            .latest_checkpoint_for_task(&workspace_id, &task_id)
                            .await?
                    }
                    (None, None) => service.latest_checkpoint(&workspace_id).await?,
                    (Some(_), Some(_)) => {
                        unreachable!("clap enforces conflicting checkpoint scopes")
                    }
                };
                print_json(checkpoint)?;
            }
        },
        Command::Memory { command } => match command {
            MemoryCommand::Add(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                let metadata: Value = args
                    .metadata
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?
                    .unwrap_or_else(|| json!({}));
                let mut memory = MemoryRecord::new(workspace_id, args.kind.into(), args.content);
                memory.session_id = args.session_id;
                memory.task_id = args.task_id;
                memory.related_paths = args.related_paths;
                memory.metadata = metadata;
                print_json(service.record_memory(memory).await?)?;
            }
        },
        Command::Episode { command } => match command {
            EpisodeCommand::Start(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                print_json(
                    service
                        .start_episode(EpisodeStartRequest {
                            workspace_id,
                            session_id: args.session_id,
                            task_id: args.task_id,
                            episode_type: args.episode_type.into(),
                            title: args.title,
                            created_by: EpisodeCreator::User,
                        })
                        .await?,
                )?;
            }
            EpisodeCommand::AddEvents(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                print_json(
                    service
                        .add_episode_events(EpisodeEventAssociationRequest {
                            workspace_id,
                            episode_id: args.episode_id,
                            expected_version: args.expected_version,
                            request_key: args.request_key,
                            event_ids: args.event_ids,
                        })
                        .await?,
                )?;
            }
            EpisodeCommand::Close(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                print_json(
                    service
                        .close_episode(EpisodeTerminalRequest {
                            workspace_id,
                            episode_id: args.episode_id,
                            expected_version: args.expected_version,
                            request_key: args.request_key,
                        })
                        .await?,
                )?;
            }
            EpisodeCommand::Abandon(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                print_json(
                    service
                        .abandon_episode(EpisodeTerminalRequest {
                            workspace_id,
                            episode_id: args.episode_id,
                            expected_version: args.expected_version,
                            request_key: args.request_key,
                        })
                        .await?,
                )?;
            }
            EpisodeCommand::Show(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                print_json(service.get_episode(&workspace_id, &args.episode_id).await?)?;
            }
            EpisodeCommand::List(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                print_json(
                    service
                        .list_episodes(EpisodeListRequest {
                            workspace_id,
                            session_id: args.session_id,
                            task_id: args.task_id,
                            limit: args.limit,
                        })
                        .await?,
                )?;
            }
        },
        Command::Experience { command } => match command {
            ExperienceCommand::Preview(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                print_json(
                    service
                        .preview_experience(&ConsolidationRequest {
                            workspace_id,
                            episode_id: args.episode_id,
                            expected_episode_version: args.expected_version,
                        })
                        .await?,
                )?;
            }
            ExperienceCommand::Consolidate(args) => {
                let workspace_id = cli_workspace_id(&service, &args.episode.workspace_id).await?;
                print_json(
                    service
                        .accept_experience(&ConsolidationAcceptanceRequest {
                            request: ConsolidationRequest {
                                workspace_id,
                                episode_id: args.episode.episode_id,
                                expected_episode_version: args.episode.expected_version,
                            },
                            expected_fingerprint: args.expected_fingerprint,
                            expected_proposal_hash: args.expected_proposal_hash,
                        })
                        .await?,
                )?;
            }
            ExperienceCommand::Search(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                let signature = parse_cli_failure_signature(args.failure_signature.as_deref())?;
                print_json(
                    service
                        .search_experiences(&ExperienceSearchRequest {
                            workspace_id,
                            query: args.query,
                            exact_failure_signature: signature,
                            compatible_components: Default::default(),
                            path: None,
                            graph_stable_key: None,
                            outcomes: Vec::new(),
                            strengths: Vec::new(),
                            lifecycles: Vec::new(),
                            include_historical: args.include_historical,
                            created_after: None,
                            created_before: None,
                            limit: args.limit,
                        })
                        .await?,
                )?;
            }
            ExperienceCommand::Show(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                print_json(
                    service
                        .get_experience(&workspace_id, &args.experience_id)
                        .await?,
                )?;
            }
            ExperienceCommand::Explain(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                print_json(
                    service
                        .experience_get(&workspace_id, &args.experience_id)
                        .await?,
                )?;
            }
            ExperienceCommand::History(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                let after = cli_assessment_cursor(
                    args.after_created_at.as_deref(),
                    args.after_id.as_deref(),
                )?;
                print_json(
                    service
                        .experience_assessment_history(
                            &workspace_id,
                            &args.experience_id,
                            after.as_ref(),
                            args.limit,
                        )
                        .await?,
                )?;
            }
            ExperienceCommand::Assess(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                print_json(
                    service
                        .review_experience_assessment(ExperienceAssessmentReviewRequest {
                            workspace_id,
                            experience_id: args.experience_id,
                            kind: args.kind.into(),
                            reviewed_by: args.reviewed_by,
                            request_key: args.request_key,
                            reason: args.reason,
                            replacement_experience_id: args.replacement_experience_id,
                            evidence_event_ids: args.evidence_event_ids,
                        })
                        .await?,
                )?;
            }
            ExperienceCommand::ProposeDispute(args) => {
                let workspace_id = cli_workspace_id(&service, &args.workspace_id).await?;
                print_json(
                    service
                        .propose_experience_disputes(&ExperienceDisputeProposalRequest {
                            workspace_id,
                            failure_signature: parse_required_cli_failure_signature(
                                &args.failure_signature,
                            )?,
                            recurring_failure_event_ids: args.recurring_failure_event_ids,
                        })
                        .await?,
                )?;
            }
        },
        Command::Reindex { workspace_id } => {
            let workspace_id = cli_workspace_id(&service, &workspace_id).await?;
            print_json(service.workspace_reindex(&workspace_id).await?)?
        }
    }
    Ok(())
}

fn cli_assessment_cursor(
    created_at: Option<&str>,
    id: Option<&str>,
) -> Result<Option<cortexweave::domain::ExperienceAssessmentCursor>> {
    match (created_at, id) {
        (None, None) => Ok(None),
        (Some(created_at), Some(id)) if !id.trim().is_empty() => {
            let created_at = DateTime::parse_from_rfc3339(created_at)
                .map_err(|error| {
                    CortexError::Analysis(format!("invalid --after-created-at: {error}"))
                })?
                .with_timezone(&Utc);
            Ok(Some(cortexweave::domain::ExperienceAssessmentCursor {
                created_at,
                id: id.to_owned(),
            }))
        }
        _ => Err(CortexError::Analysis(
            "--after-created-at and --after-id must be supplied together".into(),
        )),
    }
}

fn parse_cli_failure_signature(value: Option<&str>) -> Result<Option<FailureSignature>> {
    value.map(parse_required_cli_failure_signature).transpose()
}

fn parse_required_cli_failure_signature(value: &str) -> Result<FailureSignature> {
    serde_json::from_str(value)
        .map_err(|error| CortexError::Analysis(format!("failure_signature must be JSON: {error}")))
}

fn parse_cli_episode_limit(value: &str) -> std::result::Result<usize, String> {
    let value = value
        .parse::<usize>()
        .map_err(|_| "limit must be a non-negative integer".to_owned())?;
    (value <= 100)
        .then_some(value)
        .ok_or_else(|| "limit must not exceed 100".to_owned())
}

fn parse_cli_experience_limit(value: &str) -> std::result::Result<usize, String> {
    let value = value
        .parse::<usize>()
        .map_err(|_| "limit must be an integer".to_owned())?;
    (1..=50)
        .contains(&value)
        .then_some(value)
        .ok_or_else(|| "limit must be between 1 and 50".to_owned())
}

async fn cli_workspace_id(service: &CortexWeaveService, selector: &str) -> Result<String> {
    Ok(service
        .resolve_workspace(parse_workspace_selector(selector), None)
        .await?
        .id)
}

fn parse_workspace_selector(value: &str) -> WorkspaceSelector {
    if value
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
    {
        WorkspaceSelector::FileUri(value.to_owned())
    } else if uuid::Uuid::parse_str(value).is_ok() {
        WorkspaceSelector::Id(value.to_owned())
    } else if looks_like_absolute_path(value) {
        WorkspaceSelector::RootPath(PathBuf::from(value))
    } else {
        WorkspaceSelector::Name(value.to_owned())
    }
}

fn looks_like_absolute_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    PathBuf::from(value).is_absolute()
        || value.starts_with('/')
        || value.starts_with(r"\\")
        || (bytes.get(1) == Some(&b':')
            && bytes.first().is_some_and(u8::is_ascii_alphabetic)
            && matches!(bytes.get(2), Some(b'/' | b'\\')))
}

fn serve_workspace_hint(argument: Option<PathBuf>) -> Option<WorkspaceHint> {
    argument
        .or_else(|| std::env::var_os("CORTEXWEAVE_WORKSPACE_ROOT").map(PathBuf::from))
        .filter(|path| !path.as_os_str().is_empty())
        .map(WorkspaceHint::RootPath)
}

fn graph_read_options(args: GraphReadArgs) -> StructuralReadOptions {
    let defaults = StructuralReadOptions::default();
    StructuralReadOptions {
        allow_stale: args.allow_stale,
        max_nodes: args.max_nodes.unwrap_or(defaults.max_nodes),
        max_edges: args.max_edges.unwrap_or(defaults.max_edges),
        max_depth: args.max_depth.unwrap_or(defaults.max_depth),
    }
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
    let experience_persistence = check(
        service.storage().experience_health_check().await,
        "episode, Experience, assessment, and Experience FTS persistence are healthy",
    );
    let (workspace_inventory, workspaces) = match service.list_workspaces().await {
        Ok(workspaces) => (
            Check {
                ok: true,
                detail: format!("loaded {} registered workspaces", workspaces.len()),
            },
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
                .collect::<Vec<_>>(),
        ),
        Err(error) => (
            Check {
                ok: false,
                detail: format!("workspace inventory is unavailable: {error}"),
            },
            Vec::new(),
        ),
    };
    let configured_workspace_hint = std::env::var_os("CORTEXWEAVE_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty());
    let workspace_resolution = if !workspace_inventory.ok {
        Check {
            ok: false,
            detail: "workspace resolution was not evaluated because inventory failed".into(),
        }
    } else {
        match configured_workspace_hint {
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
                    detail: "no default root is configured; MCP calls need an explicit selector"
                        .into(),
                },
            },
        }
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
        ok: workspace_inventory.ok
            && workspaces
                .iter()
                .all(|workspace| workspace.root_is_directory),
        detail: if workspace_inventory.ok {
            "all registered workspace roots are available for watcher startup".into()
        } else {
            "watcher health was not evaluated because workspace inventory failed".into()
        },
    };
    let mut graph_workspaces = Vec::with_capacity(workspaces.len());
    for workspace in &workspaces {
        match service.workspace_graph_status(&workspace.id).await {
            Ok(status) => graph_workspaces.push(GraphWorkspaceCheck {
                id: workspace.id.clone(),
                status: Some(status),
                error: None,
            }),
            Err(error) => graph_workspaces.push(GraphWorkspaceCheck {
                id: workspace.id.clone(),
                status: None,
                error: Some(error.to_string()),
            }),
        }
    }
    let graph = Check {
        ok: workspace_inventory.ok
            && graph_workspaces.iter().all(|workspace| {
                workspace.error.is_none()
                    && workspace
                        .status
                        .as_ref()
                        .is_some_and(|status| status.is_current)
            }),
        detail: if !workspace_inventory.ok {
            "graph health was not evaluated because workspace inventory failed".into()
        } else if graph_workspaces.is_empty() {
            "no workspaces are registered".into()
        } else {
            let current = graph_workspaces
                .iter()
                .filter(|workspace| {
                    workspace
                        .status
                        .as_ref()
                        .is_some_and(|status| status.is_current)
                })
                .count();
            format!(
                "{current}/{} workspace graph projections are current",
                graph_workspaces.len()
            )
        },
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
        experience_persistence,
        workspace_inventory,
        workspace_resolution,
        registered_analyzers,
        grammar_initialization,
        watcher,
        graph,
        graph_workspaces,
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
