use super::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum DecisionJournalPhase {
    Prepared,
    #[default]
    Committed,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DecisionJournalRecord {
    event_id: String,
    kind: String,
    subject_id: String,
    command_id: String,
    occurred_at: String,
    #[serde(default, skip_serializing_if = "is_committed")]
    phase: DecisionJournalPhase,
}

fn is_committed(phase: &DecisionJournalPhase) -> bool {
    *phase == DecisionJournalPhase::Committed
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchTakeSelection {
    pub shot_id: String,
    pub take_id: String,
    #[serde(default)]
    pub accept_warnings: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRecord {
    pub event_id: String,
    pub kind: String,
    pub subject_id: String,
    pub command_id: String,
    pub occurred_at: String,
}

impl ProjectStore {
    pub fn decision_history(&self, limit: u32) -> Result<Vec<DecisionRecord>, StoreError> {
        if !(1..=1_000).contains(&limit) {
            return Err(StoreError::InvalidHistoryLimit(limit));
        }
        let _lock = self.lock()?;
        let state = self.read_state()?;
        self.recover_decisions_for_state(&state)?;
        let mut decisions = self.committed_decisions()?;
        decisions.reverse();
        decisions.truncate(limit as usize);
        Ok(decisions)
    }

    pub fn recover_decision_journal(&self) -> Result<(), StoreError> {
        let _lock = self.lock()?;
        let state = self.read_state()?;
        self.recover_decisions_for_state(&state)
    }

    pub fn review_takes_batch(
        &self,
        selections: &[BatchTakeSelection],
        approve: bool,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        validate_review_batch(&state, selections, approve)?;

        for selection in selections {
            let shot = state
                .shots
                .get_mut(&selection.shot_id)
                .ok_or_else(|| StoreError::ShotNotFound(selection.shot_id.clone()))?;
            shot.selected_candidate_take_id = Some(selection.take_id.clone());
            shot.approved_take_id = approve.then(|| selection.take_id.clone());
            shot.audition_target_takes = None;
            shot.stage = if approve {
                ShotStage::Approved
            } else {
                ShotStage::Selected
            };
            state.pending_approvals.retain(|approval| {
                approval.kind != ApprovalKind::CandidateSelection
                    || approval.shot_id.as_deref() != Some(&selection.shot_id)
            });
            self.mark_builds_stale_for_decision(
                &mut state,
                &selection.shot_id,
                Some(&selection.take_id),
            );
        }
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        let kind = if approve {
            "take_approved_batch"
        } else {
            "take_selected_batch"
        };
        let decisions = self.prepare_decisions(
            selections
                .iter()
                .map(|selection| (kind, selection.take_id.as_str())),
            command_id,
            now,
        )?;
        self.save_state(&state, expected_revision)?;
        self.commit_decisions(&decisions)?;
        Ok(state)
    }

    pub fn set_project_paused(
        &self,
        paused: bool,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        state.paused = paused;
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        let decision = self.prepare_decision(
            if paused {
                "project_paused"
            } else {
                "project_resumed"
            },
            &state.project_id,
            command_id,
            now,
        )?;
        self.save_state(&state, expected_revision)?;
        self.commit_decisions(&[decision])?;
        Ok(state)
    }

    pub fn select_take(
        &self,
        shot_id: &str,
        take_id: &str,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        let shot = state
            .shots
            .get(shot_id)
            .ok_or_else(|| StoreError::ShotNotFound(shot_id.to_owned()))?;
        if let Some(job_id) = &shot.active_job_id {
            return Err(StoreError::ShotBusy {
                shot_id: shot_id.to_owned(),
                job_id: job_id.clone(),
            });
        }
        if shot.approved_take_id.is_some() {
            return Err(StoreError::ShotAlreadyApproved(shot_id.to_owned()));
        }
        let take = state
            .takes
            .get(take_id)
            .ok_or_else(|| StoreError::TakeNotFound(take_id.to_owned()))?;
        if take.shot_id != shot_id {
            return Err(StoreError::TakeShotMismatch {
                take_id: take_id.to_owned(),
                shot_id: shot_id.to_owned(),
            });
        }
        if take.stale {
            return Err(StoreError::TakeStale(take_id.to_owned()));
        }
        let shot = state
            .shots
            .get_mut(shot_id)
            .ok_or_else(|| StoreError::ShotNotFound(shot_id.to_owned()))?;
        if !shot.take_ids.iter().any(|candidate| candidate == take_id) {
            return Err(StoreError::TakeUnavailable(take_id.to_owned()));
        }
        if shot
            .rejected_take_ids
            .iter()
            .any(|rejected| rejected == take_id)
        {
            return Err(StoreError::TakeRejected(take_id.to_owned()));
        }
        shot.selected_candidate_take_id = Some(take_id.to_owned());
        shot.audition_target_takes = None;
        shot.stage = ShotStage::Selected;
        state.pending_approvals.retain(|approval| {
            approval.kind != ApprovalKind::CandidateSelection
                || approval.shot_id.as_deref() != Some(shot_id)
        });
        self.mark_builds_stale_for_decision(&mut state, shot_id, Some(take_id));
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        let decision = self.prepare_decision("take_selected", take_id, command_id, now)?;
        self.save_state(&state, expected_revision)?;
        self.commit_decisions(&[decision])?;
        Ok(state)
    }

    pub fn approve_take(
        &self,
        shot_id: &str,
        take_id: &str,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        let shot = state
            .shots
            .get(shot_id)
            .ok_or_else(|| StoreError::ShotNotFound(shot_id.to_owned()))?;
        if let Some(job_id) = &shot.active_job_id {
            return Err(StoreError::ShotBusy {
                shot_id: shot_id.to_owned(),
                job_id: job_id.clone(),
            });
        }
        if shot.approved_take_id.is_some() {
            return Err(StoreError::ShotAlreadyApproved(shot_id.to_owned()));
        }
        let take = state
            .takes
            .get(take_id)
            .ok_or_else(|| StoreError::TakeNotFound(take_id.to_owned()))?;
        if take.shot_id != shot_id {
            return Err(StoreError::TakeShotMismatch {
                take_id: take_id.to_owned(),
                shot_id: shot_id.to_owned(),
            });
        }
        if take.stale {
            return Err(StoreError::TakeStale(take_id.to_owned()));
        }
        let shot = state
            .shots
            .get_mut(shot_id)
            .ok_or_else(|| StoreError::ShotNotFound(shot_id.to_owned()))?;
        if !shot.take_ids.iter().any(|candidate| candidate == take_id) {
            return Err(StoreError::TakeUnavailable(take_id.to_owned()));
        }
        if shot
            .rejected_take_ids
            .iter()
            .any(|rejected| rejected == take_id)
        {
            return Err(StoreError::TakeRejected(take_id.to_owned()));
        }
        if shot.selected_candidate_take_id.as_deref() != Some(take_id) {
            return Err(StoreError::TakeNotSelected(take_id.to_owned()));
        }
        shot.approved_take_id = Some(take_id.to_owned());
        shot.audition_target_takes = None;
        shot.stage = ShotStage::Approved;
        state.pending_approvals.retain(|approval| {
            approval.kind != ApprovalKind::CandidateSelection
                || approval.shot_id.as_deref() != Some(shot_id)
        });
        self.mark_builds_stale_for_decision(&mut state, shot_id, Some(take_id));
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        let decision = self.prepare_decision("take_approved", take_id, command_id, now)?;
        self.save_state(&state, expected_revision)?;
        self.commit_decisions(&[decision])?;
        Ok(state)
    }

    pub fn reject_take(
        &self,
        shot_id: &str,
        take_id: &str,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        let shot = state
            .shots
            .get(shot_id)
            .ok_or_else(|| StoreError::ShotNotFound(shot_id.to_owned()))?;
        if let Some(job_id) = &shot.active_job_id {
            return Err(StoreError::ShotBusy {
                shot_id: shot_id.to_owned(),
                job_id: job_id.clone(),
            });
        }
        if shot.approved_take_id.is_some() {
            return Err(StoreError::ShotAlreadyApproved(shot_id.to_owned()));
        }
        let take = state
            .takes
            .get(take_id)
            .ok_or_else(|| StoreError::TakeNotFound(take_id.to_owned()))?;
        if take.shot_id != shot_id {
            return Err(StoreError::TakeShotMismatch {
                take_id: take_id.to_owned(),
                shot_id: shot_id.to_owned(),
            });
        }
        if take.stale {
            return Err(StoreError::TakeStale(take_id.to_owned()));
        }
        let shot = state
            .shots
            .get_mut(shot_id)
            .ok_or_else(|| StoreError::ShotNotFound(shot_id.to_owned()))?;
        if !shot.take_ids.iter().any(|candidate| candidate == take_id) {
            return Err(StoreError::TakeUnavailable(take_id.to_owned()));
        }
        if shot
            .rejected_take_ids
            .iter()
            .any(|rejected| rejected == take_id)
        {
            return Err(StoreError::TakeRejected(take_id.to_owned()));
        }
        shot.rejected_take_ids.push(take_id.to_owned());
        if shot.selected_candidate_take_id.as_deref() == Some(take_id) {
            shot.selected_candidate_take_id = None;
        }
        if shot.approved_take_id.as_deref() == Some(take_id) {
            shot.approved_take_id = None;
        }
        shot.audition_target_takes = None;
        let all_rejected = shot
            .take_ids
            .iter()
            .all(|candidate| shot.rejected_take_ids.contains(candidate));
        shot.stage = if all_rejected {
            ShotStage::Pending
        } else {
            ShotStage::CandidatesReady
        };
        for approval in &mut state.pending_approvals {
            if approval.kind == ApprovalKind::CandidateSelection
                && approval.shot_id.as_deref() == Some(shot_id)
            {
                approval.take_ids.retain(|candidate| candidate != take_id);
            }
        }
        state.pending_approvals.retain(|approval| {
            approval.kind != ApprovalKind::CandidateSelection
                || approval.shot_id.as_deref() != Some(shot_id)
                || !approval.take_ids.is_empty()
        });
        let current_take_id = state.shots[shot_id].selected_candidate_take_id.clone();
        self.mark_builds_stale_for_decision(&mut state, shot_id, current_take_id.as_deref());
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        let decision = self.prepare_decision("take_rejected", take_id, command_id, now)?;
        self.save_state(&state, expected_revision)?;
        self.commit_decisions(&[decision])?;
        Ok(state)
    }

    pub(super) fn prepare_decision(
        &self,
        kind: &str,
        subject_id: &str,
        command_id: &str,
        now: &str,
    ) -> Result<DecisionJournalRecord, StoreError> {
        let decision = DecisionJournalRecord {
            event_id: format!("DEC-{}", Ulid::new()),
            kind: kind.to_owned(),
            subject_id: subject_id.to_owned(),
            command_id: command_id.to_owned(),
            occurred_at: now.to_owned(),
            phase: DecisionJournalPhase::Prepared,
        };
        append_jsonl(&self.root.join("decisions.jsonl"), &decision)?;
        Ok(decision)
    }

    pub(super) fn prepare_decisions<'a>(
        &self,
        decisions: impl IntoIterator<Item = (&'a str, &'a str)>,
        command_id: &str,
        now: &str,
    ) -> Result<Vec<DecisionJournalRecord>, StoreError> {
        decisions
            .into_iter()
            .map(|(kind, subject_id)| self.prepare_decision(kind, subject_id, command_id, now))
            .collect()
    }

    pub(super) fn commit_decisions(
        &self,
        decisions: &[DecisionJournalRecord],
    ) -> Result<(), StoreError> {
        for decision in decisions {
            let mut committed = decision.clone();
            committed.phase = DecisionJournalPhase::Committed;
            append_jsonl(&self.root.join("decisions.jsonl"), &committed)?;
        }
        Ok(())
    }

    pub(super) fn recover_decisions_for_state(
        &self,
        state: &ProjectState,
    ) -> Result<(), StoreError> {
        let Some(command_id) = state.last_command_id.as_deref() else {
            return Ok(());
        };
        let records =
            crate::store::read_jsonl::<DecisionJournalRecord>(&self.root.join("decisions.jsonl"))?;
        let mut latest = std::collections::HashMap::new();
        for record in records {
            latest.insert(record.event_id.clone(), record);
        }
        let pending = latest
            .into_values()
            .filter(|record| {
                record.phase == DecisionJournalPhase::Prepared && record.command_id == command_id
            })
            .collect::<Vec<_>>();
        self.commit_decisions(&pending)
    }

    fn committed_decisions(&self) -> Result<Vec<DecisionRecord>, StoreError> {
        let records =
            crate::store::read_jsonl::<DecisionJournalRecord>(&self.root.join("decisions.jsonl"))?;
        let mut order = Vec::new();
        let mut latest = std::collections::HashMap::new();
        for record in records {
            if !latest.contains_key(&record.event_id) {
                order.push(record.event_id.clone());
            }
            latest.insert(record.event_id.clone(), record);
        }
        Ok(order
            .into_iter()
            .filter_map(|event_id| latest.remove(&event_id))
            .filter(|record| record.phase == DecisionJournalPhase::Committed)
            .map(|record| DecisionRecord {
                event_id: record.event_id,
                kind: record.kind,
                subject_id: record.subject_id,
                command_id: record.command_id,
                occurred_at: record.occurred_at,
            })
            .collect())
    }
}

fn validate_review_batch(
    state: &ProjectState,
    selections: &[BatchTakeSelection],
    approve: bool,
) -> Result<(), StoreError> {
    if selections.is_empty() {
        return Err(StoreError::InvalidReviewBatch(
            "at least one selection is required".to_owned(),
        ));
    }
    if selections.len() > 1_000 {
        return Err(StoreError::InvalidReviewBatch(
            "at most 1000 selections are allowed".to_owned(),
        ));
    }
    let mut shot_ids = HashSet::new();
    for selection in selections {
        if !shot_ids.insert(selection.shot_id.as_str()) {
            return Err(StoreError::InvalidReviewBatch(format!(
                "shot `{}` appears more than once",
                selection.shot_id
            )));
        }
        let shot = state
            .shots
            .get(&selection.shot_id)
            .ok_or_else(|| StoreError::ShotNotFound(selection.shot_id.clone()))?;
        if let Some(job_id) = &shot.active_job_id {
            return Err(StoreError::ShotBusy {
                shot_id: selection.shot_id.clone(),
                job_id: job_id.clone(),
            });
        }
        if shot.approved_take_id.is_some() {
            return Err(StoreError::ShotAlreadyApproved(selection.shot_id.clone()));
        }
        let take = state
            .takes
            .get(&selection.take_id)
            .ok_or_else(|| StoreError::TakeNotFound(selection.take_id.clone()))?;
        if take.shot_id != selection.shot_id {
            return Err(StoreError::TakeShotMismatch {
                take_id: selection.take_id.clone(),
                shot_id: selection.shot_id.clone(),
            });
        }
        if take.stale {
            return Err(StoreError::TakeStale(selection.take_id.clone()));
        }
        if !shot.take_ids.contains(&selection.take_id) {
            return Err(StoreError::TakeUnavailable(selection.take_id.clone()));
        }
        if shot.rejected_take_ids.contains(&selection.take_id) {
            return Err(StoreError::TakeRejected(selection.take_id.clone()));
        }
        if approve && !take.warnings.is_empty() && !selection.accept_warnings {
            return Err(StoreError::ReviewWarningsNotAccepted(
                selection.take_id.clone(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, ProjectStore) {
        let directory = tempfile::tempdir().unwrap();
        let store =
            ProjectStore::create(directory.path(), "demo", "Demo", "Brief", "create", "100")
                .unwrap();
        (directory, store)
    }

    #[test]
    fn prepared_decision_without_state_change_is_hidden() {
        let (_directory, store) = store();
        store
            .prepare_decision("project_paused", "demo", "pause", "101")
            .unwrap();

        assert!(store.decision_history(10).unwrap().is_empty());
    }

    #[test]
    fn prepared_decision_is_recovered_after_matching_state_commit() {
        let (_directory, store) = store();
        store
            .prepare_decision("project_paused", "demo", "pause", "101")
            .unwrap();
        let mut state = store.read_state().unwrap();
        state.paused = true;
        state.last_command_id = Some("pause".to_owned());
        state.bump_revision("101".to_owned()).unwrap();
        store.save_state(&state, 1).unwrap();

        let decisions = store.decision_history(10).unwrap();

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].command_id, "pause");
        assert_eq!(decisions[0].kind, "project_paused");
    }

    #[test]
    fn duplicate_commits_are_deduplicated_by_event_id() {
        let (_directory, store) = store();
        let decision = store
            .prepare_decision("project_paused", "demo", "pause", "101")
            .unwrap();
        store
            .commit_decisions(std::slice::from_ref(&decision))
            .unwrap();
        store.commit_decisions(&[decision]).unwrap();

        let decisions = store.decision_history(10).unwrap();

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].kind, "project_paused");
    }

    #[test]
    fn recovery_completes_every_decision_in_a_batch() {
        let (_directory, store) = store();
        let decisions = store
            .prepare_decisions(
                [
                    ("take_selected_batch", "TAKE-01"),
                    ("take_selected_batch", "TAKE-02"),
                ],
                "batch",
                "101",
            )
            .unwrap();
        store
            .commit_decisions(std::slice::from_ref(&decisions[0]))
            .unwrap();
        let mut state = store.read_state().unwrap();
        state.last_command_id = Some("batch".to_owned());
        state.bump_revision("101".to_owned()).unwrap();
        store.save_state(&state, 1).unwrap();

        let history = store.decision_history(10).unwrap();

        assert_eq!(history.len(), 2);
        assert_eq!(
            history
                .iter()
                .map(|decision| decision.subject_id.as_str())
                .collect::<Vec<_>>(),
            ["TAKE-02", "TAKE-01"]
        );
    }

    #[test]
    fn legacy_decision_records_default_to_committed() {
        let (_directory, store) = store();
        append_jsonl(
            &store.root.join("decisions.jsonl"),
            &DecisionRecord {
                event_id: "DEC-legacy".to_owned(),
                kind: "take_selected".to_owned(),
                subject_id: "TAKE-01".to_owned(),
                command_id: "legacy".to_owned(),
                occurred_at: "099".to_owned(),
            },
        )
        .unwrap();

        let history = store.decision_history(10).unwrap();

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].event_id, "DEC-legacy");
    }
}
