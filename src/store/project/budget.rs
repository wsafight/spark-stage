use super::*;
use crate::domain::{BudgetContract, BudgetOverrun};

impl ProjectStore {
    #[allow(clippy::too_many_arguments)]
    pub fn request_budget_overrun(
        &self,
        scope: &str,
        shot_id: &str,
        operation: &str,
        dimensions: Vec<String>,
        reasons: Vec<String>,
        incremental_wall_seconds: u64,
        incremental_disk_bytes: u64,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<(ProjectState, Approval), StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        if let Some(overrun) = state
            .budget
            .overruns
            .values()
            .find(|overrun| overrun.scope == scope && overrun.approved_at.is_none())
        {
            let approval = state
                .pending_approvals
                .iter()
                .find(|approval| approval.approval_id == overrun.approval_id)
                .cloned()
                .ok_or_else(|| StoreError::ApprovalNotFound(overrun.approval_id.clone()))?;
            return Ok((state, approval));
        }

        let approval_id = format!("APR-{}", Ulid::new());
        let description = format!(
            "Approve {operation} budget overrun for {shot_id}: {} (+{}s, +{} bytes)",
            reasons.join("; "),
            incremental_wall_seconds,
            incremental_disk_bytes
        );
        let approval = Approval {
            approval_id: approval_id.clone(),
            kind: ApprovalKind::BudgetOverrun,
            subject_id: Some(approval_id.clone()),
            shot_id: Some(shot_id.to_owned()),
            take_ids: Vec::new(),
            blocking: true,
            description,
            created_at: now.to_owned(),
        };
        state.budget.overruns.insert(
            approval_id.clone(),
            BudgetOverrun {
                approval_id,
                scope: scope.to_owned(),
                shot_id: shot_id.to_owned(),
                operation: operation.to_owned(),
                dimensions,
                reasons,
                incremental_wall_seconds,
                incremental_disk_bytes,
                requested_at: now.to_owned(),
                approved_at: None,
            },
        );
        state.pending_approvals.push(approval.clone());
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        self.save_state(&state, expected_revision)?;
        Ok((state, approval))
    }

    pub fn approve_budget_overrun(
        &self,
        approval_id: &str,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        let approval = state
            .pending_approvals
            .iter()
            .find(|approval| approval.approval_id == approval_id)
            .ok_or_else(|| StoreError::ApprovalNotFound(approval_id.to_owned()))?;
        if approval.kind != ApprovalKind::BudgetOverrun {
            return Err(StoreError::ApprovalNotFound(approval_id.to_owned()));
        }
        let overrun = state
            .budget
            .overruns
            .get_mut(approval_id)
            .ok_or_else(|| StoreError::ApprovalNotFound(approval_id.to_owned()))?;
        overrun.approved_at = Some(now.to_owned());
        state
            .pending_approvals
            .retain(|approval| approval.approval_id != approval_id);
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        let decision =
            self.prepare_decision("budget_overrun_approved", approval_id, command_id, now)?;
        self.save_state(&state, expected_revision)?;
        self.commit_decisions(&[decision])?;
        Ok(state)
    }

    pub fn update_budget_contract(
        &self,
        mut contract: BudgetContract,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<ProjectState, StoreError> {
        contract.validate()?;
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        ensure_revision(&state, expected_revision)?;
        contract.contract_revision = state
            .budget
            .contract
            .contract_revision
            .checked_add(1)
            .ok_or(StoreError::Invariant(
                crate::domain::StateInvariantError::RevisionOverflow,
            ))?;
        state.budget.contract = contract;
        state
            .pending_approvals
            .retain(|approval| approval.kind != ApprovalKind::BudgetOverrun);
        state.budget.overruns.clear();
        state.last_command_id = Some(command_id.to_owned());
        state.bump_revision(now.to_owned())?;
        let decision = self.prepare_decision(
            "budget_contract_updated",
            &state.project_id,
            command_id,
            now,
        )?;
        self.save_state(&state, expected_revision)?;
        self.commit_decisions(&[decision])?;
        Ok(state)
    }
}
