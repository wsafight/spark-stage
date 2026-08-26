use super::*;
use crate::store::BatchTakeSelection;
use crate::tui::protocol::WorkerPayload;

impl<B: TuiBackend> App<B> {
    pub(super) fn refresh_projects(&mut self) -> bool {
        match self.backend.list_projects() {
            Ok(projects) => {
                self.projects = projects;
                self.clamp_selection(Page::Projects);
                true
            }
            Err(error) => {
                if self.page == Page::Projects {
                    self.record_command_error(error);
                }
                false
            }
        }
    }

    pub(super) fn select_project(&mut self, project_id: &str) {
        match self.backend.select_project(project_id) {
            Ok(snapshot) => {
                self.apply_snapshot(snapshot);
                self.connection = ConnectionState::Connected;
                self.page = Page::Dashboard;
                self.storage_report = None;
                self.cleanup_plan = None;
                self.decisions.clear();
                self.project_changed = true;
                self.status = StatusMessage {
                    kind: StatusKind::Success,
                    text: format!("Opened project {project_id}"),
                };
                self.next_refresh_at = Instant::now() + self.refresh_interval;
            }
            Err(error) => self.record_command_error(error),
        }
    }

    pub(super) fn refresh_page_data(&mut self) {
        match self.page {
            Page::Projects => {
                self.refresh_projects();
            }
            Page::Storage if self.snapshot.is_some() => {
                self.dispatch_read_only(WorkerCommand::StorageStatus);
            }
            Page::History if self.snapshot.is_some() => {
                self.dispatch_read_only(WorkerCommand::DecisionHistory { limit: 200 });
            }
            _ => {}
        }
    }

    pub(super) fn sync_review_choices(&mut self, snapshot: &AppSnapshot) {
        let mut next = BTreeMap::new();
        for approval in snapshot
            .pending_approvals
            .iter()
            .filter(|approval| approval.kind == "candidate_selection")
        {
            let Some(shot_id) = approval.shot_id.as_ref() else {
                continue;
            };
            let selected = self
                .review_choices
                .get(shot_id)
                .filter(|choice| approval.take_ids.contains(&choice.take_id))
                .cloned()
                .or_else(|| {
                    snapshot
                        .shots
                        .iter()
                        .find(|shot| &shot.shot_id == shot_id)
                        .and_then(|shot| shot.selected_take_id.as_ref())
                        .filter(|take_id| approval.take_ids.contains(take_id))
                        .map(|take_id| ReviewChoice {
                            take_id: take_id.clone(),
                            included: true,
                        })
                })
                .or_else(|| {
                    approval.take_ids.first().map(|take_id| ReviewChoice {
                        take_id: take_id.clone(),
                        included: true,
                    })
                });
            if let Some(selected) = selected {
                next.insert(shot_id.clone(), selected);
            }
        }
        self.review_choices = next;
    }

    pub(super) fn confirm_project_toggle(&mut self) {
        let Some(project) = self.selected_project() else {
            self.no_selection("project");
            return;
        };
        if self
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.project.id != project.id)
        {
            self.status = StatusMessage {
                kind: StatusKind::Warning,
                text: "Open the highlighted project before pausing or resuming it".to_owned(),
            };
            return;
        }
        self.confirmation = Some(Confirmation {
            prompt: format!(
                "{} project {}?",
                if project.paused { "Resume" } else { "Pause" },
                project.id
            ),
            command: if project.paused {
                WorkerCommand::ResumeProject
            } else {
                WorkerCommand::PauseProject
            },
        });
    }

    pub(super) fn toggle_review_row(&mut self) {
        let Some(shot_id) = self.selected_review_row().map(|row| row.shot_id) else {
            self.no_selection("review row");
            return;
        };
        if let Some(choice) = self.review_choices.get_mut(&shot_id) {
            choice.included = !choice.included;
        }
    }

    pub(super) fn cycle_review_take(&mut self, delta: isize) {
        let Some(row) = self.selected_review_row() else {
            self.no_selection("review row");
            return;
        };
        if row.take_ids.is_empty() {
            return;
        }
        let current = row
            .take_ids
            .iter()
            .position(|take_id| take_id == &row.take_id)
            .unwrap_or(0);
        let next = current
            .saturating_add_signed(delta)
            .min(row.take_ids.len() - 1);
        if let Some(choice) = self.review_choices.get_mut(&row.shot_id) {
            choice.take_id.clone_from(&row.take_ids[next]);
        }
    }

    pub(super) fn confirm_batch_review(&mut self, approve: bool) {
        let rows = self
            .review_rows()
            .into_iter()
            .filter(|row| row.included)
            .collect::<Vec<_>>();
        if rows.is_empty() {
            self.no_selection("included review row");
            return;
        }
        let warnings = rows.iter().map(|row| row.warning_count).sum::<usize>();
        let selections = rows
            .iter()
            .map(|row| BatchTakeSelection {
                shot_id: row.shot_id.clone(),
                take_id: row.take_id.clone(),
                accept_warnings: approve && row.warning_count > 0,
            })
            .collect();
        let warning_text = if warnings == 0 {
            String::new()
        } else {
            format!(" This explicitly accepts {warnings} warning(s).")
        };
        self.confirmation = Some(Confirmation {
            prompt: format!(
                "{} {} selected take(s)?{warning_text}",
                if approve {
                    "Select and approve"
                } else {
                    "Select"
                },
                rows.len()
            ),
            command: WorkerCommand::ReviewBatch {
                selections,
                approve,
            },
        });
    }

    pub(super) fn confirm_cleanup_action(&mut self, apply: bool) {
        let Some(plan) = self.cleanup_plan.as_ref() else {
            self.no_selection("cleanup plan");
            return;
        };
        self.confirmation = Some(Confirmation {
            prompt: format!(
                "{} cleanup plan {} ({} files, {} bytes)?",
                if apply { "Apply" } else { "Restore" },
                plan.plan_id,
                plan.items.len(),
                plan.reclaimable_bytes
            ),
            command: if apply {
                WorkerCommand::ApplyCleanupPlan {
                    plan_id: plan.plan_id.clone(),
                }
            } else {
                WorkerCommand::RestoreCleanupPlan {
                    plan_id: plan.plan_id.clone(),
                }
            },
        });
    }

    pub(super) fn apply_payload(&mut self, payload: WorkerPayload) {
        match payload {
            WorkerPayload::ProjectList { projects } => {
                self.projects = projects;
                self.clamp_selection(Page::Projects);
            }
            WorkerPayload::StorageReport(report) => self.storage_report = Some(report),
            WorkerPayload::CleanupPlan(plan) => self.cleanup_plan = Some(plan),
            WorkerPayload::DecisionHistory { decisions } => {
                self.decisions = decisions;
                self.clamp_selection(Page::History);
            }
            WorkerPayload::ReferenceList { .. }
            | WorkerPayload::ReferenceImpact(_)
            | WorkerPayload::ReferenceVerification(_) => {}
        }
    }
}
