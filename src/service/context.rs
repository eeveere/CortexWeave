use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Utc};

use crate::{
    CortexError, Result,
    config::{ContextConfig, TemporalConfig, WorkingSetConfig},
    domain::{
        Checkpoint, ContextCandidate, ContextCandidatePool, ContextFreshness, ContextPin,
        ContextRequest, ContextScores, ContextSelectionReason, ContextSourceType, TemporalBounds,
        TemporalContextItem, TemporalQuery, TemporalSessionScope, WorkingSetEntry,
        WorkingSetSnapshot,
    },
    retrieval::{RetrievalResult, RetrievalService},
    storage::{SqliteStorage, TemporalCandidate},
};

trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub struct ContextService {
    storage: Arc<SqliteStorage>,
    retrieval: Arc<RetrievalService>,
    config: WorkingSetConfig,
    temporal: TemporalConfig,
    context: ContextConfig,
    clock: Arc<dyn Clock>,
}

impl ContextService {
    pub fn new(
        storage: Arc<SqliteStorage>,
        retrieval: Arc<RetrievalService>,
        config: WorkingSetConfig,
        temporal: TemporalConfig,
        context: ContextConfig,
    ) -> Result<Self> {
        Self::with_clock(
            storage,
            retrieval,
            config,
            temporal,
            context,
            Arc::new(SystemClock),
        )
    }

    fn with_clock(
        storage: Arc<SqliteStorage>,
        retrieval: Arc<RetrievalService>,
        config: WorkingSetConfig,
        temporal: TemporalConfig,
        context: ContextConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        config.validate()?;
        temporal.validate()?;
        context.validate()?;
        Ok(Self {
            storage,
            retrieval,
            config,
            temporal,
            context,
            clock,
        })
    }

    pub async fn activate_source(
        &self,
        workspace_id: &str,
        session_id: &str,
        task_id: Option<&str>,
        source_id: &str,
        source_type: ContextSourceType,
    ) -> Result<WorkingSetEntry> {
        self.ensure_enabled()?;
        self.validate_scope(workspace_id, session_id, task_id, true)
            .await?;
        self.validate_source(workspace_id, source_id, &source_type)
            .await?;

        let now = self.clock.now();
        let mut candidate =
            WorkingSetEntry::new(workspace_id, session_id, source_id, source_type, 0.0);
        candidate.task_id = task_id.map(ToOwned::to_owned);
        candidate.created_at = now;
        candidate.last_activated_at = now;
        let half_life = self.config.decay_half_life_minutes;
        let increment = self.config.activation_increment;
        let maximum = self.config.max_activation_score;
        let entry = self
            .storage
            .mutate_working_set_entry(candidate, |existing| {
                let current = existing.map_or(0.0, |entry| {
                    decayed_score(
                        entry.activation_score,
                        &entry.last_activated_at,
                        &now,
                        half_life,
                    )
                });
                (current + increment).min(maximum)
            })
            .await?;
        self.prune_session(session_id).await?;
        Ok(entry)
    }

    pub async fn inspect_working_set(
        &self,
        workspace_id: &str,
        session_id: &str,
        task_id: Option<&str>,
    ) -> Result<WorkingSetSnapshot> {
        self.ensure_enabled()?;
        self.validate_scope(workspace_id, session_id, task_id, false)
            .await?;
        let (mut entries, mut pins, generated_at) = self.prune_session(session_id).await?;

        if let Some(task_id) = task_id {
            entries.retain(|entry| entry.task_id.as_deref().is_none_or(|id| id == task_id));
            pins.retain(|pin| pin.task_id.as_deref().is_none_or(|id| id == task_id));
        }

        Ok(WorkingSetSnapshot {
            workspace_id: workspace_id.to_owned(),
            session_id: session_id.to_owned(),
            task_id: task_id.map(ToOwned::to_owned),
            entries,
            pins,
            generated_at,
        })
    }

    pub async fn pin_context(
        &self,
        workspace_id: &str,
        session_id: &str,
        task_id: Option<&str>,
        source_id: &str,
        source_type: ContextSourceType,
    ) -> Result<ContextPin> {
        self.ensure_enabled()?;
        self.validate_scope(workspace_id, session_id, task_id, true)
            .await?;
        self.validate_source(workspace_id, source_id, &source_type)
            .await?;

        if let Some(pin) = self
            .storage
            .context_pin(session_id, task_id, source_id, &source_type)
            .await?
        {
            return Ok(pin);
        }
        if self.storage.context_pins(session_id).await?.len() >= self.config.max_items {
            return Err(CortexError::Analysis(format!(
                "session {session_id} has reached the working-set pin limit of {}",
                self.config.max_items
            )));
        }

        let mut pin = ContextPin::new(workspace_id, session_id, source_id, source_type.clone());
        pin.task_id = task_id.map(ToOwned::to_owned);
        pin.created_at = self.clock.now();
        self.storage.insert_context_pin(&pin).await?;
        self.storage
            .context_pin(session_id, task_id, source_id, &source_type)
            .await?
            .ok_or_else(|| {
                CortexError::Analysis("context pin insert did not produce a readable row".into())
            })
    }

    pub async fn unpin_context(
        &self,
        workspace_id: &str,
        session_id: &str,
        task_id: Option<&str>,
        source_id: &str,
        source_type: ContextSourceType,
    ) -> Result<bool> {
        self.ensure_enabled()?;
        self.validate_scope(workspace_id, session_id, task_id, false)
            .await?;
        self.storage
            .delete_context_pin_for_source(session_id, task_id, source_id, &source_type)
            .await
    }

    pub async fn create_checkpoint(&self, mut checkpoint: Checkpoint) -> Result<Checkpoint> {
        validate_checkpoint_fields(&checkpoint)?;
        self.validate_scope(
            &checkpoint.workspace_id,
            &checkpoint.session_id,
            checkpoint.task_id.as_deref(),
            true,
        )
        .await?;
        for decision_id in &checkpoint.decision_ids {
            if !self
                .storage
                .decision_memory_exists(&checkpoint.workspace_id, decision_id)
                .await?
            {
                return Err(CortexError::NotFound(format!(
                    "decision memory {decision_id} in workspace {}",
                    checkpoint.workspace_id
                )));
            }
        }

        checkpoint.created_at = self.clock.now();
        self.storage.insert_checkpoint(&checkpoint).await?;
        Ok(checkpoint)
    }

    pub async fn latest_checkpoint(&self, workspace_id: &str) -> Result<Option<Checkpoint>> {
        self.validate_workspace(workspace_id).await?;
        self.storage.latest_checkpoint(workspace_id).await
    }

    pub async fn latest_checkpoint_for_session(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<Option<Checkpoint>> {
        self.validate_scope(workspace_id, session_id, None, false)
            .await?;
        self.storage
            .latest_checkpoint_for_session(workspace_id, session_id)
            .await
    }

    pub async fn latest_checkpoint_for_task(
        &self,
        workspace_id: &str,
        task_id: &str,
    ) -> Result<Option<Checkpoint>> {
        self.validate_workspace(workspace_id).await?;
        let task = self
            .storage
            .get_task(task_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("task {task_id}")))?;
        if task.workspace_id != workspace_id {
            return Err(CortexError::Analysis(
                "checkpoint task belongs to a different workspace".into(),
            ));
        }
        self.storage
            .latest_checkpoint_for_task(workspace_id, task_id)
            .await
    }

    pub async fn temporal_retrieval(
        &self,
        query: TemporalQuery,
    ) -> Result<Vec<TemporalContextItem>> {
        const MAX_TEMPORAL_LIMIT: usize = 100;

        if query.limit == 0 {
            return Ok(Vec::new());
        }
        self.storage
            .get_workspace(&query.workspace_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("workspace {}", query.workspace_id)))?;
        let now = self.clock.now();
        let (bounds, _) = self.resolve_temporal_bounds(&query, now).await?;
        let candidates = self
            .storage
            .temporal_candidates(
                &query.workspace_id,
                &bounds,
                &query.source_types,
                query.limit.min(MAX_TEMPORAL_LIMIT),
            )
            .await?;
        let mut items: Vec<_> = candidates
            .into_iter()
            .map(|candidate| self.temporal_item(candidate, now))
            .collect();
        items.sort_by(|left, right| {
            freshness_priority(right.freshness)
                .cmp(&freshness_priority(left.freshness))
                .then_with(|| right.recency_score.total_cmp(&left.recency_score))
                .then_with(|| {
                    left.source_type
                        .storage_name()
                        .cmp(&right.source_type.storage_name())
                })
                .then_with(|| left.source_id.cmp(&right.source_id))
        });
        items.truncate(query.limit.min(MAX_TEMPORAL_LIMIT));
        Ok(items)
    }

    pub async fn build_candidate_pool(
        &self,
        request: ContextRequest,
    ) -> Result<ContextCandidatePool> {
        self.validate_candidate_scope(&request).await?;
        let generated_at = self.clock.now();
        let limit = self.context.candidate_pool_limit;
        let mut source_types = vec![
            ContextSourceType::TaskState,
            ContextSourceType::SessionState,
        ];
        if request.include_code {
            source_types.push(ContextSourceType::Code);
        }
        if request.include_documents {
            source_types.push(ContextSourceType::Document);
        }
        if request.include_memories {
            source_types.push(ContextSourceType::Memory);
        }
        if request.include_events {
            source_types.push(ContextSourceType::Event);
        }

        let mut temporal_query = TemporalQuery::new(&request.workspace_id);
        temporal_query.source_types = source_types;
        temporal_query.limit = limit;
        let temporal = self.temporal_retrieval(temporal_query).await?;
        let direct_code = match request
            .query
            .as_deref()
            .filter(|query| !query.trim().is_empty())
        {
            Some(query) if request.include_code => {
                self.retrieval
                    .hybrid_search(&request.workspace_id, query, limit)
                    .await?
            }
            _ => Vec::new(),
        };

        let mut candidates = BTreeMap::new();
        for item in temporal {
            insert_candidate(&mut candidates, candidate_from_temporal(item));
        }
        for result in direct_code {
            insert_candidate(&mut candidates, candidate_from_retrieval(result));
        }

        if let Some(session_id) = request.session_id.as_deref()
            && self.config.enabled
        {
            let working_set = self
                .inspect_working_set(
                    &request.workspace_id,
                    session_id,
                    request.task_id.as_deref(),
                )
                .await?;
            for entry in working_set.entries {
                if let Some(source) = self
                    .storage
                    .context_source_candidate(
                        &request.workspace_id,
                        &entry.source_id,
                        &entry.source_type,
                    )
                    .await?
                {
                    let mut candidate =
                        candidate_from_temporal(self.temporal_item(source, generated_at));
                    candidate.scores.working_set = entry.activation_score;
                    candidate
                        .reasons
                        .push(ContextSelectionReason::ActiveWorkingSet);
                    insert_candidate(&mut candidates, candidate);
                }
            }
        }

        if let Some(task_id) = request.task_id.as_deref()
            && let Some(source) = self
                .storage
                .context_source_candidate(
                    &request.workspace_id,
                    task_id,
                    &ContextSourceType::TaskState,
                )
                .await?
        {
            let mut candidate = candidate_from_temporal(self.temporal_item(source, generated_at));
            candidate.scores.task = 1.0;
            candidate
                .reasons
                .push(ContextSelectionReason::ActiveTaskReference);
            insert_candidate(&mut candidates, candidate);
        }
        if let Some(session_id) = request.session_id.as_deref()
            && let Some(source) = self
                .storage
                .context_source_candidate(
                    &request.workspace_id,
                    session_id,
                    &ContextSourceType::SessionState,
                )
                .await?
        {
            let mut candidate = candidate_from_temporal(self.temporal_item(source, generated_at));
            candidate.reasons.push(ContextSelectionReason::ResumeState);
            insert_candidate(&mut candidates, candidate);
        }

        let mut candidates: Vec<_> = candidates
            .into_values()
            .filter(|candidate| candidate_matches_scope(candidate, &request))
            .collect();
        candidates.sort_by(candidate_order);
        candidates.truncate(limit);
        Ok(ContextCandidatePool {
            workspace_id: request.workspace_id,
            session_id: request.session_id,
            task_id: request.task_id,
            candidates,
            generated_at,
        })
    }

    async fn resolve_temporal_bounds(
        &self,
        query: &TemporalQuery,
        now: DateTime<Utc>,
    ) -> Result<(TemporalBounds, Option<crate::domain::Session>)> {
        let filter = &query.filter;
        if invalid_temporal_range(filter.created_after, filter.created_before)
            || invalid_temporal_range(filter.modified_after, filter.modified_before)
        {
            return Err(CortexError::Analysis(
                "temporal after boundary must not be later than before boundary".into(),
            ));
        }
        if filter
            .recent_within
            .is_some_and(|window| window.hours() == 0)
        {
            return Err(CortexError::Analysis(
                "temporal recent window must be greater than zero".into(),
            ));
        }
        let mut bounds = TemporalBounds {
            created_after: filter.created_after,
            created_before: filter.created_before,
            modified_after: filter.modified_after,
            modified_before: filter.modified_before,
            activity_after: filter
                .recent_within
                .map(|window| {
                    let hours = i64::try_from(window.hours()).map_err(|_| {
                        CortexError::Analysis("temporal recent window is too large".into())
                    })?;
                    let duration = chrono::Duration::try_hours(hours).ok_or_else(|| {
                        CortexError::Analysis("temporal recent window is too large".into())
                    })?;
                    now.checked_sub_signed(duration).ok_or_else(|| {
                        CortexError::Analysis("temporal recent window is too large".into())
                    })
                })
                .transpose()?,
            activity_before: None,
            scoped_session_id: None,
            include_superseded: filter.include_superseded,
        };
        let scoped_session = match filter.session_scope {
            TemporalSessionScope::Any => None,
            TemporalSessionScope::Current | TemporalSessionScope::Previous => {
                let session_id = query.session_id.as_deref().ok_or_else(|| {
                    CortexError::Analysis(
                        "current or previous session scope requires session_id".into(),
                    )
                })?;
                let current = self
                    .storage
                    .get_session(session_id)
                    .await?
                    .ok_or_else(|| CortexError::NotFound(format!("session {session_id}")))?;
                if current.workspace_id != query.workspace_id {
                    return Err(CortexError::Analysis(
                        "temporal session belongs to a different workspace".into(),
                    ));
                }
                Some(
                    if matches!(filter.session_scope, TemporalSessionScope::Current) {
                        current
                    } else {
                        self.storage
                            .previous_session(&query.workspace_id, current.started_at)
                            .await?
                            .ok_or_else(|| {
                                CortexError::NotFound(
                                    "no previous session exists in this workspace".into(),
                                )
                            })?
                    },
                )
            }
        };
        if let Some(session) = scoped_session.as_ref() {
            bounds.activity_after = max_time(bounds.activity_after, Some(session.started_at));
            bounds.activity_before = Some(session.ended_at.unwrap_or(now));
            bounds.scoped_session_id = Some(session.id.clone());
        }
        Ok((bounds, scoped_session))
    }

    fn temporal_item(
        &self,
        candidate: TemporalCandidate,
        now: DateTime<Utc>,
    ) -> TemporalContextItem {
        let activity_at = candidate.modified_at.unwrap_or(candidate.created_at);
        let age_hours = now
            .signed_duration_since(activity_at)
            .num_milliseconds()
            .max(0) as f64
            / 3_600_000.0;
        let recency_score = 0.5_f64.powf(age_hours / self.temporal.recency_half_life_hours) as f32;
        let freshness = if candidate.superseded {
            ContextFreshness::Superseded
        } else {
            match candidate.source_type {
                ContextSourceType::Code
                | ContextSourceType::Document
                | ContextSourceType::TaskState => ContextFreshness::Current,
                ContextSourceType::Memory
                | ContextSourceType::Event
                | ContextSourceType::SessionState
                | ContextSourceType::Other(_) => ContextFreshness::Historical,
            }
        };
        TemporalContextItem {
            source_id: candidate.source_id,
            source_type: candidate.source_type,
            session_id: candidate.session_id,
            task_id: candidate.task_id,
            content: candidate.content,
            path: candidate.path,
            symbol: candidate.symbol,
            language: candidate.language,
            created_at: candidate.created_at,
            modified_at: candidate.modified_at,
            freshness,
            recency_score,
        }
    }

    async fn prune_session(
        &self,
        session_id: &str,
    ) -> Result<(Vec<WorkingSetEntry>, Vec<ContextPin>, DateTime<Utc>)> {
        let generated_at = self.clock.now();
        let mut entries = self.storage.working_set_entries(session_id).await?;
        let pins = self.storage.context_pins(session_id).await?;
        let pin_keys: HashSet<_> = pins
            .iter()
            .map(|pin| {
                (
                    pin.task_id.clone(),
                    pin.source_type.storage_name(),
                    pin.source_id.clone(),
                )
            })
            .collect();

        for entry in &mut entries {
            entry.activation_score = decayed_score(
                entry.activation_score,
                &entry.last_activated_at,
                &generated_at,
                self.config.decay_half_life_minutes,
            );
        }
        entries.sort_by(|left, right| {
            let left_pinned = is_pinned(left, &pin_keys);
            let right_pinned = is_pinned(right, &pin_keys);
            right_pinned
                .cmp(&left_pinned)
                .then_with(|| right.activation_score.total_cmp(&left.activation_score))
                .then_with(|| right.last_activated_at.cmp(&left.last_activated_at))
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut remove_ids = Vec::new();
        entries.retain(|entry| {
            let retain = is_pinned(entry, &pin_keys)
                || entry.activation_score >= self.config.min_activation_score;
            if !retain {
                remove_ids.push(entry.id.clone());
            }
            retain
        });
        if entries.len() > self.config.max_items {
            remove_ids.extend(entries.drain(self.config.max_items..).map(|entry| entry.id));
        }
        self.storage.delete_working_set_entries(&remove_ids).await?;
        Ok((entries, pins, generated_at))
    }

    async fn validate_scope(
        &self,
        workspace_id: &str,
        session_id: &str,
        task_id: Option<&str>,
        require_active_session: bool,
    ) -> Result<()> {
        self.validate_workspace(workspace_id).await?;
        let session = self
            .storage
            .get_session(session_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("session {session_id}")))?;
        if session.workspace_id != workspace_id {
            return Err(CortexError::Analysis(
                "working-set session belongs to a different workspace".into(),
            ));
        }
        if require_active_session && session.ended_at.is_some() {
            return Err(CortexError::Analysis(
                "cannot mutate the working set of an ended session".into(),
            ));
        }
        if let Some(task_id) = task_id {
            let task = self
                .storage
                .get_task(task_id)
                .await?
                .ok_or_else(|| CortexError::NotFound(format!("task {task_id}")))?;
            if task.workspace_id != workspace_id || task.session_id.as_deref() != Some(session_id) {
                return Err(CortexError::Analysis(
                    "working-set task does not belong to the selected workspace and session".into(),
                ));
            }
        }
        Ok(())
    }

    async fn validate_candidate_scope(&self, request: &ContextRequest) -> Result<()> {
        match (request.session_id.as_deref(), request.task_id.as_deref()) {
            (Some(session_id), task_id) => {
                self.validate_scope(&request.workspace_id, session_id, task_id, false)
                    .await
            }
            (None, Some(task_id)) => {
                self.validate_workspace(&request.workspace_id).await?;
                let task = self
                    .storage
                    .get_task(task_id)
                    .await?
                    .ok_or_else(|| CortexError::NotFound(format!("task {task_id}")))?;
                if task.workspace_id != request.workspace_id {
                    return Err(CortexError::Analysis(
                        "candidate-pool task belongs to a different workspace".into(),
                    ));
                }
                Ok(())
            }
            (None, None) => self.validate_workspace(&request.workspace_id).await,
        }
    }

    async fn validate_workspace(&self, workspace_id: &str) -> Result<()> {
        self.storage
            .get_workspace(workspace_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("workspace {workspace_id}")))?;
        Ok(())
    }

    async fn validate_source(
        &self,
        workspace_id: &str,
        source_id: &str,
        source_type: &ContextSourceType,
    ) -> Result<()> {
        if source_id.trim().is_empty() {
            return Err(CortexError::Analysis(
                "working-set source ID cannot be empty".into(),
            ));
        }
        if !self
            .storage
            .context_source_exists(workspace_id, source_id, source_type)
            .await?
        {
            return Err(CortexError::NotFound(format!(
                "{} context source {source_id} in workspace {workspace_id}",
                source_type.storage_name()
            )));
        }
        Ok(())
    }

    fn ensure_enabled(&self) -> Result<()> {
        if self.config.enabled {
            Ok(())
        } else {
            Err(CortexError::Configuration(
                "working-set support is disabled".into(),
            ))
        }
    }
}

fn decayed_score(
    activation_score: f32,
    last_activated_at: &DateTime<Utc>,
    now: &DateTime<Utc>,
    half_life_minutes: f64,
) -> f32 {
    let elapsed_ms = now
        .signed_duration_since(last_activated_at)
        .num_milliseconds()
        .max(0) as f64;
    let elapsed_minutes = elapsed_ms / 60_000.0;
    (f64::from(activation_score) * 0.5_f64.powf(elapsed_minutes / half_life_minutes)) as f32
}

fn is_pinned(
    entry: &WorkingSetEntry,
    pin_keys: &HashSet<(Option<String>, String, String)>,
) -> bool {
    let source_type = entry.source_type.storage_name();
    pin_keys.contains(&(
        entry.task_id.clone(),
        source_type.clone(),
        entry.source_id.clone(),
    )) || pin_keys.contains(&(None, source_type, entry.source_id.clone()))
}

fn max_time(left: Option<DateTime<Utc>>, right: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (value @ Some(_), None) | (None, value @ Some(_)) => value,
        (None, None) => None,
    }
}

fn invalid_temporal_range(after: Option<DateTime<Utc>>, before: Option<DateTime<Utc>>) -> bool {
    matches!((after, before), (Some(after), Some(before)) if after > before)
}

fn validate_checkpoint_fields(checkpoint: &Checkpoint) -> Result<()> {
    if checkpoint.content.trim().is_empty() {
        return Err(CortexError::Analysis(
            "checkpoint content cannot be empty".into(),
        ));
    }
    let mut decision_ids = HashSet::new();
    for decision_id in &checkpoint.decision_ids {
        if decision_id.trim().is_empty() {
            return Err(CortexError::Analysis(
                "checkpoint decision IDs cannot be empty".into(),
            ));
        }
        if !decision_ids.insert(decision_id) {
            return Err(CortexError::Analysis(
                "checkpoint decision IDs must be unique".into(),
            ));
        }
    }
    Ok(())
}

fn candidate_from_temporal(item: TemporalContextItem) -> ContextCandidate {
    ContextCandidate {
        source_id: item.source_id,
        source_type: item.source_type,
        content: item.content,
        path: item.path,
        symbol: item.symbol,
        language: item.language,
        freshness: item.freshness,
        scores: ContextScores {
            recency: item.recency_score,
            provenance: provenance_score(item.freshness),
            ..ContextScores::default()
        },
        reasons: Vec::new(),
    }
}

fn candidate_from_retrieval(result: RetrievalResult) -> ContextCandidate {
    let mut reasons = Vec::new();
    if result.scores.semantic.is_some() {
        reasons.push(ContextSelectionReason::DirectSemanticMatch);
    }
    if result.scores.lexical.is_some() {
        reasons.push(ContextSelectionReason::DirectLexicalMatch);
    }
    ContextCandidate {
        source_id: result.chunk_id,
        source_type: ContextSourceType::Code,
        content: result.content,
        path: Some(result.path),
        symbol: result.qualified_symbol.or(result.symbol),
        language: Some(result.language),
        freshness: ContextFreshness::Current,
        scores: ContextScores {
            semantic: result.scores.semantic,
            lexical: result.scores.lexical,
            provenance: provenance_score(ContextFreshness::Current),
            ..ContextScores::default()
        },
        reasons,
    }
}

fn insert_candidate(
    candidates: &mut BTreeMap<(String, String), ContextCandidate>,
    candidate: ContextCandidate,
) {
    let key = (
        candidate.source_type.storage_name(),
        candidate.source_id.clone(),
    );
    match candidates.get_mut(&key) {
        Some(existing) => merge_candidate(existing, candidate),
        None => {
            candidates.insert(key, candidate);
        }
    }
}

fn merge_candidate(existing: &mut ContextCandidate, incoming: ContextCandidate) {
    existing.freshness =
        if freshness_priority(incoming.freshness) > freshness_priority(existing.freshness) {
            incoming.freshness
        } else {
            existing.freshness
        };
    existing.scores.semantic =
        max_optional_score(existing.scores.semantic, incoming.scores.semantic);
    existing.scores.lexical = max_optional_score(existing.scores.lexical, incoming.scores.lexical);
    existing.scores.recency = existing.scores.recency.max(incoming.scores.recency);
    existing.scores.working_set = existing.scores.working_set.max(incoming.scores.working_set);
    existing.scores.task = existing.scores.task.max(incoming.scores.task);
    existing.scores.provenance = existing.scores.provenance.max(incoming.scores.provenance);
    if incoming.scores.semantic.is_some() || incoming.scores.lexical.is_some() {
        existing.content = incoming.content;
        existing.path = incoming.path;
        existing.symbol = incoming.symbol;
        existing.language = incoming.language;
    }
    for reason in incoming.reasons {
        if !existing.reasons.contains(&reason) {
            existing.reasons.push(reason);
        }
    }
}

fn max_optional_score(left: Option<f32>, right: Option<f32>) -> Option<f32> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(score), None) | (None, Some(score)) => Some(score),
        (None, None) => None,
    }
}

fn candidate_matches_scope(candidate: &ContextCandidate, request: &ContextRequest) -> bool {
    let path_matches = request.path_scope.is_empty()
        || candidate.path.as_deref().is_some_and(|path| {
            request.path_scope.iter().any(|scope| {
                path == scope
                    || path
                        .strip_prefix(scope)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
        });
    let language_matches = request.language_scope.is_empty()
        || candidate.language.as_deref().is_some_and(|language| {
            request
                .language_scope
                .iter()
                .any(|scope| language.eq_ignore_ascii_case(scope))
        });
    path_matches && language_matches
}

fn candidate_order(left: &ContextCandidate, right: &ContextCandidate) -> std::cmp::Ordering {
    right
        .scores
        .working_set
        .total_cmp(&left.scores.working_set)
        .then_with(|| right.scores.task.total_cmp(&left.scores.task))
        .then_with(|| {
            right
                .scores
                .semantic
                .unwrap_or_default()
                .total_cmp(&left.scores.semantic.unwrap_or_default())
        })
        .then_with(|| {
            right
                .scores
                .lexical
                .unwrap_or_default()
                .total_cmp(&left.scores.lexical.unwrap_or_default())
        })
        .then_with(|| freshness_priority(right.freshness).cmp(&freshness_priority(left.freshness)))
        .then_with(|| right.scores.recency.total_cmp(&left.scores.recency))
        .then_with(|| {
            left.source_type
                .storage_name()
                .cmp(&right.source_type.storage_name())
        })
        .then_with(|| left.source_id.cmp(&right.source_id))
}

fn provenance_score(freshness: ContextFreshness) -> f32 {
    match freshness {
        ContextFreshness::Current => 1.0,
        ContextFreshness::Unknown => 0.5,
        ContextFreshness::Historical => 0.25,
        ContextFreshness::Superseded => 0.0,
    }
}

fn freshness_priority(freshness: ContextFreshness) -> u8 {
    match freshness {
        ContextFreshness::Current => 3,
        ContextFreshness::Unknown => 2,
        ContextFreshness::Historical => 1,
        ContextFreshness::Superseded => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::{Duration, TimeZone};

    use super::*;
    use crate::domain::{
        Checkpoint, CortexEvent, Document, EventType, MemoryKind, MemoryRecord, Session,
        StoredChunk, Task, TemporalFilter, TemporalQuery, Workspace,
    };
    use crate::embedding::provider::MockEmbeddingProvider;

    fn test_retrieval(storage: Arc<SqliteStorage>) -> Arc<RetrievalService> {
        Arc::new(
            RetrievalService::new(
                storage,
                Arc::new(MockEmbeddingProvider::new("context", 4)),
                0.7,
                0.3,
            )
            .unwrap(),
        )
    }

    struct TestClock {
        now: Mutex<DateTime<Utc>>,
    }

    impl TestClock {
        fn new(now: DateTime<Utc>) -> Self {
            Self {
                now: Mutex::new(now),
            }
        }

        fn advance(&self, duration: Duration) {
            let mut now = self.now.lock().unwrap();
            *now += duration;
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> DateTime<Utc> {
            *self.now.lock().unwrap()
        }
    }

    async fn fixture(
        max_items: usize,
    ) -> (
        ContextService,
        Arc<TestClock>,
        Arc<SqliteStorage>,
        Workspace,
        Session,
        Vec<StoredChunk>,
    ) {
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        let workspace = Workspace::new("C:/working-set", "working-set");
        storage.insert_workspace(&workspace).await.unwrap();
        let session = Session::new(&workspace.id, serde_json::json!({}));
        storage.insert_session(&session).await.unwrap();
        let document = Document::new(&workspace.id, "src/lib.rs");
        storage.insert_document(&document).await.unwrap();
        let chunks: Vec<_> = ["a", "b", "c"]
            .into_iter()
            .map(|name| StoredChunk::new(&document.id, name, format!("fn {name}() {{}}")))
            .collect();
        for chunk in &chunks {
            storage.insert_chunk(chunk).await.unwrap();
        }

        let clock = Arc::new(TestClock::new(
            Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap(),
        ));
        let clock_handle: Arc<dyn Clock> = clock.clone();
        let config = WorkingSetConfig {
            decay_half_life_minutes: 60.0,
            activation_increment: 0.4,
            min_activation_score: 0.01,
            max_items,
            ..WorkingSetConfig::default()
        };
        let service = ContextService::with_clock(
            Arc::clone(&storage),
            test_retrieval(Arc::clone(&storage)),
            config,
            TemporalConfig::default(),
            ContextConfig::default(),
            clock_handle,
        )
        .unwrap();
        (service, clock, storage, workspace, session, chunks)
    }

    #[tokio::test]
    async fn repeated_activation_rises_and_time_decay_is_deterministic() {
        let (service, clock, _storage, workspace, session, chunks) = fixture(10).await;

        let first = service
            .activate_source(
                &workspace.id,
                &session.id,
                None,
                &chunks[0].id,
                ContextSourceType::Code,
            )
            .await
            .unwrap();
        let second = service
            .activate_source(
                &workspace.id,
                &session.id,
                None,
                &chunks[0].id,
                ContextSourceType::Code,
            )
            .await
            .unwrap();
        assert!((first.activation_score - 0.4).abs() < f32::EPSILON);
        assert!((second.activation_score - 0.8).abs() < f32::EPSILON);

        clock.advance(Duration::minutes(60));
        let snapshot = service
            .inspect_working_set(&workspace.id, &session.id, None)
            .await
            .unwrap();
        assert_eq!(snapshot.entries.len(), 1);
        assert!((snapshot.entries[0].activation_score - 0.4).abs() < 0.0001);

        let reactivated = service
            .activate_source(
                &workspace.id,
                &session.id,
                None,
                &chunks[0].id,
                ContextSourceType::Code,
            )
            .await
            .unwrap();
        assert!((reactivated.activation_score - 0.8).abs() < 0.0001);
    }

    #[tokio::test]
    async fn bounded_working_set_preserves_pins_until_unpinned() {
        let (service, clock, _storage, workspace, session, chunks) = fixture(2).await;

        for _ in 0..2 {
            service
                .activate_source(
                    &workspace.id,
                    &session.id,
                    None,
                    &chunks[0].id,
                    ContextSourceType::Code,
                )
                .await
                .unwrap();
        }
        clock.advance(Duration::minutes(120));
        service
            .activate_source(
                &workspace.id,
                &session.id,
                None,
                &chunks[1].id,
                ContextSourceType::Code,
            )
            .await
            .unwrap();
        let first_pin = service
            .pin_context(
                &workspace.id,
                &session.id,
                None,
                &chunks[2].id,
                ContextSourceType::Code,
            )
            .await
            .unwrap();
        let repeated_pin = service
            .pin_context(
                &workspace.id,
                &session.id,
                None,
                &chunks[2].id,
                ContextSourceType::Code,
            )
            .await
            .unwrap();
        assert_eq!(repeated_pin.id, first_pin.id);
        service
            .activate_source(
                &workspace.id,
                &session.id,
                None,
                &chunks[2].id,
                ContextSourceType::Code,
            )
            .await
            .unwrap();

        let bounded = service
            .inspect_working_set(&workspace.id, &session.id, None)
            .await
            .unwrap();
        assert_eq!(bounded.entries.len(), 2);
        assert!(
            bounded
                .entries
                .iter()
                .any(|entry| entry.source_id == chunks[1].id)
        );
        assert!(
            bounded
                .entries
                .iter()
                .any(|entry| entry.source_id == chunks[2].id)
        );

        clock.advance(Duration::minutes(600));
        let pinned = service
            .inspect_working_set(&workspace.id, &session.id, None)
            .await
            .unwrap();
        assert_eq!(pinned.entries.len(), 1);
        assert_eq!(pinned.entries[0].source_id, chunks[2].id);
        assert_eq!(pinned.pins, vec![first_pin]);

        assert!(
            service
                .unpin_context(
                    &workspace.id,
                    &session.id,
                    None,
                    &chunks[2].id,
                    ContextSourceType::Code,
                )
                .await
                .unwrap()
        );
        let unpinned = service
            .inspect_working_set(&workspace.id, &session.id, None)
            .await
            .unwrap();
        assert!(unpinned.entries.is_empty());
        assert!(unpinned.pins.is_empty());
    }

    #[tokio::test]
    async fn mutations_validate_source_workspace_and_active_session() {
        let (service, _clock, storage, workspace, session, chunks) = fixture(10).await;
        let other = Workspace::new("C:/other-working-set", "other-working-set");
        storage.insert_workspace(&other).await.unwrap();

        let error = service
            .activate_source(
                &other.id,
                &session.id,
                None,
                &chunks[0].id,
                ContextSourceType::Code,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("different workspace"));

        storage.end_session(&session.id, Utc::now()).await.unwrap();
        let error = service
            .pin_context(
                &workspace.id,
                &session.id,
                None,
                &chunks[0].id,
                ContextSourceType::Code,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("ended session"));
    }

    #[tokio::test]
    async fn task_scopes_do_not_leak_entries() {
        let (service, _clock, storage, workspace, session, chunks) = fixture(10).await;
        let first_task = Task::new(
            &workspace.id,
            Some(session.id.clone()),
            "first task",
            serde_json::json!({}),
        );
        let second_task = Task::new(
            &workspace.id,
            Some(session.id.clone()),
            "second task",
            serde_json::json!({}),
        );
        storage.insert_task(&first_task).await.unwrap();
        storage.insert_task(&second_task).await.unwrap();

        for (task_id, chunk) in [
            (Some(first_task.id.as_str()), &chunks[0]),
            (Some(second_task.id.as_str()), &chunks[1]),
            (None, &chunks[2]),
        ] {
            service
                .activate_source(
                    &workspace.id,
                    &session.id,
                    task_id,
                    &chunk.id,
                    ContextSourceType::Code,
                )
                .await
                .unwrap();
        }

        let snapshot = service
            .inspect_working_set(&workspace.id, &session.id, Some(&first_task.id))
            .await
            .unwrap();
        let source_ids: HashSet<_> = snapshot
            .entries
            .iter()
            .map(|entry| entry.source_id.as_str())
            .collect();
        assert_eq!(
            source_ids,
            HashSet::from([chunks[0].id.as_str(), chunks[2].id.as_str()])
        );
    }

    #[tokio::test]
    async fn concurrent_activations_merge_without_lost_updates() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            SqliteStorage::open(directory.path().join("working-set.sqlite"))
                .await
                .unwrap(),
        );
        let workspace = Workspace::new("C:/concurrent-working-set", "concurrent-working-set");
        storage.insert_workspace(&workspace).await.unwrap();
        let session = Session::new(&workspace.id, serde_json::json!({}));
        storage.insert_session(&session).await.unwrap();
        let document = Document::new(&workspace.id, "src/concurrent.rs");
        storage.insert_document(&document).await.unwrap();
        let chunk = StoredChunk::new(&document.id, "run", "fn run() {}");
        storage.insert_chunk(&chunk).await.unwrap();
        let clock = Arc::new(TestClock::new(
            Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap(),
        ));
        let clock_handle: Arc<dyn Clock> = clock;
        let service = Arc::new(
            ContextService::with_clock(
                Arc::clone(&storage),
                test_retrieval(Arc::clone(&storage)),
                WorkingSetConfig {
                    activation_increment: 0.1,
                    ..WorkingSetConfig::default()
                },
                TemporalConfig::default(),
                ContextConfig::default(),
                clock_handle,
            )
            .unwrap(),
        );

        let mut activations = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let service = Arc::clone(&service);
            let workspace_id = workspace.id.clone();
            let session_id = session.id.clone();
            let chunk_id = chunk.id.clone();
            activations.spawn(async move {
                service
                    .activate_source(
                        &workspace_id,
                        &session_id,
                        None,
                        &chunk_id,
                        ContextSourceType::Code,
                    )
                    .await
            });
        }
        while let Some(result) = activations.join_next().await {
            result.unwrap().unwrap();
        }

        let snapshot = service
            .inspect_working_set(&workspace.id, &session.id, None)
            .await
            .unwrap();
        assert_eq!(snapshot.entries.len(), 1);
        assert!((snapshot.entries[0].activation_score - 0.8).abs() < 0.0001);
    }

    #[tokio::test]
    async fn every_v0_2_source_type_is_validated_in_its_workspace() {
        let (service, _clock, storage, workspace, session, chunks) = fixture(10).await;
        let memory = MemoryRecord::new(&workspace.id, MemoryKind::Decision, "Use cosine");
        storage.insert_memory(&memory).await.unwrap();
        let event = CortexEvent::new(
            &workspace.id,
            EventType::FileModified,
            serde_json::json!({"path": "src/lib.rs"}),
        );
        storage.insert_event(&event).await.unwrap();
        let task = Task::new(
            &workspace.id,
            Some(session.id.clone()),
            "validate sources",
            serde_json::json!({}),
        );
        storage.insert_task(&task).await.unwrap();

        for (source_id, source_type) in [
            (chunks[0].id.as_str(), ContextSourceType::Code),
            (chunks[0].document_id.as_str(), ContextSourceType::Document),
            (memory.id.as_str(), ContextSourceType::Memory),
            (event.id.as_str(), ContextSourceType::Event),
            (task.id.as_str(), ContextSourceType::TaskState),
            (session.id.as_str(), ContextSourceType::SessionState),
        ] {
            service
                .activate_source(&workspace.id, &session.id, None, source_id, source_type)
                .await
                .unwrap();
        }

        let snapshot = service
            .inspect_working_set(&workspace.id, &session.id, None)
            .await
            .unwrap();
        assert_eq!(snapshot.entries.len(), 6);

        let error = service
            .activate_source(
                &workspace.id,
                &session.id,
                None,
                "missing",
                ContextSourceType::Memory,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, CortexError::NotFound(_)));
    }

    #[tokio::test]
    async fn temporal_filters_respect_recent_windows_and_supersession() {
        let (service, clock, storage, workspace, _session, _chunks) = fixture(10).await;
        let now = clock.now();
        let mut old = MemoryRecord::new(&workspace.id, MemoryKind::Decision, "Use L2 distance");
        old.created_at = now - Duration::days(10);
        storage.insert_memory(&old).await.unwrap();
        let mut current = MemoryRecord::new(&workspace.id, MemoryKind::Decision, "Use cosine");
        current.created_at = now - Duration::hours(1);
        storage.insert_memory(&current).await.unwrap();
        storage
            .insert_memory_supersession(&crate::domain::MemorySupersession::new(
                &workspace.id,
                &old.id,
                &current.id,
            ))
            .await
            .unwrap();

        let mut recent = TemporalQuery::new(&workspace.id);
        recent.source_types = vec![ContextSourceType::Memory];
        recent.filter.recent_within = Some(crate::domain::RecentWindow::Hours(2));
        let results = service.temporal_retrieval(recent).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_id, current.id);
        assert!(results[0].recency_score > 0.9);

        let mut include_old = TemporalQuery::new(&workspace.id);
        include_old.source_types = vec![ContextSourceType::Memory];
        include_old.filter.include_superseded = true;
        let results = service.temporal_retrieval(include_old).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[1].freshness, ContextFreshness::Superseded);
    }

    #[tokio::test]
    async fn temporal_session_scope_selects_current_or_previous_session() {
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        let workspace = Workspace::new("C:/temporal-sessions", "temporal-sessions");
        storage.insert_workspace(&workspace).await.unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap();
        let mut previous = Session::new(&workspace.id, serde_json::json!({}));
        previous.started_at = now - Duration::hours(4);
        previous.ended_at = Some(now - Duration::hours(3));
        storage.insert_session(&previous).await.unwrap();
        let mut current = Session::new(&workspace.id, serde_json::json!({}));
        current.started_at = now - Duration::hours(2);
        storage.insert_session(&current).await.unwrap();
        let mut previous_memory =
            MemoryRecord::new(&workspace.id, MemoryKind::Decision, "previous");
        previous_memory.session_id = Some(previous.id.clone());
        previous_memory.created_at = now - Duration::hours(3) - Duration::minutes(30);
        storage.insert_memory(&previous_memory).await.unwrap();
        let mut previous_checkpoint =
            Checkpoint::new(&workspace.id, &previous.id, "previous checkpoint");
        previous_checkpoint.created_at = now - Duration::hours(3) - Duration::minutes(15);
        storage
            .insert_checkpoint(&previous_checkpoint)
            .await
            .unwrap();
        let mut current_memory = MemoryRecord::new(&workspace.id, MemoryKind::Decision, "current");
        current_memory.session_id = Some(current.id.clone());
        current_memory.created_at = now - Duration::hours(1);
        storage.insert_memory(&current_memory).await.unwrap();
        let clock = Arc::new(TestClock::new(now));
        let clock_handle: Arc<dyn Clock> = clock;
        let service = ContextService::with_clock(
            Arc::clone(&storage),
            test_retrieval(Arc::clone(&storage)),
            WorkingSetConfig::default(),
            TemporalConfig::default(),
            ContextConfig::default(),
            clock_handle,
        )
        .unwrap();

        let mut query = TemporalQuery::new(&workspace.id);
        query.session_id = Some(current.id.clone());
        query.source_types = vec![ContextSourceType::Memory];
        query.filter = TemporalFilter {
            session_scope: TemporalSessionScope::Current,
            ..TemporalFilter::default()
        };
        assert_eq!(
            service.temporal_retrieval(query.clone()).await.unwrap()[0].source_id,
            current_memory.id
        );
        query.filter.session_scope = TemporalSessionScope::Previous;
        assert_eq!(
            service.temporal_retrieval(query).await.unwrap()[0].source_id,
            previous_memory.id
        );
        let checkpoints = storage.checkpoints(&workspace.id).await.unwrap();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].id, previous_checkpoint.id);
        assert_eq!(checkpoints[0].session_id, previous.id);
    }

    #[tokio::test]
    async fn temporal_retrieval_prioritizes_current_source_truth_over_recency() {
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        let workspace = Workspace::new("C:/temporal-ranking", "temporal-ranking");
        storage.insert_workspace(&workspace).await.unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap();

        let mut document = Document::new(&workspace.id, "src/lib.rs");
        document.indexed_at = now - Duration::days(30);
        storage.insert_document(&document).await.unwrap();
        let mut chunk = StoredChunk::new(&document.id, "lib", "pub fn stable() {}");
        chunk.created_at = now - Duration::days(30);
        chunk.updated_at = now - Duration::days(30);
        storage.insert_chunk(&chunk).await.unwrap();

        let mut event = CortexEvent::new(
            &workspace.id,
            EventType::TaskUpdated,
            serde_json::json!({"status": "recent"}),
        );
        event.created_at = now - Duration::minutes(1);
        storage.insert_event(&event).await.unwrap();
        let mut memory = MemoryRecord::new(
            &workspace.id,
            MemoryKind::Decision,
            "The old implementation uses L2 distance",
        );
        memory.created_at = now - Duration::days(1);
        storage.insert_memory(&memory).await.unwrap();

        let clock_handle: Arc<dyn Clock> = Arc::new(TestClock::new(now));
        let service = ContextService::with_clock(
            Arc::clone(&storage),
            test_retrieval(Arc::clone(&storage)),
            WorkingSetConfig::default(),
            TemporalConfig::default(),
            ContextConfig::default(),
            clock_handle,
        )
        .unwrap();
        let mut query = TemporalQuery::new(&workspace.id);
        query.source_types = vec![
            ContextSourceType::Code,
            ContextSourceType::Memory,
            ContextSourceType::Event,
        ];
        query.filter.modified_before = Some(now - Duration::days(7));
        let results = service.temporal_retrieval(query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_id, chunk.id);

        let mut query = TemporalQuery::new(&workspace.id);
        query.source_types = vec![
            ContextSourceType::Code,
            ContextSourceType::Memory,
            ContextSourceType::Event,
        ];
        query.limit = 1;
        let results = service.temporal_retrieval(query.clone()).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_id, chunk.id);

        query.limit = 3;
        let results = service.temporal_retrieval(query).await.unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].source_id, chunk.id);
        assert!(results[0].recency_score < results[1].recency_score);
        assert!(results[0].recency_score < results[2].recency_score);
        assert_eq!(results[0].freshness, ContextFreshness::Current);
        assert_eq!(results[1].source_id, event.id);
        assert_eq!(results[1].freshness, ContextFreshness::Historical);
        assert_eq!(results[2].source_id, memory.id);
        assert_eq!(results[2].freshness, ContextFreshness::Historical);
    }

    #[tokio::test]
    async fn temporal_retrieval_finds_a_recent_code_edit_by_modified_time() {
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        let workspace = Workspace::new("C:/temporal-edit", "temporal-edit");
        storage.insert_workspace(&workspace).await.unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap();
        let mut document = Document::new(&workspace.id, "src/edit.rs");
        document.indexed_at = now - Duration::hours(1);
        storage.insert_document(&document).await.unwrap();

        let mut old = StoredChunk::new(&document.id, "old", "fn old() {}");
        old.created_at = now - Duration::days(10);
        old.updated_at = now - Duration::days(10);
        storage.insert_chunk(&old).await.unwrap();
        let mut edited = StoredChunk::new(&document.id, "edited", "fn edited() {}");
        edited.created_at = now - Duration::days(10);
        edited.updated_at = now - Duration::minutes(30);
        storage.insert_chunk(&edited).await.unwrap();

        let clock_handle: Arc<dyn Clock> = Arc::new(TestClock::new(now));
        let service = ContextService::with_clock(
            Arc::clone(&storage),
            test_retrieval(Arc::clone(&storage)),
            WorkingSetConfig::default(),
            TemporalConfig::default(),
            ContextConfig::default(),
            clock_handle,
        )
        .unwrap();
        let mut query = TemporalQuery::new(&workspace.id);
        query.source_types = vec![ContextSourceType::Code];
        query.filter.modified_after = Some(now - Duration::hours(2));
        let results = service.temporal_retrieval(query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_id, edited.id);
        assert!(results[0].recency_score > 0.99);
    }

    #[tokio::test]
    async fn temporal_retrieval_rejects_unrepresentable_recent_windows() {
        let (service, _clock, _storage, workspace, _session, _chunks) = fixture(10).await;
        let mut query = TemporalQuery::new(&workspace.id);
        query.filter.recent_within = Some(crate::domain::RecentWindow::Hours(u64::MAX));
        let error = service.temporal_retrieval(query).await.unwrap_err();
        assert!(error.to_string().contains("recent window is too large"));
    }

    #[tokio::test]
    async fn checkpoints_preserve_structured_state_and_latest_scope() {
        let (service, clock, storage, workspace, session, _chunks) = fixture(10).await;
        let task = Task::new(
            &workspace.id,
            Some(session.id.clone()),
            "checkpoint task",
            serde_json::json!({}),
        );
        storage.insert_task(&task).await.unwrap();
        let mut decision = MemoryRecord::new(&workspace.id, MemoryKind::Decision, "Use cosine");
        decision.session_id = Some(session.id.clone());
        decision.task_id = Some(task.id.clone());
        storage.insert_memory(&decision).await.unwrap();

        let mut first = Checkpoint::new(&workspace.id, &session.id, "First checkpoint");
        first.task_id = Some(task.id.clone());
        first.objective = Some("Preserve context state".into());
        first.completed = vec!["Schema reviewed".into()];
        first.decision_ids = vec![decision.id.clone()];
        first.open_problems = vec!["Add checkpoint API".into()];
        first.related_paths = vec!["src/service/context.rs".into()];
        first.related_symbols = vec!["ContextService".into()];
        first.next_action = Some("Expose checkpoint reads".into());
        let first = service.create_checkpoint(first).await.unwrap();
        assert_eq!(first.created_at, clock.now());

        clock.advance(Duration::minutes(1));
        let mut latest = Checkpoint::new(&workspace.id, &session.id, "Latest checkpoint");
        latest.task_id = Some(task.id.clone());
        let latest = service.create_checkpoint(latest).await.unwrap();

        assert_eq!(
            service.latest_checkpoint(&workspace.id).await.unwrap(),
            Some(latest.clone())
        );
        assert_eq!(
            service
                .latest_checkpoint_for_session(&workspace.id, &session.id)
                .await
                .unwrap(),
            Some(latest.clone())
        );
        assert_eq!(
            service
                .latest_checkpoint_for_task(&workspace.id, &task.id)
                .await
                .unwrap(),
            Some(latest)
        );
        assert_eq!(
            service
                .latest_checkpoint_for_task(&workspace.id, "missing-task")
                .await
                .unwrap_err()
                .to_string(),
            "not found: task missing-task"
        );
    }

    #[tokio::test]
    async fn checkpoint_creation_validates_content_and_decision_provenance() {
        let (service, _clock, storage, workspace, session, _chunks) = fixture(10).await;
        let empty = Checkpoint::new(&workspace.id, &session.id, "   ");
        assert!(service.create_checkpoint(empty).await.is_err());

        let mut duplicate = Checkpoint::new(&workspace.id, &session.id, "duplicate decision");
        duplicate.decision_ids = vec!["same".into(), "same".into()];
        assert!(service.create_checkpoint(duplicate).await.is_err());

        let mut missing = Checkpoint::new(&workspace.id, &session.id, "missing decision");
        missing.decision_ids = vec!["missing".into()];
        assert!(matches!(
            service.create_checkpoint(missing).await,
            Err(CortexError::NotFound(_))
        ));

        let observation =
            MemoryRecord::new(&workspace.id, MemoryKind::Observation, "not a decision");
        storage.insert_memory(&observation).await.unwrap();
        let mut not_a_decision = Checkpoint::new(&workspace.id, &session.id, "wrong kind");
        not_a_decision.decision_ids = vec![observation.id];
        assert!(matches!(
            service.create_checkpoint(not_a_decision).await,
            Err(CortexError::NotFound(_))
        ));

        storage
            .end_session(
                &session.id,
                Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap(),
            )
            .await
            .unwrap();
        let ended = Checkpoint::new(&workspace.id, &session.id, "ended session");
        assert!(service.create_checkpoint(ended).await.is_err());
    }

    #[tokio::test]
    async fn candidate_pool_merges_retrieval_temporal_and_working_set_sources() {
        let (service, _clock, storage, workspace, session, chunks) = fixture(10).await;
        let task = Task::new(
            &workspace.id,
            Some(session.id.clone()),
            "candidate task",
            serde_json::json!({}),
        );
        storage.insert_task(&task).await.unwrap();
        let mut memory = MemoryRecord::new(&workspace.id, MemoryKind::Decision, "Use cosine");
        memory.session_id = Some(session.id.clone());
        memory.task_id = Some(task.id.clone());
        storage.insert_memory(&memory).await.unwrap();
        let mut event = CortexEvent::new(
            &workspace.id,
            EventType::TaskUpdated,
            serde_json::json!({"state": "candidate"}),
        );
        event.session_id = Some(session.id.clone());
        event.task_id = Some(task.id.clone());
        storage.insert_event(&event).await.unwrap();
        service
            .activate_source(
                &workspace.id,
                &session.id,
                Some(&task.id),
                &chunks[0].id,
                ContextSourceType::Code,
            )
            .await
            .unwrap();
        service
            .activate_source(
                &workspace.id,
                &session.id,
                Some(&task.id),
                &memory.id,
                ContextSourceType::Memory,
            )
            .await
            .unwrap();

        let mut request = ContextRequest::new(&workspace.id);
        request.query = Some("fn".into());
        request.session_id = Some(session.id.clone());
        request.task_id = Some(task.id.clone());
        let pool = service.build_candidate_pool(request).await.unwrap();

        assert!(pool.candidates.len() <= 50);
        let code: Vec<_> = pool
            .candidates
            .iter()
            .filter(|candidate| candidate.source_id == chunks[0].id)
            .collect();
        assert_eq!(code.len(), 1);
        assert!(
            code[0]
                .reasons
                .contains(&ContextSelectionReason::DirectLexicalMatch)
        );
        assert!(
            code[0]
                .reasons
                .contains(&ContextSelectionReason::ActiveWorkingSet)
        );
        let memory: Vec<_> = pool
            .candidates
            .iter()
            .filter(|candidate| candidate.source_id == memory.id)
            .collect();
        assert_eq!(memory.len(), 1);
        assert!(
            memory[0]
                .reasons
                .contains(&ContextSelectionReason::ActiveWorkingSet)
        );
        assert!(pool.candidates.iter().any(|candidate| {
            candidate.source_id == task.id
                && candidate
                    .reasons
                    .contains(&ContextSelectionReason::ActiveTaskReference)
        }));
        assert!(pool.candidates.iter().any(|candidate| {
            candidate.source_id == session.id
                && candidate
                    .reasons
                    .contains(&ContextSelectionReason::ResumeState)
        }));
    }

    #[tokio::test]
    async fn candidate_pool_deduplicates_and_respects_its_bound() {
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        let workspace = Workspace::new("C:/bounded-candidates", "bounded-candidates");
        storage.insert_workspace(&workspace).await.unwrap();
        let session = Session::new(&workspace.id, serde_json::json!({}));
        storage.insert_session(&session).await.unwrap();
        let document = Document::new(&workspace.id, "src/pool.rs");
        storage.insert_document(&document).await.unwrap();
        for index in 0..10 {
            let chunk = StoredChunk::new(&document.id, format!("chunk-{index}"), "fn pool() {}");
            storage.insert_chunk(&chunk).await.unwrap();
            storage
                .insert_memory(&MemoryRecord::new(
                    &workspace.id,
                    MemoryKind::Observation,
                    format!("memory {index}"),
                ))
                .await
                .unwrap();
        }
        let clock: Arc<dyn Clock> = Arc::new(TestClock::new(
            Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap(),
        ));
        let service = ContextService::with_clock(
            Arc::clone(&storage),
            test_retrieval(Arc::clone(&storage)),
            WorkingSetConfig::default(),
            TemporalConfig::default(),
            ContextConfig {
                candidate_pool_limit: 3,
            },
            clock,
        )
        .unwrap();

        let pool = service
            .build_candidate_pool(ContextRequest::new(&workspace.id))
            .await
            .unwrap();
        assert_eq!(pool.candidates.len(), 3);
        let unique: HashSet<_> = pool
            .candidates
            .iter()
            .map(|candidate| (candidate.source_type.storage_name(), &candidate.source_id))
            .collect();
        assert_eq!(unique.len(), pool.candidates.len());
    }
}
