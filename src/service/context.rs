use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Utc};

use crate::{
    CortexError, Result,
    config::{
        ContextBudgetConfig, ContextConfig, ContextRankingConfig, TemporalConfig, WorkingSetConfig,
    },
    domain::{
        Checkpoint, ContextCandidate, ContextCandidatePool, ContextExplanation, ContextFreshness,
        ContextItem, ContextPacket, ContextPin, ContextRequest, ContextScores,
        ContextSelectionExplanation, ContextSelectionReason, ContextSourceType, CortexEvent,
        EventType, GraphEdgeType, MemoryKind, RecentChange, ResumeContext, ResumeContextRequest,
        ResumeSessionSelection, ResumeTaskSelection, SourceSegment, StoredChunk, Task,
        TemporalBounds, TemporalContextItem, TemporalQuery, TemporalSessionScope, WorkingSetEntry,
        WorkingSetSnapshot,
    },
    embedding::{ConservativeByteCounter, TokenCounter},
    retrieval::{RetrievalResult, RetrievalService},
    storage::{SqliteStorage, StructuralRelation, TemporalCandidate},
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
    token_counter: Arc<dyn TokenCounter>,
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
        Self::new_with_token_counter(
            storage,
            retrieval,
            config,
            temporal,
            context,
            Arc::new(ConservativeByteCounter),
        )
    }

    pub fn new_with_token_counter(
        storage: Arc<SqliteStorage>,
        retrieval: Arc<RetrievalService>,
        config: WorkingSetConfig,
        temporal: TemporalConfig,
        context: ContextConfig,
        token_counter: Arc<dyn TokenCounter>,
    ) -> Result<Self> {
        Self::with_clock_and_token_counter(
            storage,
            retrieval,
            config,
            temporal,
            context,
            token_counter,
            Arc::new(SystemClock),
        )
    }

    #[cfg(test)]
    fn with_clock(
        storage: Arc<SqliteStorage>,
        retrieval: Arc<RetrievalService>,
        config: WorkingSetConfig,
        temporal: TemporalConfig,
        context: ContextConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        Self::with_clock_and_token_counter(
            storage,
            retrieval,
            config,
            temporal,
            context,
            Arc::new(ConservativeByteCounter),
            clock,
        )
    }

    fn with_clock_and_token_counter(
        storage: Arc<SqliteStorage>,
        retrieval: Arc<RetrievalService>,
        config: WorkingSetConfig,
        temporal: TemporalConfig,
        context: ContextConfig,
        token_counter: Arc<dyn TokenCounter>,
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
            token_counter,
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
        let effective_session_id = self.effective_candidate_session_id(&request).await?;
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
        let direct_memories = match request
            .query
            .as_deref()
            .filter(|query| !query.trim().is_empty())
        {
            Some(query) if request.include_memories => {
                self.direct_memory_candidates(&request.workspace_id, query, limit, generated_at)
                    .await?
            }
            _ => Vec::new(),
        };

        let mut candidates = BTreeMap::new();
        for item in temporal {
            if temporal_item_matches_candidate_scope(
                &item,
                &request,
                effective_session_id.as_deref(),
            ) {
                insert_candidate(&mut candidates, candidate_from_temporal(item));
            }
        }
        for result in direct_code {
            insert_candidate(&mut candidates, candidate_from_retrieval(result));
        }
        for candidate in direct_memories {
            insert_candidate(&mut candidates, candidate);
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
            for pin in working_set.pins {
                if let Some(source) = self
                    .storage
                    .context_source_candidate(
                        &request.workspace_id,
                        &pin.source_id,
                        &pin.source_type,
                    )
                    .await?
                {
                    let mut candidate =
                        candidate_from_temporal(self.temporal_item(source, generated_at));
                    candidate.scores.working_set = self.config.max_activation_score;
                    candidate.reasons.push(ContextSelectionReason::Pinned);
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
            candidate.scores.task = 0.8;
            candidate.reasons.push(ContextSelectionReason::ResumeState);
            insert_candidate(&mut candidates, candidate);
        }

        if request.include_code {
            let source_ids: Vec<_> = candidates
                .values()
                .filter(|candidate| candidate.source_type == ContextSourceType::Code)
                .filter(|candidate| is_structural_seed(candidate))
                .map(|candidate| candidate.source_id.clone())
                .collect();
            for source_id in source_ids {
                for structural in self
                    .storage
                    .structural_code_candidates(
                        &request.workspace_id,
                        &source_id,
                        self.context.structural_expansion_limit,
                    )
                    .await?
                {
                    let mut candidate = candidate_from_temporal(
                        self.temporal_item(structural.candidate, generated_at),
                    );
                    candidate
                        .reasons
                        .push(structural_reason(structural.relation));
                    insert_candidate(&mut candidates, candidate);
                }
            }
        }

        let mut candidates: Vec<_> = candidates
            .into_values()
            .filter(|candidate| candidate_matches_scope(candidate, &request))
            .collect();
        for candidate in &mut candidates {
            rank_candidate(
                candidate,
                &self.context.ranking,
                self.config.max_activation_score,
            );
        }
        retain_bounded_candidates(&mut candidates, limit);
        Ok(ContextCandidatePool {
            workspace_id: request.workspace_id,
            session_id: request.session_id,
            task_id: request.task_id,
            candidates,
            generated_at,
        })
    }

    pub async fn assemble_context_packet(&self, request: ContextRequest) -> Result<ContextPacket> {
        if request.token_budget == 0 {
            self.validate_candidate_scope(&request).await?;
            let mut packet = ContextPacket {
                workspace_id: request.workspace_id,
                session_id: request.session_id,
                task_id: request.task_id,
                summary: None,
                items: Vec::new(),
                token_budget: 0,
                estimated_tokens: 0,
                generated_at: self.clock.now(),
                explanation: None,
            };
            attach_explanation(&mut packet, request.include_explanation);
            return Ok(packet);
        }

        let token_budget = request.token_budget;
        let include_explanation = request.include_explanation;
        let pool = self.build_candidate_pool(request).await?;
        let mut packet = build_context_packet(
            pool,
            token_budget,
            &self.context.budget,
            self.token_counter.as_ref(),
        );
        attach_explanation(&mut packet, include_explanation);
        Ok(packet)
    }

    pub async fn resume_context(&self, request: ResumeContextRequest) -> Result<ResumeContext> {
        const MAX_RESUME_MEMORIES: usize = 32;
        const MAX_RESUME_EVENTS: usize = 128;
        const MAX_RESUME_PATHS: usize = 50;

        self.validate_workspace(&request.workspace_id).await?;
        let now = self.clock.now();
        let explicit_task = match request.task_id.as_deref() {
            Some(task_id) => {
                let task = self
                    .storage
                    .get_task(task_id)
                    .await?
                    .ok_or_else(|| CortexError::NotFound(format!("task {task_id}")))?;
                if task.workspace_id != request.workspace_id {
                    return Err(CortexError::Analysis(
                        "resume task belongs to a different workspace".into(),
                    ));
                }
                Some(task)
            }
            None => None,
        };
        let explicit_session = match request.session_id.as_deref() {
            Some(session_id) => {
                let session = self
                    .storage
                    .get_session(session_id)
                    .await?
                    .ok_or_else(|| CortexError::NotFound(format!("session {session_id}")))?;
                if session.workspace_id != request.workspace_id {
                    return Err(CortexError::Analysis(
                        "resume session belongs to a different workspace".into(),
                    ));
                }
                Some(session)
            }
            None => None,
        };
        if let (Some(session), Some(task)) = (&explicit_session, &explicit_task)
            && task.session_id.as_deref() != Some(&session.id)
        {
            return Err(CortexError::Analysis(
                "explicit resume session and task do not belong together".into(),
            ));
        }

        let (selected_session, session_selection) = if let Some(session) = explicit_session {
            (Some(session), ResumeSessionSelection::Explicit)
        } else if let Some(session_id) = explicit_task
            .as_ref()
            .and_then(|task| task.session_id.as_deref())
        {
            (
                self.storage.get_session(session_id).await?,
                ResumeSessionSelection::TaskAssociation,
            )
        } else if let Some(session) = self
            .storage
            .latest_active_session(&request.workspace_id)
            .await?
        {
            (Some(session), ResumeSessionSelection::LatestActive)
        } else if let Some(session) = self
            .storage
            .latest_ended_session(&request.workspace_id)
            .await?
        {
            (Some(session), ResumeSessionSelection::LatestEnded)
        } else {
            (None, ResumeSessionSelection::NoneAvailable)
        };
        let session_is_explicit = request.session_id.is_some();

        let (selected_task, task_selection) = if let Some(task) = explicit_task {
            (Some(task), ResumeTaskSelection::Explicit)
        } else if let Some(session) = selected_session.as_ref()
            && let Some(task) = self
                .storage
                .latest_active_task(&request.workspace_id, Some(&session.id))
                .await?
        {
            (Some(task), ResumeTaskSelection::SessionActive)
        } else if let Some(session) = selected_session.as_ref()
            && let Some(task) = self
                .storage
                .latest_incomplete_task(&request.workspace_id, Some(&session.id))
                .await?
        {
            (Some(task), ResumeTaskSelection::SessionIncomplete)
        } else if !session_is_explicit
            && let Some(task) = self
                .storage
                .latest_active_task(&request.workspace_id, None)
                .await?
        {
            (Some(task), ResumeTaskSelection::WorkspaceActive)
        } else if !session_is_explicit
            && let Some(task) = self
                .storage
                .latest_incomplete_task(&request.workspace_id, None)
                .await?
        {
            (Some(task), ResumeTaskSelection::WorkspaceIncomplete)
        } else {
            (None, ResumeTaskSelection::NoneAvailable)
        };

        let checkpoint = match selected_task.as_ref() {
            Some(task) => match self
                .storage
                .latest_checkpoint_for_task(&request.workspace_id, &task.id)
                .await?
            {
                Some(checkpoint) => Some(checkpoint),
                None => match task.session_id.as_deref() {
                    Some(session_id) => {
                        self.storage
                            .latest_taskless_checkpoint_for_session(
                                &request.workspace_id,
                                session_id,
                            )
                            .await?
                    }
                    None => None,
                },
            },
            None => match selected_session.as_ref() {
                Some(session) => {
                    self.storage
                        .latest_checkpoint_for_session(&request.workspace_id, &session.id)
                        .await?
                }
                None => {
                    self.storage
                        .latest_checkpoint(&request.workspace_id)
                        .await?
                }
            },
        };
        let evidence_session_id = checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.session_id.clone())
            .or_else(|| {
                selected_task
                    .as_ref()
                    .and_then(|task| task.session_id.clone())
            })
            .or_else(|| selected_session.as_ref().map(|session| session.id.clone()));
        let evidence_session = match evidence_session_id.as_deref() {
            Some(session_id) => self.storage.get_session(session_id).await?,
            None => None,
        };

        let mut candidates = BTreeMap::new();
        let mut paths = BTreeMap::<String, ()>::new();
        let mut symbols = BTreeMap::<String, ()>::new();
        if let Some(task) = selected_task.as_ref() {
            insert_candidate(
                &mut candidates,
                resume_task_candidate(task, now, self.temporal.recency_half_life_hours),
            );
        }
        if let Some(checkpoint) = checkpoint.as_ref() {
            insert_candidate(
                &mut candidates,
                resume_checkpoint_candidate(checkpoint, now, self.temporal.recency_half_life_hours),
            );
            for path in &checkpoint.related_paths {
                paths.insert(path.clone(), ());
            }
            for symbol in &checkpoint.related_symbols {
                symbols.insert(symbol.clone(), ());
            }
        }

        let mut memory_ids = BTreeMap::<String, ()>::new();
        if let Some(checkpoint) = checkpoint.as_ref() {
            for memory_id in &checkpoint.decision_ids {
                memory_ids.insert(memory_id.clone(), ());
            }
        }
        let memories = self
            .storage
            .resume_memories(
                &request.workspace_id,
                selected_task.as_ref().map(|task| task.id.as_str()),
                evidence_session_id.as_deref(),
                MAX_RESUME_MEMORIES,
            )
            .await?;
        for memory in &memories {
            memory_ids.insert(memory.id.clone(), ());
        }
        for memory_id in memory_ids.keys() {
            let Some(memory) = self
                .storage
                .resume_memory(&request.workspace_id, memory_id)
                .await?
            else {
                continue;
            };
            for path in &memory.related_paths {
                paths.insert(path.clone(), ());
            }
            if let Some(source) = self
                .storage
                .context_source_candidate(
                    &request.workspace_id,
                    &memory.id,
                    &ContextSourceType::Memory,
                )
                .await?
            {
                let mut candidate = candidate_from_temporal(self.temporal_item(source, now));
                candidate.scores.task = 0.8;
                candidate.reasons.push(match memory.kind {
                    MemoryKind::Decision => ContextSelectionReason::RecentDecision,
                    MemoryKind::Failure => ContextSelectionReason::RecentFailure,
                    _ => ContextSelectionReason::ResumeState,
                });
                if checkpoint
                    .as_ref()
                    .is_some_and(|checkpoint| checkpoint.decision_ids.contains(&memory.id))
                {
                    candidate.reasons.push(ContextSelectionReason::ResumeState);
                    candidate.scores.task = 0.9;
                }
                insert_candidate(&mut candidates, candidate);
            }
        }

        let events = match evidence_session.as_ref() {
            Some(session) => {
                self.storage
                    .resume_events(
                        &request.workspace_id,
                        &session.id,
                        session.started_at,
                        session.ended_at.unwrap_or(now),
                        selected_task.as_ref().map(|task| task.id.as_str()),
                        MAX_RESUME_EVENTS,
                    )
                    .await?
            }
            None => Vec::new(),
        };
        for event in events.iter().filter(is_failed_build_event) {
            if let Some(source) = self
                .storage
                .context_source_candidate(
                    &request.workspace_id,
                    &event.id,
                    &ContextSourceType::Event,
                )
                .await?
            {
                let mut candidate = candidate_from_temporal(self.temporal_item(source, now));
                candidate.scores.task = 0.7;
                candidate
                    .reasons
                    .push(ContextSelectionReason::RecentFailure);
                insert_candidate(&mut candidates, candidate);
            }
        }
        let mut recent_changes = aggregate_recent_changes(&events);
        for change in &mut recent_changes {
            change.currently_present = self
                .storage
                .find_document(&request.workspace_id, &change.path)
                .await?
                .is_some();
            paths.insert(change.path.clone(), ());
            let mut candidate =
                resume_change_candidate(change, now, self.temporal.recency_half_life_hours);
            candidate.scores.task = 0.7;
            insert_candidate(&mut candidates, candidate);
        }

        let mut working_sets = Vec::new();
        if self.config.enabled {
            let mut working_session_ids = BTreeMap::<String, ()>::new();
            if let Some(session) = selected_session.as_ref() {
                working_session_ids.insert(session.id.clone(), ());
            }
            if let Some(session_id) = evidence_session_id.as_deref() {
                working_session_ids.insert(session_id.to_owned(), ());
            }
            for session_id in working_session_ids.keys() {
                let task_id = selected_task
                    .as_ref()
                    .filter(|task| task.session_id.as_deref() == Some(session_id.as_str()))
                    .map(|task| task.id.as_str());
                let snapshot = self
                    .inspect_working_set(&request.workspace_id, session_id, task_id)
                    .await?;
                for entry in &snapshot.entries {
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
                            candidate_from_temporal(self.temporal_item(source, now));
                        candidate.scores.working_set = entry.activation_score;
                        candidate
                            .reasons
                            .push(ContextSelectionReason::ActiveWorkingSet);
                        if let Some(path) = candidate.path.clone() {
                            paths.insert(path, ());
                        }
                        insert_candidate(&mut candidates, candidate);
                    }
                }
                for pin in &snapshot.pins {
                    if let Some(source) = self
                        .storage
                        .context_source_candidate(
                            &request.workspace_id,
                            &pin.source_id,
                            &pin.source_type,
                        )
                        .await?
                    {
                        let mut candidate =
                            candidate_from_temporal(self.temporal_item(source, now));
                        candidate.scores.working_set = self.config.max_activation_score;
                        candidate.reasons.push(ContextSelectionReason::Pinned);
                        if let Some(path) = candidate.path.clone() {
                            paths.insert(path, ());
                        }
                        insert_candidate(&mut candidates, candidate);
                    }
                }
                working_sets.push(snapshot);
            }
        }

        for path in paths.keys().take(MAX_RESUME_PATHS) {
            let Some(document) = self
                .storage
                .find_document(&request.workspace_id, path)
                .await?
            else {
                continue;
            };
            let chunks = self.storage.list_chunks(&document.id).await?;
            let mut ordered_chunks: Vec<_> = chunks
                .iter()
                .filter(|chunk| chunk_matches_symbols(chunk, symbols.keys()))
                .chain(
                    chunks
                        .iter()
                        .filter(|chunk| !chunk_matches_symbols(chunk, symbols.keys())),
                )
                .collect();
            ordered_chunks.truncate(self.context.structural_expansion_limit.max(1));
            for chunk in ordered_chunks {
                if let Some(source) = self
                    .storage
                    .context_source_candidate(
                        &request.workspace_id,
                        &chunk.id,
                        &ContextSourceType::Code,
                    )
                    .await?
                {
                    let mut candidate = candidate_from_temporal(self.temporal_item(source, now));
                    candidate.scores.task = 0.75;
                    candidate.reasons.push(
                        if recent_changes.iter().any(|change| change.path == *path) {
                            ContextSelectionReason::RecentModification
                        } else {
                            ContextSelectionReason::RelatedFile
                        },
                    );
                    insert_candidate(&mut candidates, candidate);
                }
            }
        }

        let source_ids: Vec<_> = candidates
            .values()
            .filter(|candidate| candidate.source_type == ContextSourceType::Code)
            .filter(|candidate| is_structural_seed(candidate))
            .map(|candidate| candidate.source_id.clone())
            .collect();
        for source_id in source_ids {
            for structural in self
                .storage
                .structural_code_candidates(
                    &request.workspace_id,
                    &source_id,
                    self.context.structural_expansion_limit,
                )
                .await?
            {
                let mut candidate =
                    candidate_from_temporal(self.temporal_item(structural.candidate, now));
                candidate
                    .reasons
                    .push(structural_reason(structural.relation));
                insert_candidate(&mut candidates, candidate);
            }
        }
        let mut candidates: Vec<_> = candidates.into_values().collect();
        for candidate in &mut candidates {
            rank_candidate(
                candidate,
                &self.context.ranking,
                self.config.max_activation_score,
            );
        }
        retain_bounded_candidates(&mut candidates, self.context.candidate_pool_limit);
        let mut packet = build_context_packet(
            ContextCandidatePool {
                workspace_id: request.workspace_id.clone(),
                session_id: selected_session.as_ref().map(|session| session.id.clone()),
                task_id: selected_task.as_ref().map(|task| task.id.clone()),
                candidates,
                generated_at: now,
            },
            request.token_budget,
            &self.context.budget,
            self.token_counter.as_ref(),
        );
        attach_explanation(&mut packet, request.include_explanation);
        Ok(ResumeContext {
            workspace_id: request.workspace_id,
            selected_session,
            session_selection,
            selected_task,
            task_selection,
            evidence_session_id,
            checkpoint,
            recent_changes,
            working_sets,
            packet,
        })
    }

    async fn direct_memory_candidates(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
        now: DateTime<Utc>,
    ) -> Result<Vec<ContextCandidate>> {
        let Some(match_query) = context_fts_query(query) else {
            return Ok(Vec::new());
        };
        let memories = self
            .storage
            .search_memories(workspace_id, &match_query, limit)
            .await?;
        let count = memories.len();
        let mut candidates = Vec::with_capacity(count);
        for (index, memory) in memories.into_iter().enumerate() {
            let Some(source) = self
                .storage
                .context_source_candidate(workspace_id, &memory.id, &ContextSourceType::Memory)
                .await?
            else {
                continue;
            };
            let mut candidate = candidate_from_temporal(self.temporal_item(source, now));
            if candidate.freshness == ContextFreshness::Superseded {
                continue;
            }
            candidate.scores.lexical = Some((count - index) as f32 / count as f32);
            candidate
                .reasons
                .push(ContextSelectionReason::DirectLexicalMatch);
            match memory.kind {
                MemoryKind::Decision => candidate
                    .reasons
                    .push(ContextSelectionReason::RecentDecision),
                MemoryKind::Failure => candidate
                    .reasons
                    .push(ContextSelectionReason::RecentFailure),
                _ => {}
            }
            candidates.push(candidate);
        }
        Ok(candidates)
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
        let recency_score = recency_score(activity_at, now, self.temporal.recency_half_life_hours);
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
            source_segments: candidate.source_segments,
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

    async fn effective_candidate_session_id(
        &self,
        request: &ContextRequest,
    ) -> Result<Option<String>> {
        if let Some(session_id) = request.session_id.as_ref() {
            return Ok(Some(session_id.clone()));
        }
        let Some(task_id) = request.task_id.as_deref() else {
            return Ok(None);
        };
        let task = self
            .storage
            .get_task(task_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("task {task_id}")))?;
        Ok(task.session_id)
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum BudgetCategory {
    Code,
    Structural,
    Memory,
    Event,
    State,
}

impl BudgetCategory {
    const COUNT: usize = 5;

    fn index(self) -> usize {
        match self {
            Self::Code => 0,
            Self::Structural => 1,
            Self::Memory => 2,
            Self::Event => 3,
            Self::State => 4,
        }
    }
}

fn build_context_packet(
    pool: ContextCandidatePool,
    token_budget: usize,
    config: &ContextBudgetConfig,
    token_counter: &dyn TokenCounter,
) -> ContextPacket {
    let generated_at = pool.generated_at;
    let workspace_id = pool.workspace_id;
    let session_id = pool.session_id;
    let task_id = pool.task_id;
    let candidates = pool.candidates;
    let estimates: Vec<_> = candidates
        .iter()
        .map(|candidate| token_counter.count(&candidate.content).tokens)
        .collect();
    let category_budgets = category_budgets(token_budget, config);
    let mut category_usage = [0_usize; BudgetCategory::COUNT];
    let mut selected_keys = HashSet::new();
    let mut items = Vec::new();
    let mut used_tokens = 0_usize;

    let mut required_indexes: Vec<_> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            required_priority(candidate).map(|priority| (priority, index))
        })
        .collect();
    required_indexes.sort_by(
        |(left_priority, left_index), (right_priority, right_index)| {
            left_priority
                .cmp(right_priority)
                .then_with(|| candidate_order(&candidates[*left_index], &candidates[*right_index]))
        },
    );
    for (_, index) in required_indexes {
        let candidate = &candidates[index];
        if is_redundant_code(candidate, &items) {
            continue;
        }
        let remaining = token_budget.saturating_sub(used_tokens);
        if let Some(item) = context_item_for_budget(candidate, remaining, true, token_counter) {
            record_selected_item(
                &mut items,
                &mut selected_keys,
                &mut category_usage,
                &mut used_tokens,
                item,
            );
        }
    }

    let mut candidate_indexes: Vec<_> = (0..candidates.len()).collect();
    candidate_indexes.sort_by(|left, right| {
        candidate_value(&candidates[*right], estimates[*right])
            .total_cmp(&candidate_value(&candidates[*left], estimates[*left]))
            .then_with(|| candidate_order(&candidates[*left], &candidates[*right]))
    });

    for index in &candidate_indexes {
        let candidate = &candidates[*index];
        if selected_keys.contains(&candidate_key(candidate)) || is_redundant_code(candidate, &items)
        {
            continue;
        }
        let estimate = estimates[*index];
        let category = budget_category(candidate);
        if estimate > token_budget.saturating_sub(used_tokens)
            || category_usage[category.index()].saturating_add(estimate)
                > category_budgets[category.index()]
        {
            continue;
        }
        if let Some(item) = context_item_for_budget(
            candidate,
            token_budget.saturating_sub(used_tokens),
            false,
            token_counter,
        ) {
            record_selected_item(
                &mut items,
                &mut selected_keys,
                &mut category_usage,
                &mut used_tokens,
                item,
            );
        }
    }

    for index in candidate_indexes {
        let candidate = &candidates[index];
        if selected_keys.contains(&candidate_key(candidate)) || is_redundant_code(candidate, &items)
        {
            continue;
        }
        if estimates[index] > token_budget.saturating_sub(used_tokens) {
            continue;
        }
        if let Some(item) = context_item_for_budget(
            candidate,
            token_budget.saturating_sub(used_tokens),
            false,
            token_counter,
        ) {
            record_selected_item(
                &mut items,
                &mut selected_keys,
                &mut category_usage,
                &mut used_tokens,
                item,
            );
        }
    }

    ContextPacket {
        workspace_id,
        session_id,
        task_id,
        summary: None,
        items,
        token_budget,
        estimated_tokens: used_tokens,
        generated_at,
        explanation: None,
    }
}

fn attach_explanation(packet: &mut ContextPacket, include_explanation: bool) {
    if include_explanation {
        packet.explanation = Some(ContextExplanation {
            selected: packet
                .items
                .iter()
                .map(|item| ContextSelectionExplanation {
                    source_id: item.source_id.clone(),
                    source_type: item.source_type.clone(),
                    path: item.path.clone(),
                    symbol: item.symbol.clone(),
                    reasons: item.reasons.clone(),
                    scores: item.scores.clone(),
                    estimated_tokens: item.estimated_tokens,
                    truncated: item.truncated,
                })
                .collect(),
        });
    }
}

fn category_budgets(token_budget: usize, config: &ContextBudgetConfig) -> [usize; 5] {
    [
        (token_budget as f32 * config.code_fraction).floor() as usize,
        (token_budget as f32 * config.structural_fraction).floor() as usize,
        (token_budget as f32 * config.memory_fraction).floor() as usize,
        (token_budget as f32 * config.event_fraction).floor() as usize,
        (token_budget as f32 * config.state_fraction).floor() as usize,
    ]
}

fn required_priority(candidate: &ContextCandidate) -> Option<u8> {
    if candidate
        .reasons
        .contains(&ContextSelectionReason::ActiveTaskReference)
    {
        Some(0)
    } else if candidate
        .reasons
        .contains(&ContextSelectionReason::CurrentCheckpoint)
    {
        Some(1)
    } else if candidate.reasons.contains(&ContextSelectionReason::Pinned) {
        Some(2)
    } else {
        None
    }
}

fn retain_bounded_candidates(candidates: &mut Vec<ContextCandidate>, limit: usize) {
    candidates.sort_by(|left, right| {
        required_priority(left)
            .unwrap_or(u8::MAX)
            .cmp(&required_priority(right).unwrap_or(u8::MAX))
            .then_with(|| candidate_order(left, right))
    });
    let selected_has_structural = candidates
        .get(..limit)
        .unwrap_or(candidates)
        .iter()
        .any(|candidate| !candidate.structural_evidence.is_empty());
    if !selected_has_structural
        && let Some(structural_index) =
            candidates
                .iter()
                .enumerate()
                .skip(limit)
                .find_map(|(index, candidate)| {
                    (!candidate.structural_evidence.is_empty()).then_some(index)
                })
        && let Some(replacement_index) = (0..limit.min(candidates.len()))
            .rev()
            .find(|index| required_priority(&candidates[*index]).is_none())
    {
        let structural = candidates.remove(structural_index);
        candidates.remove(replacement_index);
        candidates.truncate(limit.saturating_sub(1));
        candidates.push(structural);
    } else {
        candidates.truncate(limit);
    }
    candidates.sort_by(candidate_order);
}

fn candidate_key(candidate: &ContextCandidate) -> (String, String) {
    (
        candidate.source_type.storage_name(),
        candidate.source_id.clone(),
    )
}

fn candidate_value(candidate: &ContextCandidate, estimated_tokens: usize) -> f32 {
    let query_evidence =
        candidate.scores.semantic.unwrap_or(0.0) + candidate.scores.lexical.unwrap_or(0.0);
    (candidate.scores.final_score + query_evidence) / estimated_tokens.max(1) as f32
}

fn budget_category(candidate: &ContextCandidate) -> BudgetCategory {
    match candidate.source_type {
        ContextSourceType::Code | ContextSourceType::Document => {
            if candidate.scores.structural > 0.0 {
                BudgetCategory::Structural
            } else {
                BudgetCategory::Code
            }
        }
        ContextSourceType::Memory => BudgetCategory::Memory,
        ContextSourceType::Event => BudgetCategory::Event,
        ContextSourceType::TaskState
        | ContextSourceType::SessionState
        | ContextSourceType::Other(_) => BudgetCategory::State,
    }
}

fn context_item_for_budget(
    candidate: &ContextCandidate,
    available_tokens: usize,
    allow_truncation: bool,
    token_counter: &dyn TokenCounter,
) -> Option<ContextItem> {
    let full_tokens = token_counter.count(&candidate.content).tokens;
    let (content, estimated_tokens, truncated) = if full_tokens <= available_tokens {
        (candidate.content.clone(), full_tokens, false)
    } else if allow_truncation {
        let (content, tokens) =
            truncate_to_budget(&candidate.content, available_tokens, token_counter)?;
        (content, tokens, true)
    } else {
        return None;
    };
    let source_segments = selected_source_segments(candidate, &content);
    Some(ContextItem {
        source_id: candidate.source_id.clone(),
        source_type: candidate.source_type.clone(),
        content,
        path: candidate.path.clone(),
        symbol: candidate.symbol.clone(),
        language: candidate.language.clone(),
        source_segments,
        freshness: candidate.freshness,
        scores: candidate.scores.clone(),
        reasons: candidate.reasons.clone(),
        structural_evidence: candidate.structural_evidence.clone(),
        estimated_tokens,
        truncated,
    })
}

fn truncate_to_budget(
    content: &str,
    token_budget: usize,
    token_counter: &dyn TokenCounter,
) -> Option<(String, usize)> {
    if content.is_empty() {
        let tokens = token_counter.count(content).tokens;
        return (tokens <= token_budget).then_some((String::new(), tokens));
    }
    let boundaries: Vec<_> = content
        .char_indices()
        .skip(1)
        .map(|(index, _)| index)
        .chain(std::iter::once(content.len()))
        .collect();
    let mut low = 0_usize;
    let mut high = boundaries.len();
    while low < high {
        let middle = low + (high - low) / 2;
        let tokens = token_counter.count(&content[..boundaries[middle]]).tokens;
        if tokens <= token_budget {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let end = boundaries.get(low.saturating_sub(1)).copied()?;
    let truncated = content[..end].to_owned();
    let tokens = token_counter.count(&truncated).tokens;
    (tokens <= token_budget).then_some((truncated, tokens))
}

fn selected_source_segments(
    candidate: &ContextCandidate,
    selected_content: &str,
) -> Vec<SourceSegment> {
    let mut segments = candidate.source_segments.clone();
    if selected_content.len() < candidate.content.len()
        && let [segment] = segments.as_mut_slice()
    {
        let selected_bytes = u64::try_from(selected_content.len()).unwrap_or(u64::MAX);
        segment.end_byte = segment
            .start_byte
            .saturating_add(selected_bytes)
            .min(segment.end_byte);
    }
    segments
}

fn is_redundant_code(candidate: &ContextCandidate, items: &[ContextItem]) -> bool {
    if candidate.source_type != ContextSourceType::Code {
        return false;
    }
    items.iter().any(|item| {
        item.source_type == ContextSourceType::Code
            && item.path == candidate.path
            && (item.content.contains(&candidate.content)
                || candidate.content.contains(&item.content))
    })
}

fn record_selected_item(
    items: &mut Vec<ContextItem>,
    selected_keys: &mut HashSet<(String, String)>,
    category_usage: &mut [usize; BudgetCategory::COUNT],
    used_tokens: &mut usize,
    item: ContextItem,
) {
    let category = match item.source_type {
        ContextSourceType::Code | ContextSourceType::Document => {
            if item.scores.structural > 0.0 {
                BudgetCategory::Structural
            } else {
                BudgetCategory::Code
            }
        }
        ContextSourceType::Memory => BudgetCategory::Memory,
        ContextSourceType::Event => BudgetCategory::Event,
        ContextSourceType::TaskState
        | ContextSourceType::SessionState
        | ContextSourceType::Other(_) => BudgetCategory::State,
    };
    selected_keys.insert((item.source_type.storage_name(), item.source_id.clone()));
    category_usage[category.index()] += item.estimated_tokens;
    *used_tokens += item.estimated_tokens;
    items.push(item);
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
    let provenance = provenance_score(&item.source_type);
    let freshness = freshness_score(item.freshness);
    ContextCandidate {
        source_id: item.source_id,
        source_type: item.source_type,
        content: item.content,
        path: item.path,
        symbol: item.symbol,
        language: item.language,
        source_segments: item.source_segments,
        freshness: item.freshness,
        scores: ContextScores {
            recency: item.recency_score,
            provenance,
            freshness,
            ..ContextScores::default()
        },
        reasons: Vec::new(),
        structural_evidence: Vec::new(),
    }
}

fn structural_reason(relation: StructuralRelation) -> ContextSelectionReason {
    match relation {
        StructuralRelation::Container => ContextSelectionReason::ContainerOfRelevantSymbol,
        StructuralRelation::Neighbor => ContextSelectionReason::NeighborOfRelevantSymbol,
        StructuralRelation::Related => ContextSelectionReason::RelatedSymbol,
    }
}

fn is_structural_seed(candidate: &ContextCandidate) -> bool {
    candidate.reasons.iter().any(|reason| {
        matches!(
            reason,
            ContextSelectionReason::DirectSemanticMatch
                | ContextSelectionReason::DirectLexicalMatch
                | ContextSelectionReason::ActiveWorkingSet
                | ContextSelectionReason::Pinned
                | ContextSelectionReason::RecentModification
                | ContextSelectionReason::RelatedFile
        )
    })
}

fn recency_score(activity_at: DateTime<Utc>, now: DateTime<Utc>, half_life_hours: f64) -> f32 {
    let age_hours = now
        .signed_duration_since(activity_at)
        .num_milliseconds()
        .max(0) as f64
        / 3_600_000.0;
    0.5_f64.powf(age_hours / half_life_hours) as f32
}

fn resume_task_candidate(
    task: &Task,
    now: DateTime<Utc>,
    half_life_hours: f64,
) -> ContextCandidate {
    ContextCandidate {
        source_id: task.id.clone(),
        source_type: ContextSourceType::TaskState,
        content: format!(
            "Task: {}\nStatus: {}\nDetails: {}\nUpdated: {}",
            task.title,
            task.status.as_str(),
            serde_json::to_string(&task.details).unwrap_or_else(|_| "null".into()),
            task.updated_at.to_rfc3339(),
        ),
        path: None,
        symbol: None,
        language: None,
        source_segments: Vec::new(),
        freshness: ContextFreshness::Current,
        scores: ContextScores {
            recency: recency_score(task.updated_at, now, half_life_hours),
            task: 1.0,
            provenance: provenance_score(&ContextSourceType::TaskState),
            freshness: freshness_score(ContextFreshness::Current),
            ..ContextScores::default()
        },
        reasons: vec![ContextSelectionReason::ActiveTaskReference],
        structural_evidence: Vec::new(),
    }
}

fn resume_checkpoint_candidate(
    checkpoint: &Checkpoint,
    now: DateTime<Utc>,
    half_life_hours: f64,
) -> ContextCandidate {
    ContextCandidate {
        source_id: checkpoint.id.clone(),
        source_type: ContextSourceType::Other("checkpoint".into()),
        content: render_checkpoint(checkpoint),
        path: None,
        symbol: None,
        language: None,
        source_segments: Vec::new(),
        freshness: ContextFreshness::Historical,
        scores: ContextScores {
            recency: recency_score(checkpoint.created_at, now, half_life_hours),
            task: 1.0,
            provenance: provenance_score(&ContextSourceType::Other("checkpoint".into())),
            freshness: freshness_score(ContextFreshness::Historical),
            ..ContextScores::default()
        },
        reasons: vec![ContextSelectionReason::CurrentCheckpoint],
        structural_evidence: Vec::new(),
    }
}

fn render_checkpoint(checkpoint: &Checkpoint) -> String {
    format!(
        "Checkpoint: {}\nObjective: {}\nCompleted: {}\nOpen problems: {}\nRelated paths: {}\nRelated symbols: {}\nNext action: {}",
        checkpoint.content,
        checkpoint.objective.as_deref().unwrap_or(""),
        checkpoint.completed.join(", "),
        checkpoint.open_problems.join(", "),
        checkpoint.related_paths.join(", "),
        checkpoint.related_symbols.join(", "),
        checkpoint.next_action.as_deref().unwrap_or(""),
    )
}

fn is_failed_build_event(event: &&CortexEvent) -> bool {
    matches!(
        event.event_type,
        EventType::CompilerResult | EventType::TestResult
    ) && event.payload.get("ok").and_then(serde_json::Value::as_bool) == Some(false)
}

fn aggregate_recent_changes(events: &[CortexEvent]) -> Vec<RecentChange> {
    let mut changes = BTreeMap::<String, RecentChange>::new();
    for event in events {
        if !matches!(
            event.event_type,
            EventType::FileCreated
                | EventType::FileModified
                | EventType::FileRemoved
                | EventType::FileRenamed
        ) {
            continue;
        }
        let Some(path) = event
            .payload
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(normalize_event_path)
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        let kind = event.event_type.storage_name();
        let entry = changes.entry(path.clone()).or_insert_with(|| RecentChange {
            path,
            change_count: 0,
            session_scoped_count: 0,
            unscoped_count: 0,
            change_kinds: Vec::new(),
            first_changed_at: event.created_at,
            last_changed_at: event.created_at,
            currently_present: false,
        });
        entry.change_count += 1;
        if event.session_id.is_some() {
            entry.session_scoped_count += 1;
        } else {
            entry.unscoped_count += 1;
        }
        if !entry.change_kinds.contains(&kind) {
            entry.change_kinds.push(kind);
            entry.change_kinds.sort();
        }
        entry.first_changed_at = entry.first_changed_at.min(event.created_at);
        entry.last_changed_at = entry.last_changed_at.max(event.created_at);
    }
    changes.into_values().collect()
}

fn normalize_event_path(path: &str) -> String {
    path.trim().trim_start_matches("./").replace('\\', "/")
}

fn resume_change_candidate(
    change: &RecentChange,
    now: DateTime<Utc>,
    half_life_hours: f64,
) -> ContextCandidate {
    ContextCandidate {
        source_id: format!("resume-change:{}", change.path),
        source_type: ContextSourceType::Event,
        content: format!(
            "Recent file change: {}\nKinds: {}\nChanges: {} (session-scoped: {}, unscoped: {})\nLast changed: {}\nCurrently present: {}",
            change.path,
            change.change_kinds.join(", "),
            change.change_count,
            change.session_scoped_count,
            change.unscoped_count,
            change.last_changed_at.to_rfc3339(),
            change.currently_present,
        ),
        path: Some(change.path.clone()),
        symbol: None,
        language: None,
        source_segments: Vec::new(),
        freshness: ContextFreshness::Historical,
        scores: ContextScores {
            recency: recency_score(change.last_changed_at, now, half_life_hours),
            provenance: provenance_score(&ContextSourceType::Event),
            freshness: freshness_score(ContextFreshness::Historical),
            ..ContextScores::default()
        },
        reasons: vec![ContextSelectionReason::RecentModification],
        structural_evidence: Vec::new(),
    }
}

fn chunk_matches_symbols<'a>(
    chunk: &StoredChunk,
    symbols: impl Iterator<Item = &'a String> + Clone,
) -> bool {
    symbols.clone().any(|symbol| {
        chunk.symbol.as_deref() == Some(symbol.as_str())
            || chunk.qualified_symbol.as_deref() == Some(symbol.as_str())
    })
}

fn candidate_from_retrieval(result: RetrievalResult) -> ContextCandidate {
    let mut reasons = Vec::new();
    if result.scores.semantic.is_some() {
        reasons.push(ContextSelectionReason::DirectSemanticMatch);
    }
    if result.scores.lexical.is_some() {
        reasons.push(ContextSelectionReason::DirectLexicalMatch);
    }
    for evidence in &result.structural_evidence {
        let reason = structural_evidence_reason(evidence);
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
    let source_segments = source_segment(&result.path, result.start_byte, result.end_byte)
        .into_iter()
        .collect();
    ContextCandidate {
        source_id: result.chunk_id,
        source_type: ContextSourceType::Code,
        content: result.content,
        path: Some(result.path),
        symbol: result.qualified_symbol.or(result.symbol),
        language: Some(result.language),
        source_segments,
        freshness: ContextFreshness::Current,
        scores: ContextScores {
            semantic: result.scores.semantic,
            lexical: result.scores.lexical,
            structural: result.scores.structural.unwrap_or_default(),
            provenance: provenance_score(&ContextSourceType::Code),
            freshness: freshness_score(ContextFreshness::Current),
            ..ContextScores::default()
        },
        reasons,
        structural_evidence: result.structural_evidence,
    }
}

fn structural_evidence_reason(
    evidence: &crate::retrieval::StructuralRetrievalEvidence,
) -> ContextSelectionReason {
    if evidence.path.distance() > 1 {
        return ContextSelectionReason::ImpactedByRelevantSymbol;
    }
    let Some(edge) = evidence.path.edges.last() else {
        return ContextSelectionReason::RelatedSymbol;
    };
    match edge.edge_type {
        GraphEdgeType::Calls => {
            if edge.from_node == evidence.node_id {
                ContextSelectionReason::CallerOfRelevantSymbol
            } else {
                ContextSelectionReason::CalleeOfRelevantSymbol
            }
        }
        GraphEdgeType::References | GraphEdgeType::UsesType | GraphEdgeType::Constructs => {
            ContextSelectionReason::ReferenceToRelevantSymbol
        }
        GraphEdgeType::Implements | GraphEdgeType::Extends | GraphEdgeType::Overrides => {
            ContextSelectionReason::ImplementationOfRelevantSymbol
        }
        GraphEdgeType::Tests => ContextSelectionReason::LikelyTestAssociationWithRelevantSymbol,
        GraphEdgeType::Imports | GraphEdgeType::DependsOn => {
            if edge.from_node == evidence.node_id {
                ContextSelectionReason::DependentOnRelevantSymbol
            } else {
                ContextSelectionReason::DependencyOfRelevantSymbol
            }
        }
        _ => ContextSelectionReason::RelatedSymbol,
    }
}

fn source_segment(path: &str, start_byte: i64, end_byte: i64) -> Option<SourceSegment> {
    let start_byte = u64::try_from(start_byte).ok()?;
    let end_byte = u64::try_from(end_byte).ok()?;
    (start_byte < end_byte).then(|| SourceSegment::new(path, start_byte, end_byte))
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
    existing.scores.freshness = existing.scores.freshness.max(incoming.scores.freshness);
    existing.scores.structural = existing.scores.structural.max(incoming.scores.structural);
    existing.scores.final_score = existing.scores.final_score.max(incoming.scores.final_score);
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
    for evidence in incoming.structural_evidence {
        if !existing.structural_evidence.iter().any(|current| {
            current.node_id == evidence.node_id
                && current.path.node_ids == evidence.path.node_ids
                && current.snapshot == evidence.snapshot
        }) {
            existing.structural_evidence.push(evidence);
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
        .final_score
        .total_cmp(&left.scores.final_score)
        .then_with(|| right.scores.working_set.total_cmp(&left.scores.working_set))
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

fn rank_candidate(
    candidate: &mut ContextCandidate,
    config: &ContextRankingConfig,
    max_activation_score: f32,
) {
    candidate.scores.semantic = candidate.scores.semantic.map(normalized_score);
    candidate.scores.lexical = candidate.scores.lexical.map(normalized_score);
    candidate.scores.recency = normalized_score(candidate.scores.recency);
    candidate.scores.working_set =
        normalized_score(candidate.scores.working_set / max_activation_score);
    candidate.scores.task = normalized_score(candidate.scores.task);
    candidate.scores.provenance = provenance_score(&candidate.source_type);
    candidate.scores.freshness = freshness_score(candidate.freshness);
    candidate.scores.structural = candidate
        .scores
        .structural
        .max(structural_score(&candidate.reasons));

    candidate.scores.final_score = (candidate.scores.semantic.unwrap_or_default()
        * config.semantic_weight
        + candidate.scores.lexical.unwrap_or_default() * config.lexical_weight
        + candidate.scores.task * config.task_weight
        + candidate.scores.working_set * config.working_set_weight
        + candidate.scores.recency * config.recency_weight
        + candidate.scores.provenance * config.provenance_weight
        + candidate.scores.freshness * config.freshness_weight
        + candidate.scores.structural * config.structural_weight)
        / config.total_weight();
}

fn normalized_score(score: f32) -> f32 {
    if score.is_finite() {
        score.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn provenance_score(source_type: &ContextSourceType) -> f32 {
    match source_type {
        ContextSourceType::Code | ContextSourceType::Document => 1.0,
        ContextSourceType::TaskState | ContextSourceType::SessionState => 0.95,
        ContextSourceType::Memory => 0.9,
        ContextSourceType::Event => 0.8,
        ContextSourceType::Other(_) => 0.5,
    }
}

fn freshness_score(freshness: ContextFreshness) -> f32 {
    match freshness {
        ContextFreshness::Current => 1.0,
        ContextFreshness::Unknown => 0.5,
        ContextFreshness::Historical => 0.25,
        ContextFreshness::Superseded => 0.0,
    }
}

fn structural_score(reasons: &[ContextSelectionReason]) -> f32 {
    reasons.iter().fold(0.0_f32, |score, reason| {
        score.max(match reason {
            ContextSelectionReason::ContainerOfRelevantSymbol => 1.0,
            ContextSelectionReason::NeighborOfRelevantSymbol => 0.8,
            ContextSelectionReason::RelatedSymbol | ContextSelectionReason::RelatedFile => 0.6,
            ContextSelectionReason::CallerOfRelevantSymbol
            | ContextSelectionReason::CalleeOfRelevantSymbol
            | ContextSelectionReason::ImplementationOfRelevantSymbol
            | ContextSelectionReason::LikelyTestAssociationWithRelevantSymbol => 1.0,
            ContextSelectionReason::ReferenceToRelevantSymbol
            | ContextSelectionReason::DependencyOfRelevantSymbol
            | ContextSelectionReason::DependentOnRelevantSymbol => 0.8,
            ContextSelectionReason::ImpactedByRelevantSymbol => 0.7,
            _ => 0.0,
        })
    })
}

fn context_fts_query(query: &str) -> Option<String> {
    let terms: Vec<_> = query
        .split(|character: char| {
            !character.is_alphanumeric() && character != '_' && character != ':'
        })
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"", term.replace('"', "")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

fn freshness_priority(freshness: ContextFreshness) -> u8 {
    match freshness {
        ContextFreshness::Current => 3,
        ContextFreshness::Unknown => 2,
        ContextFreshness::Historical => 1,
        ContextFreshness::Superseded => 0,
    }
}

fn temporal_item_matches_candidate_scope(
    item: &TemporalContextItem,
    request: &ContextRequest,
    effective_session_id: Option<&str>,
) -> bool {
    match item.source_type {
        ContextSourceType::TaskState => {
            if let Some(task_id) = request.task_id.as_deref() {
                item.source_id == task_id
            } else {
                effective_session_id
                    .is_none_or(|session_id| item.session_id.as_deref() == Some(session_id))
            }
        }
        ContextSourceType::SessionState => {
            effective_session_id.is_none_or(|session_id| item.source_id == session_id)
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::{Duration, TimeZone};

    use super::*;
    use crate::domain::{
        AnalyzedChunk, Checkpoint, CortexEvent, Document, EventType, GraphState, MemoryKind,
        MemoryRecord, MemorySupersession, Session, StoredChunk, StructuralEvidence, StructuralPath,
        StructuralReadOptions, Task, TemporalFilter, TemporalQuery, Workspace,
        WorkspaceGraphRevision,
    };
    use crate::embedding::{TokenCount, TokenCountAccuracy, provider::MockEmbeddingProvider};
    use crate::parsing::{
        LanguageAnalyzer,
        languages::{CSharpAnalyzer, PythonAnalyzer, RustAnalyzer, TypeScriptAnalyzer},
    };

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

    fn stored_chunk(document_id: &str, analyzed: AnalyzedChunk) -> StoredChunk {
        let AnalyzedChunk {
            stable_key,
            language,
            symbol,
            qualified_symbol,
            symbol_kind,
            start_byte,
            end_byte,
            start_line,
            end_line,
            content,
            metadata,
        } = analyzed;
        let mut stored = StoredChunk::new(document_id, stable_key, content);
        stored.language = language;
        stored.symbol = symbol;
        stored.qualified_symbol = qualified_symbol;
        stored.symbol_kind = symbol_kind;
        stored.start_byte = i64::try_from(start_byte).unwrap();
        stored.end_byte = i64::try_from(end_byte).unwrap();
        stored.start_line = i64::try_from(start_line).unwrap();
        stored.end_line = i64::try_from(end_line).unwrap();
        stored.metadata = metadata;
        stored
    }

    fn ranking_candidate(
        source_id: &str,
        source_type: ContextSourceType,
        freshness: ContextFreshness,
    ) -> ContextCandidate {
        ContextCandidate {
            source_id: source_id.into(),
            source_type,
            content: source_id.into(),
            path: None,
            symbol: None,
            language: None,
            source_segments: Vec::new(),
            freshness,
            scores: ContextScores::default(),
            reasons: Vec::new(),
            structural_evidence: Vec::new(),
        }
    }

    fn budget_pool(candidates: Vec<ContextCandidate>) -> ContextCandidatePool {
        ContextCandidatePool {
            workspace_id: "budget-workspace".into(),
            session_id: Some("budget-session".into()),
            task_id: Some("budget-task".into()),
            candidates,
            generated_at: Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap(),
        }
    }

    fn budget_candidate(
        source_id: &str,
        source_type: ContextSourceType,
        content: &str,
        final_score: f32,
    ) -> ContextCandidate {
        let mut candidate = ranking_candidate(source_id, source_type, ContextFreshness::Current);
        candidate.content = content.into();
        candidate.scores.final_score = final_score;
        candidate
    }

    fn structural_evidence(source_id: &str) -> StructuralEvidence {
        StructuralEvidence {
            seed_node_id: "seed".into(),
            node_id: source_id.into(),
            path: StructuralPath {
                node_ids: vec!["seed".into(), source_id.into()],
                edges: Vec::new(),
                confidence: 0.7,
            },
            snapshot: WorkspaceGraphRevision {
                workspace_id: "budget-workspace".into(),
                content_revision: 1,
                graph_content_revision: 1,
                graph_schema_version: 1,
                graph_state: GraphState::Current,
                graph_update_started_at: None,
                failed_graph_target_revision: None,
                last_graph_error: None,
                updated_at: Utc::now(),
            },
            limits: StructuralReadOptions::default(),
            truncated: false,
        }
    }

    struct OverheadTokenCounter;

    impl TokenCounter for OverheadTokenCounter {
        fn count(&self, text: &str) -> TokenCount {
            TokenCount {
                tokens: text.len() + 2,
                accuracy: TokenCountAccuracy::Exact,
            }
        }

        fn identity(&self) -> &str {
            "test-overhead-v1"
        }
    }

    fn ranked_first(mut candidates: Vec<ContextCandidate>) -> ContextCandidate {
        for candidate in &mut candidates {
            rank_candidate(candidate, &ContextRankingConfig::default(), 1.0);
        }
        candidates.sort_by(candidate_order);
        candidates.remove(0)
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
    async fn resume_context_carries_session_a_evidence_into_session_b() {
        let (service, clock, storage, workspace, session, chunks) = fixture(10).await;
        let started_at = Utc.with_ymd_and_hms(2026, 8, 27, 10, 0, 0).unwrap();
        storage.end_session(&session.id, started_at).await.unwrap();
        let mut session_a =
            Session::new(&workspace.id, serde_json::json!({ "purpose": "implement" }));
        session_a.id = "session-a".into();
        session_a.started_at = started_at;
        storage.insert_session(&session_a).await.unwrap();

        let mut task = Task::new(
            &workspace.id,
            Some(session_a.id.clone()),
            "Implement resume context",
            serde_json::json!({ "why": "continue work without transcript" }),
        );
        task.id = "task-a".into();
        task.status = crate::domain::TaskStatus::Active;
        task.created_at = started_at + Duration::minutes(5);
        task.updated_at = started_at + Duration::minutes(20);
        storage.insert_task(&task).await.unwrap();

        let mut decision = MemoryRecord::new(
            &workspace.id,
            MemoryKind::Decision,
            "Keep resume_context transport-neutral.",
        );
        decision.id = "decision-a".into();
        decision.session_id = Some(session_a.id.clone());
        decision.task_id = Some(task.id.clone());
        decision.related_paths = vec!["src/lib.rs".into()];
        decision.created_at = started_at + Duration::minutes(25);
        storage.insert_memory(&decision).await.unwrap();

        let mut resolved_failure = MemoryRecord::new(
            &workspace.id,
            MemoryKind::Failure,
            "Old failure that must not be resumed.",
        );
        resolved_failure.id = "failure-a".into();
        resolved_failure.session_id = Some(session_a.id.clone());
        resolved_failure.task_id = Some(task.id.clone());
        storage.insert_memory(&resolved_failure).await.unwrap();
        let mut solution = MemoryRecord::new(&workspace.id, MemoryKind::Solution, "Fixed it.");
        solution.id = "solution-a".into();
        storage.insert_memory(&solution).await.unwrap();
        storage
            .insert_memory_supersession(&crate::domain::MemorySupersession::new(
                &workspace.id,
                &resolved_failure.id,
                &solution.id,
            ))
            .await
            .unwrap();

        service
            .activate_source(
                &workspace.id,
                &session_a.id,
                Some(&task.id),
                &chunks[0].id,
                ContextSourceType::Code,
            )
            .await
            .unwrap();
        let mut checkpoint = Checkpoint::new(
            &workspace.id,
            &session_a.id,
            "Built the resume selection core.",
        );
        checkpoint.id = "checkpoint-a".into();
        checkpoint.task_id = Some(task.id.clone());
        checkpoint.objective = Some("Resume work without a transcript.".into());
        checkpoint.completed = vec!["Added deterministic scope selection.".into()];
        checkpoint.open_problems = vec!["Expose through CLI and MCP later.".into()];
        checkpoint.decision_ids = vec![decision.id.clone()];
        checkpoint.related_paths = vec!["src/lib.rs".into()];
        checkpoint.next_action = Some("Add transcript-free integration coverage.".into());
        service.create_checkpoint(checkpoint).await.unwrap();

        for (event_type, payload, minutes) in [
            (
                EventType::FileModified,
                serde_json::json!({ "path": "src/lib.rs" }),
                30,
            ),
            (
                EventType::FileModified,
                serde_json::json!({ "path": "src\\lib.rs" }),
                35,
            ),
            (
                EventType::FileRemoved,
                serde_json::json!({ "path": "src/old.rs" }),
                40,
            ),
        ] {
            let mut event = CortexEvent::new(&workspace.id, event_type, payload);
            event.session_id = Some(session_a.id.clone());
            event.task_id = Some(task.id.clone());
            event.created_at = started_at + Duration::minutes(minutes);
            storage.insert_event(&event).await.unwrap();
        }
        storage
            .end_session(&session_a.id, started_at + Duration::hours(2))
            .await
            .unwrap();
        clock.advance(Duration::hours(1));
        let mut session_b = Session::new(&workspace.id, serde_json::json!({ "purpose": "resume" }));
        session_b.id = "session-b".into();
        session_b.started_at = clock.now();
        storage.insert_session(&session_b).await.unwrap();

        let mut request = ResumeContextRequest::new(&workspace.id);
        request.token_budget = 4_096;
        let resume = service.resume_context(request).await.unwrap();

        assert_eq!(
            resume.session_selection,
            ResumeSessionSelection::LatestActive
        );
        assert_eq!(resume.selected_session.unwrap().id, session_b.id);
        assert_eq!(resume.task_selection, ResumeTaskSelection::WorkspaceActive);
        assert_eq!(resume.selected_task.unwrap().id, task.id);
        assert_eq!(
            resume.evidence_session_id.as_deref(),
            Some(session_a.id.as_str())
        );
        assert_eq!(resume.checkpoint.unwrap().id, "checkpoint-a");
        assert_eq!(resume.recent_changes.len(), 2);
        assert_eq!(resume.recent_changes[0].path, "src/lib.rs");
        assert_eq!(resume.recent_changes[0].change_count, 2);
        assert_eq!(resume.recent_changes[1].path, "src/old.rs");
        assert_eq!(resume.working_sets.len(), 2);
        assert!(resume.packet.estimated_tokens <= resume.packet.token_budget);
        assert!(resume.packet.items.iter().any(|item| {
            item.reasons
                .contains(&ContextSelectionReason::CurrentCheckpoint)
        }));
        let task_position = resume
            .packet
            .items
            .iter()
            .position(|item| item.source_id == task.id)
            .unwrap();
        let checkpoint_position = resume
            .packet
            .items
            .iter()
            .position(|item| item.source_id == "checkpoint-a")
            .unwrap();
        assert!(task_position < checkpoint_position);
        assert!(resume.packet.items.iter().any(|item| {
            item.content
                .contains("Keep resume_context transport-neutral")
        }));
        assert!(!resume.packet.items.iter().any(|item| {
            item.content
                .contains("Old failure that must not be resumed")
        }));
        assert!(
            resume
                .packet
                .items
                .iter()
                .any(|item| item.path.as_deref() == Some("src/lib.rs"))
        );
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
    async fn optional_explanation_mirrors_selected_items_without_budget_cost() {
        let (service, _clock, _storage, workspace, session, chunks) = fixture(10).await;
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
        let mut request = ContextRequest::new(&workspace.id);
        request.session_id = Some(session.id.clone());
        request.token_budget = 256;
        request.include_explanation = true;
        let packet = service.assemble_context_packet(request).await.unwrap();

        let explanation = packet.explanation.as_ref().unwrap();
        assert_eq!(explanation.selected.len(), packet.items.len());
        assert_eq!(
            packet.estimated_tokens,
            packet
                .items
                .iter()
                .map(|item| item.estimated_tokens)
                .sum::<usize>()
        );
        assert!(
            explanation
                .selected
                .iter()
                .any(|item| item.source_id == chunks[0].id)
        );
        assert!(
            explanation
                .selected
                .iter()
                .all(|item| item.estimated_tokens > 0)
        );
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
    async fn candidate_pool_excludes_superseded_direct_memory_matches() {
        let (service, _clock, storage, workspace, _session, _chunks) = fixture(10).await;
        let obsolete = MemoryRecord::new(
            &workspace.id,
            MemoryKind::Decision,
            "obsolete retrieval protocol",
        );
        let replacement = MemoryRecord::new(
            &workspace.id,
            MemoryKind::Decision,
            "current retrieval protocol",
        );
        storage.insert_memory(&obsolete).await.unwrap();
        storage.insert_memory(&replacement).await.unwrap();
        storage
            .insert_memory_supersession(&MemorySupersession::new(
                &workspace.id,
                &obsolete.id,
                &replacement.id,
            ))
            .await
            .unwrap();

        let mut request = ContextRequest::new(&workspace.id);
        request.query = Some("obsolete retrieval protocol".into());
        request.include_code = false;
        request.include_documents = false;
        request.include_events = false;

        let pool = service.build_candidate_pool(request).await.unwrap();

        assert!(
            pool.candidates
                .iter()
                .all(|candidate| candidate.source_id != obsolete.id)
        );
    }

    #[tokio::test]
    async fn explicit_task_context_scopes_state_to_its_session() {
        let (service, _clock, storage, workspace, session_a, _chunks) = fixture(10).await;
        let task_a = Task::new(
            &workspace.id,
            Some(session_a.id.clone()),
            "task in session A",
            serde_json::json!({}),
        );
        storage.insert_task(&task_a).await.unwrap();
        let session_b = Session::new(&workspace.id, serde_json::json!({}));
        storage.insert_session(&session_b).await.unwrap();
        let task_b = Task::new(
            &workspace.id,
            Some(session_b.id.clone()),
            "task in session B",
            serde_json::json!({}),
        );
        storage.insert_task(&task_b).await.unwrap();

        let mut request = ContextRequest::new(&workspace.id);
        request.task_id = Some(task_a.id.clone());
        request.include_code = false;
        request.include_documents = false;
        request.include_memories = false;
        request.include_events = false;

        let pool = service.build_candidate_pool(request).await.unwrap();

        assert!(pool.candidates.iter().any(|candidate| {
            candidate.source_type == ContextSourceType::TaskState
                && candidate.source_id == task_a.id
        }));
        assert!(pool.candidates.iter().any(|candidate| {
            candidate.source_type == ContextSourceType::SessionState
                && candidate.source_id == session_a.id
        }));
        assert!(pool.candidates.iter().all(|candidate| {
            candidate.source_id != task_b.id && candidate.source_id != session_b.id
        }));
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

    #[test]
    fn fixed_ranking_evaluation_covers_context_intents() {
        let mut implementation = ranking_candidate(
            "implementation",
            ContextSourceType::Code,
            ContextFreshness::Current,
        );
        implementation.scores.semantic = Some(0.9);
        let mut incidental_memory = ranking_candidate(
            "incidental-memory",
            ContextSourceType::Memory,
            ContextFreshness::Historical,
        );
        incidental_memory.scores.lexical = Some(0.2);
        assert_eq!(
            ranked_first(vec![incidental_memory, implementation]).source_id,
            "implementation"
        );

        let unrelated_code = ranking_candidate(
            "unrelated-code",
            ContextSourceType::Code,
            ContextFreshness::Current,
        );
        let mut decision = ranking_candidate(
            "historical-decision",
            ContextSourceType::Memory,
            ContextFreshness::Historical,
        );
        decision.scores.lexical = Some(1.0);
        decision
            .reasons
            .push(ContextSelectionReason::RecentDecision);
        assert_eq!(
            ranked_first(vec![unrelated_code.clone(), decision]).source_id,
            "historical-decision"
        );

        let mut recent_event = ranking_candidate(
            "recent-change",
            ContextSourceType::Event,
            ContextFreshness::Historical,
        );
        recent_event.scores.recency = 1.0;
        assert_eq!(
            ranked_first(vec![unrelated_code.clone(), recent_event]).source_id,
            "recent-change"
        );

        let mut exact_symbol = ranking_candidate(
            "exact-symbol",
            ContextSourceType::Code,
            ContextFreshness::Current,
        );
        exact_symbol.scores.lexical = Some(1.0);
        assert_eq!(
            ranked_first(vec![unrelated_code.clone(), exact_symbol]).source_id,
            "exact-symbol"
        );

        let mut resume_state = ranking_candidate(
            "resume-session",
            ContextSourceType::SessionState,
            ContextFreshness::Historical,
        );
        resume_state.scores.task = 0.8;
        resume_state.scores.recency = 0.5;
        resume_state
            .reasons
            .push(ContextSelectionReason::ResumeState);
        assert_eq!(
            ranked_first(vec![unrelated_code, resume_state]).source_id,
            "resume-session"
        );

        let mut rust = ranking_candidate(
            "rust-result",
            ContextSourceType::Code,
            ContextFreshness::Current,
        );
        rust.language = Some("rust".into());
        rust.scores.semantic = Some(0.8);
        let mut typescript = ranking_candidate(
            "typescript-result",
            ContextSourceType::Code,
            ContextFreshness::Current,
        );
        typescript.language = Some("typescript".into());
        typescript.scores.semantic = Some(0.9);
        assert_eq!(
            ranked_first(vec![rust, typescript]).source_id,
            "typescript-result"
        );
    }

    #[tokio::test]
    async fn memory_query_relevance_is_ranked_and_recorded() {
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        let workspace = Workspace::new("C:/memory-ranking", "memory-ranking");
        storage.insert_workspace(&workspace).await.unwrap();
        let document = Document::new(&workspace.id, "src/unrelated.rs");
        storage.insert_document(&document).await.unwrap();
        let chunk = StoredChunk::new(&document.id, "unrelated", "fn unrelated() {}");
        storage.insert_chunk(&chunk).await.unwrap();
        let mut decision = MemoryRecord::new(
            &workspace.id,
            MemoryKind::Decision,
            "BLAKE3 identifies stable source chunks",
        );
        decision.created_at = Utc.with_ymd_and_hms(2026, 8, 24, 12, 0, 0).unwrap();
        storage.insert_memory(&decision).await.unwrap();
        let service = ContextService::with_clock(
            Arc::clone(&storage),
            test_retrieval(Arc::clone(&storage)),
            WorkingSetConfig::default(),
            TemporalConfig::default(),
            ContextConfig::default(),
            Arc::new(TestClock::new(
                Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap(),
            )),
        )
        .unwrap();
        let mut request = ContextRequest::new(&workspace.id);
        request.query = Some("BLAKE3 stable chunks".into());

        let pool = service.build_candidate_pool(request).await.unwrap();

        assert_eq!(pool.candidates[0].source_id, decision.id);
        assert_eq!(pool.candidates[0].scores.lexical, Some(1.0));
        assert_eq!(pool.candidates[0].scores.provenance, 0.9);
        assert_eq!(pool.candidates[0].scores.freshness, 0.25);
        assert!(pool.candidates[0].scores.final_score > 0.0);
        assert!(
            pool.candidates[0]
                .reasons
                .contains(&ContextSelectionReason::RecentDecision)
        );
    }

    #[tokio::test]
    async fn candidate_pool_expands_persisted_structural_relations() {
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        let workspace = Workspace::new("C:/structural-candidates", "structural-candidates");
        storage.insert_workspace(&workspace).await.unwrap();
        let document = Document::new(&workspace.id, "src/engine.rs");
        storage.insert_document(&document).await.unwrap();

        let mut container = StoredChunk::new(
            &document.id,
            "src/engine.rs::struct:Engine",
            "struct Engine { search update }",
        );
        container.language = "rust".into();
        container.symbol = Some("Engine".into());
        container.qualified_symbol = Some("Engine".into());
        container.start_byte = 0;
        container.end_byte = 31;

        let mut search = StoredChunk::new(
            &document.id,
            "src/engine.rs::struct:Engine::method:search",
            "fn search(&self) {}",
        );
        search.language = "rust".into();
        search.symbol = Some("search".into());
        search.qualified_symbol = Some("Engine.search".into());
        search.start_byte = 8;
        search.end_byte = 18;
        search.metadata = serde_json::json!({
            "parent_stable_key": container.stable_key,
            "container_symbol": "Engine",
            "structural_depth": 1,
        });

        let mut update = StoredChunk::new(
            &document.id,
            "src/engine.rs::struct:Engine::method:update",
            "fn update(&mut self) {}",
        );
        update.language = "rust".into();
        update.symbol = Some("update".into());
        update.qualified_symbol = Some("Engine.update".into());
        update.start_byte = 19;
        update.end_byte = 30;
        update.metadata = serde_json::json!({
            "parent_stable_key": container.stable_key,
            "container_symbol": "Engine",
            "structural_depth": 1,
        });

        for chunk in [&container, &search, &update] {
            storage.insert_chunk(chunk).await.unwrap();
        }
        let service = ContextService::with_clock(
            Arc::clone(&storage),
            test_retrieval(Arc::clone(&storage)),
            WorkingSetConfig::default(),
            TemporalConfig::default(),
            ContextConfig::default(),
            Arc::new(TestClock::new(
                Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap(),
            )),
        )
        .unwrap();

        let mut request = ContextRequest::new(&workspace.id);
        request.query = Some("search".into());
        let pool = service.build_candidate_pool(request).await.unwrap();

        assert!(pool.candidates.iter().any(|candidate| {
            candidate.source_id == container.id
                && candidate
                    .reasons
                    .contains(&ContextSelectionReason::ContainerOfRelevantSymbol)
        }));
        assert!(pool.candidates.iter().any(|candidate| {
            candidate
                .reasons
                .contains(&ContextSelectionReason::NeighborOfRelevantSymbol)
        }));
        assert!(pool.candidates.iter().any(|candidate| {
            candidate
                .reasons
                .contains(&ContextSelectionReason::RelatedSymbol)
        }));
    }

    #[tokio::test]
    async fn normalized_structure_expands_across_language_families() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/multi-language-structure", "multi-language-structure");
        storage.insert_workspace(&workspace).await.unwrap();
        let cases: Vec<(&dyn LanguageAnalyzer, &str, &str, &str)> = vec![
            (
                &RustAnalyzer,
                "src/engine.rs",
                "struct Engine; impl Engine { fn before(&self) {} fn search(&self) {} fn after(&self) {} fn extra(&self) {} }",
                "search",
            ),
            (
                &PythonAnalyzer,
                "engine.py",
                "class Engine:\n    def before(self): pass\n    def search(self): pass\n    def after(self): pass\n    def extra(self): pass\n",
                "search",
            ),
            (
                &TypeScriptAnalyzer,
                "engine.ts",
                "class Engine { before(): void {} search(): void {} after(): void {} extra(): void {} }",
                "search",
            ),
            (
                &CSharpAnalyzer,
                "Engine.cs",
                "class Engine { void Before() {} void Search() {} void After() {} void Extra() {} }",
                "Search",
            ),
        ];

        for (analyzer, path, source, method) in cases {
            let document = Document::new(&workspace.id, path);
            storage.insert_document(&document).await.unwrap();
            let chunks: Vec<_> = analyzer
                .analyze(std::path::Path::new(path), source)
                .unwrap()
                .chunks
                .into_iter()
                .map(|chunk| stored_chunk(&document.id, chunk))
                .collect();
            let method_id = chunks
                .iter()
                .find(|chunk| chunk.symbol.as_deref() == Some(method))
                .unwrap_or_else(|| panic!("{path} did not produce {method}: {chunks:#?}"))
                .id
                .clone();
            for chunk in &chunks {
                storage.insert_chunk(chunk).await.unwrap();
            }

            let expanded = storage
                .structural_code_candidates(&workspace.id, &method_id, 4)
                .await
                .unwrap();
            assert_eq!(
                expanded
                    .iter()
                    .map(|candidate| candidate.relation)
                    .collect::<Vec<_>>(),
                vec![
                    StructuralRelation::Container,
                    StructuralRelation::Neighbor,
                    StructuralRelation::Neighbor,
                    StructuralRelation::Related,
                ],
                "unexpected structural expansion for {path}",
            );
            assert_eq!(expanded[0].candidate.symbol.as_deref(), Some("Engine"));
        }
    }

    #[tokio::test]
    async fn structural_expansion_resolves_segmented_logical_containers() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/segmented-structure", "segmented-structure");
        storage.insert_workspace(&workspace).await.unwrap();
        let document = Document::new(&workspace.id, "src/large.rs");
        storage.insert_document(&document).await.unwrap();

        let logical_key = "src/large.rs::struct:Large";
        let mut container = StoredChunk::new(
            &document.id,
            format!("{logical_key}::segment:0"),
            "struct Large { ... }",
        );
        container.language = "rust".into();
        container.symbol = Some("Large".into());
        container.start_byte = 0;
        container.end_byte = 100;
        container.metadata = serde_json::json!({
            "parent_logical_stable_key": logical_key,
            "parent_stable_key": null,
            "ordinal_in_container": 0,
        });

        let mut method = StoredChunk::new(
            &document.id,
            format!("{logical_key}::method:search"),
            "fn search(&self) {}",
        );
        method.language = "rust".into();
        method.symbol = Some("search".into());
        method.start_byte = 20;
        method.end_byte = 40;
        method.metadata = serde_json::json!({
            "parent_stable_key": logical_key,
            "ordinal_in_container": 0,
        });
        storage.insert_chunk(&container).await.unwrap();
        storage.insert_chunk(&method).await.unwrap();

        let method_expansion = storage
            .structural_code_candidates(&workspace.id, &method.id, 4)
            .await
            .unwrap();
        assert_eq!(method_expansion[0].relation, StructuralRelation::Container);
        assert_eq!(method_expansion[0].candidate.source_id, container.id);

        let container_expansion = storage
            .structural_code_candidates(&workspace.id, &container.id, 4)
            .await
            .unwrap();
        assert!(container_expansion.iter().any(|candidate| {
            candidate.relation == StructuralRelation::Related
                && candidate.candidate.source_id == method.id
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
                ..ContextConfig::default()
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

    #[tokio::test]
    async fn bounded_pool_keeps_active_task_through_context_assembly() {
        let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
        let workspace = Workspace::new("C:/required-task-budget", "required-task-budget");
        storage.insert_workspace(&workspace).await.unwrap();
        let session = Session::new(&workspace.id, serde_json::json!({}));
        storage.insert_session(&session).await.unwrap();
        let document = Document::new(&workspace.id, "src/needle.rs");
        storage.insert_document(&document).await.unwrap();
        for index in 0..8 {
            let chunk = StoredChunk::new(
                &document.id,
                format!("needle-{index}"),
                format!("fn needle_{index}() {{}}"),
            );
            storage.insert_chunk(&chunk).await.unwrap();
        }
        let task = Task::new(
            &workspace.id,
            Some(session.id.clone()),
            "mandatory task state",
            serde_json::json!({}),
        );
        storage.insert_task(&task).await.unwrap();
        let service = ContextService::with_clock(
            Arc::clone(&storage),
            test_retrieval(Arc::clone(&storage)),
            WorkingSetConfig::default(),
            TemporalConfig::default(),
            ContextConfig {
                candidate_pool_limit: 1,
                ..ContextConfig::default()
            },
            Arc::new(TestClock::new(
                Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap(),
            )),
        )
        .unwrap();
        let mut request = ContextRequest::new(&workspace.id);
        request.query = Some("needle".into());
        request.session_id = Some(session.id);
        request.task_id = Some(task.id.clone());
        request.include_documents = false;
        request.include_memories = false;
        request.include_events = false;
        request.token_budget = 64;

        let pool = service.build_candidate_pool(request.clone()).await.unwrap();
        assert_eq!(pool.candidates.len(), 1);
        assert_eq!(pool.candidates[0].source_id, task.id);

        let packet = service.assemble_context_packet(request).await.unwrap();
        assert_eq!(packet.items.len(), 1);
        assert_eq!(packet.items[0].source_id, task.id);
        assert!(packet.estimated_tokens <= packet.token_budget);
    }

    #[test]
    fn token_budget_keeps_required_task_state_with_tiny_limits() {
        let mut task = budget_candidate(
            "active-task",
            ContextSourceType::TaskState,
            "task state must remain available",
            0.01,
        );
        task.reasons
            .push(ContextSelectionReason::ActiveTaskReference);
        let source = budget_candidate(
            "expensive-source",
            ContextSourceType::Code,
            &"x".repeat(1_000),
            1.0,
        );
        let counter = ConservativeByteCounter;

        for budget in [2, 4, 8, 16] {
            let packet = build_context_packet(
                budget_pool(vec![task.clone(), source.clone()]),
                budget,
                &ContextBudgetConfig::default(),
                &counter,
            );
            assert!(packet.estimated_tokens <= budget);
            assert_eq!(packet.items.len(), 1);
            assert_eq!(packet.items[0].source_id, "active-task");
            assert_eq!(packet.items[0].estimated_tokens, budget);
            assert!(packet.items[0].truncated);
        }
    }

    #[test]
    fn bounded_candidate_retention_preserves_required_items_before_selection() {
        let mut task = budget_candidate("active-task", ContextSourceType::TaskState, "task", 0.01);
        task.reasons
            .push(ContextSelectionReason::ActiveTaskReference);
        let mut pin = budget_candidate(
            "pinned-decision",
            ContextSourceType::Memory,
            "decision",
            0.01,
        );
        pin.reasons.push(ContextSelectionReason::Pinned);
        let ordinary = (0..64).map(|index| {
            budget_candidate(
                &format!("high-value-{index}"),
                ContextSourceType::Code,
                "high value",
                1.0,
            )
        });
        let mut candidates = vec![task, pin].into_iter().chain(ordinary).collect();

        retain_bounded_candidates(&mut candidates, 2);

        assert_eq!(candidates.len(), 2);
        assert!(
            candidates
                .iter()
                .any(|item| item.source_id == "active-task")
        );
        assert!(
            candidates
                .iter()
                .any(|item| item.source_id == "pinned-decision")
        );

        retain_bounded_candidates(&mut candidates, 1);
        assert_eq!(candidates[0].source_id, "active-task");
    }

    #[test]
    fn bounded_candidate_retention_keeps_structural_evidence_without_displacing_required_items() {
        let mut task = budget_candidate("active-task", ContextSourceType::TaskState, "task", 0.01);
        task.reasons
            .push(ContextSelectionReason::ActiveTaskReference);
        let ordinary = budget_candidate("ordinary", ContextSourceType::Code, "ordinary", 1.0);
        let mut structural = budget_candidate("structural", ContextSourceType::Code, "linked", 0.1);
        structural
            .structural_evidence
            .push(structural_evidence(&structural.source_id));
        let mut candidates = vec![task, ordinary, structural];

        retain_bounded_candidates(&mut candidates, 2);

        assert_eq!(candidates.len(), 2);
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.source_id == "active-task")
        );
        assert!(
            candidates
                .iter()
                .any(|candidate| !candidate.structural_evidence.is_empty())
        );
    }

    #[test]
    fn token_budget_counts_empty_content_with_tokenizer_overhead() {
        let mut task = budget_candidate("empty-task", ContextSourceType::TaskState, "", 0.01);
        task.reasons
            .push(ContextSelectionReason::ActiveTaskReference);
        let config = ContextBudgetConfig::default();

        let too_small = build_context_packet(
            budget_pool(vec![task.clone()]),
            1,
            &config,
            &OverheadTokenCounter,
        );
        assert!(too_small.items.is_empty());
        assert_eq!(too_small.estimated_tokens, 0);

        let exact_fit =
            build_context_packet(budget_pool(vec![task]), 2, &config, &OverheadTokenCounter);
        assert_eq!(exact_fit.items.len(), 1);
        assert_eq!(exact_fit.items[0].estimated_tokens, 2);
        assert_eq!(exact_fit.estimated_tokens, 2);
    }

    #[test]
    fn token_budget_torture_case_stays_bounded_and_avoids_code_overlap() {
        let mut task = budget_candidate(
            "active-task",
            ContextSourceType::TaskState,
            "task-state",
            0.01,
        );
        task.reasons
            .push(ContextSelectionReason::ActiveTaskReference);
        let mut pin = budget_candidate("pinned-memory", ContextSourceType::Memory, "pin", 0.01);
        pin.reasons.push(ContextSelectionReason::Pinned);

        let mut focused_code =
            budget_candidate("focused-code", ContextSourceType::Code, "needle", 0.99);
        focused_code.path = Some("src/search.rs".into());
        let mut enclosing_code = budget_candidate(
            "enclosing-code",
            ContextSourceType::Code,
            "prefix needle suffix with surrounding implementation details",
            0.30,
        );
        enclosing_code.path = Some("src/search.rs".into());
        let huge_source = budget_candidate(
            "huge-source",
            ContextSourceType::Code,
            &"z".repeat(1_000),
            1.0,
        );

        let high_value_code = (0..24).map(|index| {
            let mut candidate = budget_candidate(
                &format!("high-value-code-{index}"),
                ContextSourceType::Code,
                &format!("chunk-{index}"),
                0.9,
            );
            candidate.path = Some(format!("src/high_value_{index}.rs"));
            candidate
        });
        let memories = (0..24).map(|index| {
            budget_candidate(
                &format!("memory-{index}"),
                ContextSourceType::Memory,
                "memo",
                0.6,
            )
        });
        let events = (0..8).map(|index| {
            budget_candidate(
                &format!("event-{index}"),
                ContextSourceType::Event,
                "event",
                0.5,
            )
        });
        let candidates = vec![task, pin, focused_code, enclosing_code, huge_source]
            .into_iter()
            .chain(high_value_code)
            .chain(memories)
            .chain(events)
            .collect();
        let counter = ConservativeByteCounter;
        let packet = build_context_packet(
            budget_pool(candidates),
            96,
            &ContextBudgetConfig::default(),
            &counter,
        );

        assert!(packet.estimated_tokens <= packet.token_budget);
        assert_eq!(
            packet.estimated_tokens,
            packet
                .items
                .iter()
                .map(|item| item.estimated_tokens)
                .sum::<usize>()
        );
        assert!(
            packet
                .items
                .iter()
                .any(|item| item.source_id == "active-task")
        );
        assert!(
            packet
                .items
                .iter()
                .any(|item| item.source_id == "pinned-memory")
        );
        assert!(
            packet
                .items
                .iter()
                .any(|item| item.source_id == "focused-code")
        );
        assert!(
            !packet
                .items
                .iter()
                .any(|item| item.source_id == "enclosing-code" || item.source_id == "huge-source")
        );
        assert!(
            packet
                .items
                .iter()
                .filter(|item| item.source_type == ContextSourceType::Memory)
                .count()
                > 1
        );
        assert!(
            packet
                .items
                .iter()
                .filter(|item| item.source_type == ContextSourceType::Code)
                .count()
                > 1
        );
    }
}
