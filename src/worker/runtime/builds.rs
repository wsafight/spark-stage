use super::*;

impl WorkerRuntime {
    pub(super) fn recover_builds(&self) -> Result<(), WorkerRunError> {
        let entries =
            fs::read_dir(&self.paths.projects_dir).map_err(|source| WorkerRunError::Io {
                path: self.paths.projects_dir.clone(),
                source,
            })?;
        for entry in entries.filter_map(Result::ok) {
            let Some(project_id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(store) = ProjectStore::open(&self.paths.projects_dir, &project_id) else {
                continue;
            };
            let state = store.read_state()?;
            let active_builds = state
                .builds
                .values()
                .filter(|build| matches!(build.status.as_str(), "queued" | "running"))
                .cloned()
                .collect::<Vec<_>>();
            for build in active_builds {
                let command_id = if build.command_id.is_empty() {
                    state
                        .last_command_id
                        .clone()
                        .unwrap_or_else(|| format!("RECOVER-{}", Ulid::new()))
                } else {
                    build.command_id.clone()
                };
                let recipe_path = PathBuf::from(&build.recipe);
                if recipe_path.is_absolute()
                    || recipe_path.components().any(|component| {
                        !matches!(component, Component::Normal(_) | Component::CurDir)
                    })
                {
                    self.fail_build_recovery(
                        &store,
                        &build.build_id,
                        &command_id,
                        format!("unsafe recipe path `{}`", build.recipe),
                        false,
                    )?;
                    continue;
                }
                let recipe: BuildRecipe =
                    match crate::store::read_json(&store.root().join(recipe_path)) {
                        Ok(recipe) => recipe,
                        Err(error) => {
                            self.fail_build_recovery(
                                &store,
                                &build.build_id,
                                &command_id,
                                error.to_string(),
                                false,
                            )?;
                            continue;
                        }
                    };
                if recipe.build_id != build.build_id || recipe.project_id != project_id {
                    self.fail_build_recovery(
                        &store,
                        &build.build_id,
                        &command_id,
                        "recipe identity does not match the build record".to_owned(),
                        false,
                    )?;
                    continue;
                }
                if let Err(error) = crate::build::validate_current(&recipe, &state) {
                    self.fail_build_recovery(
                        &store,
                        &build.build_id,
                        &command_id,
                        error.to_string(),
                        true,
                    )?;
                    continue;
                }
                self.build_executor
                    .send(BuildRequest {
                        project_id: project_id.clone(),
                        project_root: store.root().to_owned(),
                        command_id,
                        recipe,
                    })
                    .map_err(WorkerRunError::BuildExecutorChannel)?;
            }
        }
        Ok(())
    }

    fn fail_build_recovery(
        &self,
        store: &ProjectStore,
        build_id: &str,
        command_id: &str,
        reason: String,
        stale: bool,
    ) -> Result<(), WorkerRunError> {
        let revision = store.read_state()?.revision;
        store.finish_build(
            build_id,
            None,
            Some(format!("build recovery failed: {reason}")),
            stale,
            revision,
            command_id,
            &timestamp(),
        )?;
        Ok(())
    }

    pub(super) fn poll_build_events(&mut self) -> Result<bool, WorkerRunError> {
        let mut changed = false;
        loop {
            let (request, mut output, mut error, mut stale) = match self.build_executor.try_recv() {
                Ok(BuildEvent::Started(request)) => {
                    let store = ProjectStore::open(&self.paths.projects_dir, &request.project_id)?;
                    let revision = store.read_state()?.revision;
                    store.mark_build_running(
                        &request.recipe.build_id,
                        revision,
                        &request.command_id,
                        &timestamp(),
                    )?;
                    changed = true;
                    continue;
                }
                Ok(BuildEvent::Completed(request)) => {
                    let output = Some(request.recipe.output_path.clone());
                    (request, output, None, false)
                }
                Ok(BuildEvent::Failed { request, message }) => {
                    (request, None, Some(message), false)
                }
                Err(TryRecvError::Empty) => return Ok(changed),
                Err(TryRecvError::Disconnected) => {
                    return Err(WorkerRunError::BuildExecutorChannel(
                        "build executor stopped".to_owned(),
                    ));
                }
            };
            let store = ProjectStore::open(&self.paths.projects_dir, &request.project_id)?;
            let state = store.read_state()?;
            if output.is_some()
                && let Err(validation) = crate::build::validate_current(&request.recipe, &state)
            {
                output = None;
                error = Some(validation.to_string());
                stale = true;
            }
            let milestone = if output.is_some() {
                (MilestoneKind::BuildCompleted, "build completed".to_owned())
            } else {
                (
                    MilestoneKind::BuildFailed,
                    error
                        .clone()
                        .unwrap_or_else(|| "build failed without diagnostics".to_owned()),
                )
            };
            store.finish_build(
                &request.recipe.build_id,
                output,
                error,
                stale,
                state.revision,
                &request.command_id,
                &timestamp(),
            )?;
            self.emit_milestone(
                milestone.0,
                request.project_id.clone(),
                request.recipe.build_id.clone(),
                milestone.1,
            );
            changed = true;
        }
    }
}
