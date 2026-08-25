use super::*;
use crate::store::BatchTakeSelection;

impl WorkerRuntime {
    pub(super) fn review_batch(
        &mut self,
        request: &ClientRequest,
        selections: &[BatchTakeSelection],
        approve: bool,
    ) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        match store.review_takes_batch(
            selections,
            approve,
            expected_revision,
            &request.command_id,
            &timestamp(),
        ) {
            Ok(state) => match self.snapshot(&store, state) {
                Ok(snapshot) => success(
                    request,
                    Some(snapshot.revision),
                    Some(snapshot),
                    if approve {
                        "take batch selected and approved"
                    } else {
                        "take batch selected"
                    },
                ),
                Err(error) => worker_failure(request, error),
            },
            Err(error) => store_failure(request, error),
        }
    }

    pub(super) fn mutate_take(
        &mut self,
        request: &ClientRequest,
        shot_id: &str,
        take_id: &str,
        mutation: TakeMutation,
    ) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let now = timestamp();
        let result = match mutation {
            TakeMutation::Select => store.select_take(
                shot_id,
                take_id,
                expected_revision,
                &request.command_id,
                &now,
            ),
            TakeMutation::Approve => store.approve_take(
                shot_id,
                take_id,
                expected_revision,
                &request.command_id,
                &now,
            ),
            TakeMutation::Reject => store.reject_take(
                shot_id,
                take_id,
                expected_revision,
                &request.command_id,
                &now,
            ),
        };
        match result {
            Ok(state) => match self.snapshot(&store, state) {
                Ok(snapshot) => success(
                    request,
                    Some(snapshot.revision),
                    Some(snapshot),
                    mutation.message(),
                ),
                Err(error) => worker_failure(request, error),
            },
            Err(error) => store_failure(request, error),
        }
    }

    pub(super) fn retry_shot(&mut self, request: &ClientRequest, shot_id: &str) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let state = match store.read_state() {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };
        if state.revision != expected_revision {
            return store_failure(
                request,
                StoreError::RevisionConflict {
                    expected: expected_revision,
                    actual: state.revision,
                },
            );
        }
        let Some(shot) = state.shots.get(shot_id) else {
            return failure(
                request,
                "SHOT_NOT_FOUND",
                format!("shot `{shot_id}` is missing from the active contract"),
                false,
                Some(state.revision),
            );
        };
        if let Some(job_id) = &shot.active_job_id {
            return store_failure(
                request,
                StoreError::ShotBusy {
                    shot_id: shot_id.to_owned(),
                    job_id: job_id.clone(),
                },
            );
        }
        if shot.approved_take_id.is_some() {
            return store_failure(request, StoreError::ShotAlreadyApproved(shot_id.to_owned()));
        }
        if !matches!(shot.stage, ShotStage::Pending | ShotStage::Failed) {
            return failure(
                request,
                "SHOT_NOT_RETRYABLE",
                format!(
                    "shot `{shot_id}` is `{}`; retry requires pending or failed",
                    shot_stage(shot.stage)
                ),
                false,
                Some(state.revision),
            );
        }
        let audition = shot.selected_candidate_take_id.is_none();
        self.enqueue_shot(request, shot_id, audition)
    }

    pub(super) fn preview_take(&self, request: &ClientRequest, take_id: &str) -> WorkerReply {
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let state = match store.read_state() {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };
        let Some(take) = state.takes.get(take_id) else {
            return store_failure(request, StoreError::TakeNotFound(take_id.to_owned()));
        };
        if take.media_path.is_absolute()
            || take
                .media_path
                .components()
                .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return failure(
                request,
                "ARTIFACT_PATH_INVALID",
                format!("take `{take_id}` has an unsafe media path"),
                false,
                Some(state.revision),
            );
        }
        let media_path = store.root().join(&take.media_path);
        if !media_path.is_file() {
            return failure(
                request,
                "ARTIFACT_NOT_FOUND",
                format!(
                    "take `{take_id}` media is missing at {}",
                    media_path.display()
                ),
                false,
                Some(state.revision),
            );
        }
        WorkerReply {
            protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
            command_id: request.command_id.clone(),
            ok: true,
            revision: Some(state.revision),
            snapshot: None,
            payload: None,
            artifact_path: Some(media_path),
            message: Some("take preview ready".to_owned()),
            error: None,
        }
    }
}
