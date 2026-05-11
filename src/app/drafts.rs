use super::*;

pub(super) fn editor_draft_item_key(item: &WorkItem) -> String {
    let kind = match item.kind {
        ItemKind::Notification => "notification",
        ItemKind::PullRequest => "pull",
        ItemKind::Issue => "issue",
    };
    match item.number {
        Some(number) => format!("{kind}:{}#{number}", item.repo),
        None => format!("{kind}:{}", item.id),
    }
}

pub(super) fn new_comment_draft_key(item: &WorkItem) -> String {
    format!("comment:{}:new", editor_draft_item_key(item))
}

pub(super) fn reply_comment_draft_key(
    item: &WorkItem,
    comment: &CommentPreview,
    index: usize,
) -> String {
    let target = comment
        .id
        .map(|id| id.to_string())
        .unwrap_or_else(|| format!("index-{index}"));
    format!("comment:{}:reply:{target}", editor_draft_item_key(item))
}

pub(super) fn edit_comment_draft_key(item: &WorkItem, comment_id: u64, is_review: bool) -> String {
    let kind = if is_review { "review" } else { "issue" };
    format!(
        "comment:{}:edit:{kind}:{comment_id}",
        editor_draft_item_key(item)
    )
}

pub(super) fn review_comment_draft_key(item: &WorkItem, target: &DiffReviewTarget) -> String {
    let start = target
        .start_line
        .map(|line| {
            format!(
                ":{}:{line}",
                target.start_side.map_or("-", DiffReviewSide::as_api_value)
            )
        })
        .unwrap_or_default();
    format!(
        "comment:{}:review:{}:{}:{}{start}",
        editor_draft_item_key(item),
        target.path,
        target.side.as_api_value(),
        target.line
    )
}

pub(super) fn new_issue_draft_key(repo: &str) -> String {
    format!("issue:new:{}", repo.trim())
}

pub(super) fn new_pr_draft_key(repo: &str) -> String {
    format!("pull:new:{}", repo.trim())
}

pub(super) fn issue_dialog_draft_payload(dialog: &IssueDialog) -> NewIssueDraft {
    NewIssueDraft {
        repo: dialog.repo.text().to_string(),
        title: dialog.title.text().to_string(),
        labels: dialog.labels.text().to_string(),
        body: dialog.body.text().to_string(),
    }
}

pub(super) fn pr_create_dialog_draft_payload(dialog: &PrCreateDialog) -> NewPrDraft {
    NewPrDraft {
        repo: dialog.repo.clone(),
        title: dialog.title.text().to_string(),
        body: dialog.body.text().to_string(),
    }
}

pub(super) fn encode_editor_draft<T: Serialize>(payload: &T) -> Result<String> {
    serde_json::to_string(payload).map_err(Into::into)
}

pub(super) fn issue_draft_has_content(payload: &NewIssueDraft) -> bool {
    !payload.title.trim().is_empty()
        || !payload.labels.trim().is_empty()
        || !payload.body.trim().is_empty()
}

pub(super) fn pr_draft_has_content(payload: &NewPrDraft) -> bool {
    !payload.title.trim().is_empty() || !payload.body.trim().is_empty()
}

impl AppState {
    pub(super) fn load_editor_drafts(&mut self, drafts: HashMap<String, EditorDraft>) {
        self.editor_drafts = drafts;
    }

    pub(super) fn open_comment_dialog_with_draft(
        &mut self,
        mode: CommentDialogMode,
        default_body: String,
        draft_key: String,
    ) -> bool {
        let (body, loaded) = self
            .editor_drafts
            .get(&draft_key)
            .map(|draft| (draft.body.clone(), true))
            .unwrap_or_else(|| (default_body.clone(), false));
        self.comment_draft_key = Some(draft_key);
        self.comment_draft_original_body = default_body;
        self.comment_draft_last_saved_body = body.clone();
        self.comment_draft_last_auto_save_at = Instant::now();
        self.comment_dialog = Some(CommentDialog {
            mode,
            body: EditorText::from_text(body),
            scroll: 0,
        });
        loaded
    }

    pub(super) fn save_active_comment_draft(
        &mut self,
        store: &SnapshotStore,
        trigger: DraftSaveTrigger,
        now: Instant,
    ) {
        match self.save_active_comment_draft_result(store, now) {
            Ok(outcome) => self.apply_draft_save_status(outcome, trigger),
            Err(error) => {
                self.status = format!("draft save failed: {error}");
            }
        }
    }

    pub(super) fn save_active_issue_draft(
        &mut self,
        store: &SnapshotStore,
        trigger: DraftSaveTrigger,
        now: Instant,
    ) {
        match self.save_active_issue_draft_result(store, now) {
            Ok(outcome) => self.apply_draft_save_status(outcome, trigger),
            Err(error) => {
                self.status = format!("draft save failed: {error}");
            }
        }
    }

    pub(super) fn save_active_pr_create_draft(
        &mut self,
        store: &SnapshotStore,
        trigger: DraftSaveTrigger,
        now: Instant,
    ) {
        match self.save_active_pr_create_draft_result(store, now) {
            Ok(outcome) => self.apply_draft_save_status(outcome, trigger),
            Err(error) => {
                self.status = format!("draft save failed: {error}");
            }
        }
    }

    pub(super) fn apply_draft_save_status(
        &mut self,
        outcome: DraftSaveOutcome,
        trigger: DraftSaveTrigger,
    ) {
        if trigger != DraftSaveTrigger::Manual {
            return;
        }
        self.status = match outcome {
            DraftSaveOutcome::Saved => "draft saved",
            DraftSaveOutcome::Cleared => "draft cleared",
            DraftSaveOutcome::Unchanged => "draft already saved",
            DraftSaveOutcome::MissingTarget => "drafts unavailable here",
        }
        .to_string();
    }

    pub(super) fn auto_save_active_editor_drafts(
        &mut self,
        store: &SnapshotStore,
        now: Instant,
    ) -> bool {
        let comment_saved = self.auto_save_active_comment_draft(store, now);
        let issue_saved = self.auto_save_active_issue_draft(store, now);
        let pr_saved = self.auto_save_active_pr_create_draft(store, now);
        comment_saved || issue_saved || pr_saved
    }

    pub(super) fn auto_save_active_comment_draft(
        &mut self,
        store: &SnapshotStore,
        now: Instant,
    ) -> bool {
        if self.comment_dialog.is_none() {
            return false;
        }
        if now.saturating_duration_since(self.comment_draft_last_auto_save_at)
            < EDITOR_DRAFT_AUTO_SAVE_INTERVAL
        {
            return false;
        }
        self.comment_draft_last_auto_save_at = now;
        self.save_active_comment_draft(store, DraftSaveTrigger::Auto, now);
        true
    }

    pub(super) fn auto_save_active_issue_draft(
        &mut self,
        store: &SnapshotStore,
        now: Instant,
    ) -> bool {
        if self.issue_dialog.is_none() {
            return false;
        }
        if now.saturating_duration_since(self.issue_draft_last_auto_save_at)
            < EDITOR_DRAFT_AUTO_SAVE_INTERVAL
        {
            return false;
        }
        self.issue_draft_last_auto_save_at = now;
        self.save_active_issue_draft(store, DraftSaveTrigger::Auto, now);
        true
    }

    pub(super) fn auto_save_active_pr_create_draft(
        &mut self,
        store: &SnapshotStore,
        now: Instant,
    ) -> bool {
        if self.pr_create_dialog.is_none() {
            return false;
        }
        if now.saturating_duration_since(self.pr_create_draft_last_auto_save_at)
            < EDITOR_DRAFT_AUTO_SAVE_INTERVAL
        {
            return false;
        }
        self.pr_create_draft_last_auto_save_at = now;
        self.save_active_pr_create_draft(store, DraftSaveTrigger::Auto, now);
        true
    }

    pub(super) fn open_issue_dialog_with_draft(&mut self, repo: String) -> bool {
        let draft_key = new_issue_draft_key(&repo);
        let loaded_payload = self
            .editor_drafts
            .get(&draft_key)
            .and_then(|draft| serde_json::from_str::<NewIssueDraft>(&draft.body).ok());
        let loaded = loaded_payload.is_some();
        let payload = loaded_payload.unwrap_or_else(|| NewIssueDraft {
            repo,
            ..NewIssueDraft::default()
        });
        let dialog = IssueDialog {
            repo: EditorText::from_text(payload.repo),
            title: EditorText::from_text(payload.title),
            labels: EditorText::from_text(payload.labels),
            body: EditorText::from_text(payload.body),
            field: IssueDialogField::Title,
            body_scroll: 0,
        };
        self.issue_draft_last_saved_body =
            encode_editor_draft(&issue_dialog_draft_payload(&dialog)).unwrap_or_default();
        self.issue_draft_last_auto_save_at = Instant::now();
        self.issue_draft_key = Some(draft_key);
        self.issue_dialog = Some(dialog);
        loaded
    }

    pub(super) fn open_pr_create_dialog_with_draft(
        &mut self,
        repo: String,
        local_dir: PathBuf,
        branch: String,
    ) -> bool {
        let head_ref = resolve_pull_request_head_ref(&local_dir, &repo, &branch);
        let draft_key = new_pr_draft_key(&repo);
        let loaded_payload = self
            .editor_drafts
            .get(&draft_key)
            .and_then(|draft| serde_json::from_str::<NewPrDraft>(&draft.body).ok());
        let loaded = loaded_payload.is_some();
        let payload = loaded_payload.unwrap_or_else(|| NewPrDraft {
            repo: repo.clone(),
            ..NewPrDraft::default()
        });
        let dialog = PrCreateDialog {
            repo,
            local_dir,
            branch,
            head_ref,
            title: EditorText::from_text(payload.title),
            body: EditorText::from_text(payload.body),
            field: PrCreateField::Title,
            body_scroll: 0,
        };
        self.pr_create_draft_last_saved_body =
            encode_editor_draft(&pr_create_dialog_draft_payload(&dialog)).unwrap_or_default();
        self.pr_create_draft_last_auto_save_at = Instant::now();
        self.pr_create_draft_key = Some(draft_key);
        self.pr_create_dialog = Some(dialog);
        loaded
    }

    pub(super) fn save_active_issue_draft_result(
        &mut self,
        store: &SnapshotStore,
        _now: Instant,
    ) -> Result<DraftSaveOutcome> {
        let Some(mut key) = self.issue_draft_key.clone() else {
            return Ok(DraftSaveOutcome::MissingTarget);
        };
        let Some(payload) = self.issue_dialog.as_ref().map(issue_dialog_draft_payload) else {
            return Ok(DraftSaveOutcome::MissingTarget);
        };
        let repo = payload.repo.trim();
        if repo.contains('/') {
            let next_key = new_issue_draft_key(repo);
            if next_key != key {
                if let Some(old_key) = self.issue_draft_key.replace(next_key.clone()) {
                    self.editor_drafts.remove(&old_key);
                    store.delete_editor_draft(&old_key)?;
                }
                self.issue_draft_last_saved_body.clear();
                key = next_key;
            }
        }
        let encoded = encode_editor_draft(&payload)?;
        if encoded == self.issue_draft_last_saved_body {
            return Ok(DraftSaveOutcome::Unchanged);
        }

        if !issue_draft_has_content(&payload) {
            store.delete_editor_draft(&key)?;
            self.editor_drafts.remove(&key);
            self.issue_draft_last_saved_body = encoded;
            return Ok(DraftSaveOutcome::Cleared);
        }

        let draft = store.save_editor_draft(&key, &encoded)?;
        self.issue_draft_last_saved_body = draft.body.clone();
        self.editor_drafts.insert(key, draft);
        Ok(DraftSaveOutcome::Saved)
    }

    pub(super) fn save_active_pr_create_draft_result(
        &mut self,
        store: &SnapshotStore,
        _now: Instant,
    ) -> Result<DraftSaveOutcome> {
        let Some(key) = self.pr_create_draft_key.clone() else {
            return Ok(DraftSaveOutcome::MissingTarget);
        };
        let Some(payload) = self
            .pr_create_dialog
            .as_ref()
            .map(pr_create_dialog_draft_payload)
        else {
            return Ok(DraftSaveOutcome::MissingTarget);
        };
        let encoded = encode_editor_draft(&payload)?;
        if encoded == self.pr_create_draft_last_saved_body {
            return Ok(DraftSaveOutcome::Unchanged);
        }

        if !pr_draft_has_content(&payload) {
            store.delete_editor_draft(&key)?;
            self.editor_drafts.remove(&key);
            self.pr_create_draft_last_saved_body = encoded;
            return Ok(DraftSaveOutcome::Cleared);
        }

        let draft = store.save_editor_draft(&key, &encoded)?;
        self.pr_create_draft_last_saved_body = draft.body.clone();
        self.editor_drafts.insert(key, draft);
        Ok(DraftSaveOutcome::Saved)
    }

    pub(super) fn clear_pending_issue_draft_local(&mut self) {
        if let Some(key) = self
            .pending_issue_create
            .as_ref()
            .and_then(|pending| pending.draft_key.as_deref())
        {
            self.editor_drafts.remove(key);
            self.issue_draft_last_saved_body.clear();
        }
    }

    pub(super) fn clear_pending_pr_create_draft_local(&mut self) {
        if let Some(key) = self
            .pending_pr_create
            .as_ref()
            .and_then(|pending| pending.draft_key.as_deref())
        {
            self.editor_drafts.remove(key);
            self.pr_create_draft_last_saved_body.clear();
        }
    }

    pub(super) fn save_active_comment_draft_result(
        &mut self,
        store: &SnapshotStore,
        _now: Instant,
    ) -> Result<DraftSaveOutcome> {
        let Some(key) = self.comment_draft_key.clone() else {
            return Ok(DraftSaveOutcome::MissingTarget);
        };
        let Some(current) = self
            .comment_dialog
            .as_ref()
            .map(|dialog| dialog.body.text().to_string())
        else {
            return Ok(DraftSaveOutcome::MissingTarget);
        };
        if current == self.comment_draft_last_saved_body {
            return Ok(DraftSaveOutcome::Unchanged);
        }

        if current.trim().is_empty() || current == self.comment_draft_original_body {
            store.delete_editor_draft(&key)?;
            self.editor_drafts.remove(&key);
            self.comment_draft_last_saved_body = current;
            return Ok(DraftSaveOutcome::Cleared);
        }

        let draft = store.save_editor_draft(&key, &current)?;
        self.comment_draft_last_saved_body = draft.body.clone();
        self.editor_drafts.insert(key, draft);
        Ok(DraftSaveOutcome::Saved)
    }

    pub(super) fn clear_pending_comment_draft_local(&mut self) {
        if let Some(key) = self
            .pending_comment_submit
            .as_ref()
            .and_then(|pending| pending.draft_key.as_deref())
        {
            self.editor_drafts.remove(key);
            if self.comment_draft_key.as_deref() == Some(key) {
                self.comment_draft_last_saved_body = self.comment_draft_original_body.clone();
            }
        }
    }
}
