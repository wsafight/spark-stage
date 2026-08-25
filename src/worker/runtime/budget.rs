use std::collections::BTreeSet;

use crate::domain::{ProjectState, ScriptBundle, ShotContract};
use crate::ipc::BudgetSummary;
use crate::store::{ProjectStore, StoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GenerationBudgetGate {
    Allowed,
    DiskBlocked {
        available_bytes: u64,
        required_bytes: u64,
    },
    ApprovalRequired {
        scope: String,
        dimensions: Vec<String>,
        reasons: Vec<String>,
        incremental_wall_seconds: u64,
        incremental_disk_bytes: u64,
    },
}

pub(super) fn budget_summary(
    store: &ProjectStore,
    state: &ProjectState,
    bundle: Option<&ScriptBundle>,
) -> Result<BudgetSummary, StoreError> {
    let contract = &state.budget.contract;
    let estimate = &contract.estimate;
    let elapsed_milliseconds = state
        .takes
        .values()
        .map(|take| take.elapsed_milliseconds)
        .fold(0_u64, u64::saturating_add);
    let elapsed_seconds = elapsed_milliseconds.saturating_add(999) / 1_000;
    let mut remaining_seconds = 0_u64;
    let mut disk_required_bytes = 0_u64;
    let mut audition_takes_used = 0_u32;
    let mut audition_takes_limit = 0_u32;
    let mut final_takes_used = 0_u32;
    let mut final_takes_limit = 0_u32;

    if let Some(bundle) = bundle {
        for shot in &bundle.shots {
            let audition_used = take_count(state, &shot.id, &shot.generation_plan.audition_profile);
            let final_used = take_count(state, &shot.id, &shot.generation_plan.final_profile);
            let audition_planned = u32::from(shot.generation_plan.audition_takes);
            let final_planned = 1_u32;
            audition_takes_used = audition_takes_used.saturating_add(audition_used);
            audition_takes_limit = audition_takes_limit.saturating_add(audition_planned);
            final_takes_used = final_takes_used.saturating_add(final_used);
            final_takes_limit = final_takes_limit.saturating_add(final_planned);
            let audition_remaining = audition_planned.saturating_sub(audition_used);
            let final_remaining = final_planned.saturating_sub(final_used);
            remaining_seconds = remaining_seconds.saturating_add(estimate_wall_seconds(
                shot,
                true,
                audition_remaining,
                state,
            ));
            remaining_seconds = remaining_seconds.saturating_add(estimate_wall_seconds(
                shot,
                false,
                final_remaining,
                state,
            ));
            disk_required_bytes = disk_required_bytes.saturating_add(estimate_disk_bytes(
                shot,
                true,
                audition_remaining,
                state,
            ));
            disk_required_bytes = disk_required_bytes.saturating_add(estimate_disk_bytes(
                shot,
                false,
                final_remaining,
                state,
            ));
        }
    }
    let disk_free_bytes = fs4::available_space(store.root()).map_err(|source| StoreError::Io {
        path: store.root().to_owned(),
        source,
    })?;
    let estimated_total_seconds = elapsed_seconds.saturating_add(remaining_seconds);
    let disk_floor = contract
        .minimum_disk_free_bytes
        .saturating_add(disk_required_bytes);
    Ok(BudgetSummary {
        estimate_source: estimate.source.clone(),
        elapsed_seconds,
        estimated_total_seconds,
        estimated_remaining_seconds: remaining_seconds,
        wall_clock_limit_seconds: contract.wall_clock_limit_seconds,
        disk_free_bytes,
        disk_required_bytes,
        minimum_disk_free_bytes: contract.minimum_disk_free_bytes,
        audition_takes_used,
        audition_takes_limit,
        max_audition_takes_per_shot: contract.max_audition_takes_per_shot,
        final_takes_used,
        final_takes_limit,
        max_final_takes_per_shot: contract.max_final_takes_per_shot,
        overrun_required: estimated_total_seconds > contract.wall_clock_limit_seconds
            || disk_free_bytes < disk_floor,
    })
}

pub(super) fn assess_generation(
    store: &ProjectStore,
    state: &ProjectState,
    bundle: &ScriptBundle,
    shot: &ShotContract,
    profile: &str,
    audition: bool,
) -> Result<GenerationBudgetGate, StoreError> {
    let summary = budget_summary(store, state, Some(bundle))?;
    let contract = &state.budget.contract;
    let used = take_count(state, &shot.id, profile);
    let projected = used.saturating_add(1);
    let incremental_wall_seconds = estimate_wall_seconds(shot, audition, 1, state);
    let incremental_disk_bytes = estimate_disk_bytes(shot, audition, 1, state);
    let planned = if audition {
        u32::from(shot.generation_plan.audition_takes)
    } else {
        1
    };
    let extra = projected > planned;
    let projected_total = summary.estimated_total_seconds.saturating_add(if extra {
        incremental_wall_seconds
    } else {
        0
    });
    let projected_disk =
        summary
            .disk_required_bytes
            .saturating_add(if extra { incremental_disk_bytes } else { 0 });
    let required_disk = contract
        .minimum_disk_free_bytes
        .saturating_add(projected_disk);
    if summary.disk_free_bytes < required_disk {
        return Ok(GenerationBudgetGate::DiskBlocked {
            available_bytes: summary.disk_free_bytes,
            required_bytes: required_disk,
        });
    }

    let operation = if audition { "audition" } else { "final" };
    let mut dimensions = Vec::new();
    let mut reasons = Vec::new();
    let limit = if audition {
        contract.max_audition_takes_per_shot
    } else {
        contract.max_final_takes_per_shot
    };
    if projected > limit {
        dimensions.push(format!(
            "take:{}:{}:{operation}:{projected}",
            contract.contract_revision, shot.id
        ));
        reasons.push(format!(
            "{operation} take {projected} exceeds the per-shot limit {limit}"
        ));
    }
    if projected_total > contract.wall_clock_limit_seconds {
        dimensions.push(format!("wall:{}", contract.contract_revision));
        reasons.push(format!(
            "estimated total {projected_total}s exceeds the {}s wall-clock limit",
            contract.wall_clock_limit_seconds
        ));
    }
    let approved = state
        .budget
        .overruns
        .values()
        .filter(|overrun| overrun.approved_at.is_some())
        .flat_map(|overrun| overrun.dimensions.iter().cloned())
        .collect::<BTreeSet<_>>();
    let mut pending_dimensions = Vec::new();
    let mut pending_reasons = Vec::new();
    for (dimension, reason) in dimensions.into_iter().zip(reasons) {
        if !approved.contains(&dimension) {
            pending_dimensions.push(dimension);
            pending_reasons.push(reason);
        }
    }
    if pending_dimensions.is_empty() {
        return Ok(GenerationBudgetGate::Allowed);
    }
    Ok(GenerationBudgetGate::ApprovalRequired {
        scope: pending_dimensions.join("|"),
        dimensions: pending_dimensions,
        reasons: pending_reasons,
        incremental_wall_seconds,
        incremental_disk_bytes,
    })
}

fn take_count(state: &ProjectState, shot_id: &str, profile: &str) -> u32 {
    u32::try_from(
        state
            .takes
            .values()
            .filter(|take| take.shot_id == shot_id && take.profile == profile)
            .count(),
    )
    .unwrap_or(u32::MAX)
}

fn estimate_wall_seconds(
    shot: &ShotContract,
    audition: bool,
    takes: u32,
    state: &ProjectState,
) -> u64 {
    let coefficient = if audition {
        state
            .budget
            .contract
            .estimate
            .audition_wall_seconds_per_video_second
    } else {
        state
            .budget
            .contract
            .estimate
            .final_wall_seconds_per_video_second
    };
    u64::from(shot.duration)
        .saturating_mul(coefficient)
        .saturating_mul(u64::from(takes))
}

fn estimate_disk_bytes(
    shot: &ShotContract,
    audition: bool,
    takes: u32,
    state: &ProjectState,
) -> u64 {
    let coefficient = if audition {
        state
            .budget
            .contract
            .estimate
            .audition_bytes_per_video_second
    } else {
        state.budget.contract.estimate.final_bytes_per_video_second
    };
    u64::from(shot.duration)
        .saturating_mul(coefficient)
        .saturating_mul(u64::from(takes))
}
