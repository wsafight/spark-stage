use super::*;

impl ProjectStore {
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
        shot.stage = ShotStage::Selected;
        state.pending_approvals.retain(|approval| {
            approval.kind != ApprovalKind::CandidateSelection
                || approval.shot_id.as_deref() != Some(shot_id)
        });
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        self.save_state(&state, expected_revision)?;
        self.append_decision("take_selected", take_id, command_id, now)?;
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
        shot.stage = ShotStage::Approved;
        state.pending_approvals.retain(|approval| {
            approval.kind != ApprovalKind::CandidateSelection
                || approval.shot_id.as_deref() != Some(shot_id)
        });
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        self.save_state(&state, expected_revision)?;
        self.append_decision("take_approved", take_id, command_id, now)?;
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
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        self.save_state(&state, expected_revision)?;
        self.append_decision("take_rejected", take_id, command_id, now)?;
        Ok(state)
    }
}
