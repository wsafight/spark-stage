use super::*;

impl WorkerRuntime {
    pub(super) fn list_projects(&self, request: &ClientRequest) -> WorkerReply {
        if request.project_id.is_some() || request.expected_revision.is_some() {
            return failure(
                request,
                "INVALID_ARGUMENT",
                "project list does not accept project_id or expected_revision".to_owned(),
                false,
                None,
            );
        }
        let entries = match fs::read_dir(&self.paths.projects_dir) {
            Ok(entries) => entries,
            Err(error) => {
                return failure(
                    request,
                    "STORE_ERROR",
                    format!(
                        "cannot list projects at {}: {error}",
                        self.paths.projects_dir.display()
                    ),
                    true,
                    None,
                );
            }
        };
        let mut projects = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().join("project.json").is_file())
            .filter_map(|entry| {
                let id = entry.file_name().into_string().ok()?;
                let item = match ProjectStore::open(&self.paths.projects_dir, &id)
                    .and_then(|store| store.read_state())
                {
                    Ok(state) => ProjectListItem {
                        id,
                        title: Some(state.title),
                        stage: Some(project_stage(state.project_stage).to_owned()),
                        outcome: Some(project_outcome(state.project_outcome).to_owned()),
                        paused: state.paused,
                        revision: Some(state.revision),
                        updated_at: Some(state.updated_at),
                        error: None,
                    },
                    Err(error) => ProjectListItem {
                        id,
                        error: Some(error.to_string()),
                        ..ProjectListItem::default()
                    },
                };
                Some(item)
            })
            .collect::<Vec<_>>();
        projects.sort_by(|left, right| left.id.cmp(&right.id));
        success_payload(
            request,
            None,
            WorkerPayload::ProjectList { projects },
            "projects loaded",
        )
    }

    pub(super) fn set_project_paused(
        &mut self,
        request: &ClientRequest,
        paused: bool,
    ) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let state = match store.set_project_paused(
            paused,
            expected_revision,
            &request.command_id,
            &timestamp(),
        ) {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };
        match self.snapshot(&store, state) {
            Ok(snapshot) => success(
                request,
                Some(snapshot.revision),
                Some(snapshot),
                if paused {
                    "project paused"
                } else {
                    "project resumed"
                },
            ),
            Err(error) => worker_failure(request, error),
        }
    }
}
