use super::*;

pub(super) fn start_refresh(
    config: Config,
    store: SnapshotStore,
    tx: UnboundedSender<AppMsg>,
    priority: RefreshPriority,
    scope: RefreshScope,
) {
    let _ = tx.send(AppMsg::RefreshStarted {
        scope: scope.clone(),
    });
    tokio::spawn(async move {
        let mut save_error = None;
        let refresh = async {
            refresh_dashboard_with_progress(&config, scope, |section| {
                if save_error.is_none()
                    && section.error.is_none()
                    && let Err(error) = store.save_section(section)
                {
                    let message = error.to_string();
                    warn!(error = %message, "failed to save refreshed snapshot");
                    save_error = Some(message);
                }

                let _ = tx.send(AppMsg::RefreshSectionLoaded {
                    section: section.clone(),
                    save_error: save_error.clone(),
                });
            })
            .await
        };
        let sections = match priority {
            RefreshPriority::User => refresh.await,
            RefreshPriority::Background => with_background_github_priority(refresh).await,
        };
        let _ = tx.send(AppMsg::RefreshFinished {
            sections,
            save_error,
        });
    });
}

pub(super) fn start_idle_sweep(
    config: Config,
    store: SnapshotStore,
    tx: UnboundedSender<AppMsg>,
    active_view: String,
    cursor: usize,
) {
    let _ = tx.send(AppMsg::IdleSweepStarted);
    tokio::spawn(async move {
        let refresh =
            refresh_idle_search_sections(&config, &active_view, cursor, IDLE_SWEEP_SECTION_LIMIT);
        let result = with_background_github_priority(refresh).await;
        let mut sections = Vec::new();

        for section in result.sections {
            if section.error.is_some() {
                continue;
            }

            if let Err(error) = store.save_section(&section) {
                warn!(
                    error = %error,
                    section = %section.key,
                    "failed to save idle refreshed snapshot"
                );
                continue;
            }

            sections.push(section);
        }

        let _ = tx.send(AppMsg::IdleSweepFinished {
            sections,
            next_cursor: result.next_cursor,
        });
    });
}

pub(super) fn start_section_page_load(
    app: &mut AppState,
    config: &Config,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
    delta: isize,
) {
    if app.section_page_loading.is_some() {
        app.status = "result page already loading".to_string();
        return;
    }

    let request = match app.section_page_request(delta, config) {
        Ok(request) => request,
        Err(message) => {
            app.status = message;
            return;
        }
    };
    let page_label =
        section_page_status_label(request.page, request.total_pages, request.total_is_capped);
    app.section_page_loading = Some(SectionPageLoading {
        section_key: request.section_key.clone(),
        title: request.title.clone(),
        page_label: page_label.clone(),
        started_at: Instant::now(),
    });
    app.last_refresh_request = Instant::now();
    app.status = format!("loading {} page {page_label}", request.title);

    let config = config.clone();
    let store = store.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let section = refresh_section_page(
            request.view,
            request.kind,
            request.title,
            request.filters,
            request.page,
            request.page_size,
            &config,
        )
        .await;
        let mut save_error = None;
        if section.error.is_none()
            && let Err(error) = store.save_section(&section)
        {
            let message = error.to_string();
            warn!(error = %message, "failed to save paged snapshot");
            save_error = Some(message);
        }
        let _ = tx.send(AppMsg::SectionPageLoaded {
            section_key: request.section_key,
            section,
            save_error,
        });
    });
}

pub(super) fn start_filtered_section_load(
    app: &mut AppState,
    config: &Config,
    tx: &UnboundedSender<AppMsg>,
    filter: Option<QuickFilter>,
) {
    let Some(section) = app.current_section() else {
        app.status = "no section selected".to_string();
        return;
    };
    if !matches!(
        section.kind,
        SectionKind::PullRequests | SectionKind::Issues
    ) {
        app.status = "quick filters are available for PR and issue sections".to_string();
        return;
    }

    let section_key = section.key.clone();
    let view = section_view_key(section);
    let kind = section.kind;
    let title = section.title.clone();
    let base_filters = app.base_filters_for_section(section);
    let page_size = section_page_size(section, config);
    let effective_filters = match &filter {
        Some(filter) => quick_filter_query(&base_filters, filter),
        None => base_filters.clone(),
    };
    let status_filter = filter.as_ref().map(QuickFilter::display);

    match filter {
        Some(filter) => {
            app.section_filter_overrides.remove(&section_key);
            app.quick_filters.insert(section_key.clone(), filter);
        }
        None => {
            app.quick_filters.remove(&section_key);
            app.section_filter_overrides.remove(&section_key);
        }
    }

    app.refreshing = true;
    app.section_page_loading = None;
    app.last_refresh_request = Instant::now();
    app.save_current_conversation_details_state();
    app.set_current_selected_position(0);
    app.clear_current_list_scroll_offset();
    app.reset_or_restore_current_conversation_details_state();
    app.comment_dialog = None;
    app.pr_action_dialog = None;
    app.status = match status_filter {
        Some(filter) if !filter.is_empty() => format!("applying filter {filter}"),
        _ => "clearing quick filter".to_string(),
    };

    let config = config.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let section =
            refresh_section_page(view, kind, title, effective_filters, 1, page_size, &config).await;
        let _ = tx.send(AppMsg::FilterSectionLoaded {
            section_key,
            section,
        });
    });
}

pub(super) fn start_global_search(
    query: String,
    display_query: String,
    repo_scope: Option<String>,
    config: Config,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let sections = search_global(&query, repo_scope.as_deref(), &config).await;
        let _ = tx.send(AppMsg::GlobalSearchFinished {
            query: display_query,
            sections,
        });
    });
}

pub(super) fn start_notification_read_sync(
    thread_id: String,
    store: SnapshotStore,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let result = match mark_notification_thread_read(&thread_id).await {
            Ok(()) => match store.mark_notification_read(&thread_id) {
                Ok(_) => Ok(None),
                Err(error) => Ok(Some(error.to_string())),
            },
            Err(error) => Err(error.to_string()),
        };
        let _ = tx.send(AppMsg::NotificationReadFinished { thread_id, result });
    });
}

pub(super) fn start_notification_done_sync(
    thread_id: String,
    store: SnapshotStore,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let result = match mark_notification_thread_done(&thread_id).await {
            Ok(()) => match store.mark_notification_done(&thread_id) {
                Ok(_) => Ok(None),
                Err(error) => Ok(Some(error.to_string())),
            },
            Err(error) => Err(error.to_string()),
        };
        let _ = tx.send(AppMsg::NotificationDoneFinished { thread_id, result });
    });
}

pub(super) fn start_inbox_mark_all_read_sync(store: SnapshotStore, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let result = match mark_all_notifications_read().await {
            Ok(()) => match store.mark_all_notifications_read() {
                Ok(_) => Ok(None),
                Err(error) => Ok(Some(error.to_string())),
            },
            Err(error) => Err(error.to_string()),
        };
        let _ = tx.send(AppMsg::InboxMarkAllReadFinished { result });
    });
}

pub(super) fn start_inbox_thread_action_sync(
    thread_id: String,
    action: InboxThreadAction,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let result = match action {
            InboxThreadAction::Mute => mute_notification_thread(&thread_id).await,
            InboxThreadAction::Subscribe => subscribe_notification_thread(&thread_id).await,
            InboxThreadAction::Unsubscribe => unsubscribe_notification_thread(&thread_id).await,
        }
        .map_err(|error| error.to_string());
        let _ = tx.send(AppMsg::InboxThreadActionFinished { action, result });
    });
}

pub(super) fn start_item_subscription_sync(
    item: WorkItem,
    action: ItemSubscriptionAction,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let item_kind = item.kind;
        let result = match (item.number, item.kind) {
            (Some(number), ItemKind::Issue | ItemKind::PullRequest) => update_item_subscription(
                &item.repo,
                number,
                item.kind,
                matches!(action, ItemSubscriptionAction::Subscribe),
            )
            .await
            .map_err(error_chain_message),
            (None, _) => Err("selected item has no issue or pull request number".to_string()),
            (_, _) => Err("selected item is not an issue or pull request".to_string()),
        };
        let _ = tx.send(AppMsg::ItemSubscriptionUpdated {
            item_id,
            item_kind,
            action,
            result,
        });
    });
}

pub(super) fn start_comments_load(item: WorkItem, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let comments = match item.number {
            Some(number) => fetch_comments(&item.repo, number, item.kind)
                .await
                .map_err(|error| error.to_string()),
            None => Ok(CommentFetchResult {
                item_metadata: None,
                item_reactions: None,
                item_milestone: None,
                comments: Vec::new(),
            }),
        };
        let _ = tx.send(AppMsg::CommentsLoaded { item_id, comments });
    });
}

pub(super) fn start_action_hints_load(item: WorkItem, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let actions = match item.number {
            Some(number) => fetch_pull_request_action_hints(&item.repo, number)
                .await
                .map_err(|error| error.to_string()),
            None => Err("selected item has no pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::ActionHintsLoaded { item_id, actions });
    });
}

pub(super) fn start_diff_load(item: WorkItem, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let diff = match item.number {
            Some(number) => match fetch_pull_request_diff(&item.repo, number).await {
                Ok(diff) => parse_pull_request_diff(&diff),
                Err(error) => Err(error.to_string()),
            },
            None => Err("selected item has no pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::DiffLoaded { item_id, diff });
    });
}

pub(super) fn error_chain_message(error: anyhow::Error) -> String {
    let mut messages = Vec::new();
    for cause in error.chain() {
        let message = cause.to_string();
        if !message.trim().is_empty() && messages.last() != Some(&message) {
            messages.push(message);
        }
    }
    messages.join(": ")
}

pub(super) fn retryable_operation_error_body(error: &str) -> String {
    operation_error_body(error)
}

pub(super) fn item_supports_comments_refresh(item: &WorkItem) -> bool {
    matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) && item.number.is_some()
}

pub(super) fn start_comment_submit(
    item: WorkItem,
    body: String,
    draft_clear: Option<DraftClearTask>,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = match item.number {
            Some(number) => post_issue_comment(&item.repo, number, &body)
                .await
                .map_err(error_chain_message),
            None => Err("selected item has no issue or pull request number".to_string()),
        };
        let posted = result.is_ok();
        if posted {
            clear_editor_draft_after_success(draft_clear);
        }
        let _ = tx.send(AppMsg::CommentPosted {
            item_id: item_id.clone(),
            result,
        });
        if posted && let Some(number) = item.number {
            let comments = fetch_comments(&item.repo, number, item.kind)
                .await
                .map_err(error_chain_message);
            let _ = tx.send(AppMsg::CommentsLoaded { item_id, comments });
        }
    });
}

pub(super) fn start_review_reply_submit(
    item: WorkItem,
    comment_id: u64,
    body: String,
    draft_clear: Option<DraftClearTask>,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = match item.number {
            Some(number) => post_pull_request_review_reply(&item.repo, number, comment_id, &body)
                .await
                .map_err(error_chain_message),
            None => Err("selected item has no pull request number".to_string()),
        };
        let posted = result.is_ok();
        if posted {
            clear_editor_draft_after_success(draft_clear);
        }
        let _ = tx.send(AppMsg::CommentPosted {
            item_id: item_id.clone(),
            result,
        });
        if posted && let Some(number) = item.number {
            let comments = fetch_comments(&item.repo, number, item.kind)
                .await
                .map_err(error_chain_message);
            let _ = tx.send(AppMsg::CommentsLoaded { item_id, comments });
        }
    });
}

pub(super) fn start_comment_edit(
    item: WorkItem,
    comment_index: usize,
    comment_id: u64,
    is_review: bool,
    body: String,
    draft_clear: Option<DraftClearTask>,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let result = match item.number {
            Some(number) => match if is_review {
                edit_pull_request_review_comment(&item.repo, comment_id, &body).await
            } else {
                edit_issue_comment(&item.repo, comment_id, &body).await
            } {
                Ok(()) => fetch_comments(&item.repo, number, item.kind)
                    .await
                    .map_err(error_chain_message),
                Err(error) => Err(error_chain_message(error)),
            },
            None => Err("selected item has no issue or pull request number".to_string()),
        };
        if result.is_ok() {
            clear_editor_draft_after_success(draft_clear);
        }
        let _ = tx.send(AppMsg::CommentUpdated {
            item_id: item.id,
            comment_index,
            result,
        });
    });
}

pub(super) fn start_item_edit(pending: PendingItemEdit, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let item_id = pending.item.id.clone();
        let result = run_item_edit(pending).await.map_err(error_chain_message);
        let _ = tx.send(AppMsg::ItemEdited { item_id, result });
    });
}

async fn run_item_edit(pending: PendingItemEdit) -> Result<ItemEditUpdate> {
    let item = pending.item;
    let Some(number) = item.number else {
        bail!("selected item has no issue or pull request number");
    };

    let original_body = item.body.clone().unwrap_or_default();
    let title_changed = pending.title != item.title;
    let body_changed = pending.body != original_body;
    let mut updated_at = None;
    if title_changed || body_changed {
        let title = title_changed.then_some(pending.title.as_str());
        let body = body_changed.then_some(pending.body.as_str());
        let update = edit_item_metadata(&item.repo, number, title, body).await?;
        updated_at = update.updated_at;
    }

    let labels_to_add = names_added(&item.labels, &pending.labels);
    let labels_to_remove = names_removed(&item.labels, &pending.labels);
    for label in labels_to_add {
        add_issue_label(&item.repo, number, &label).await?;
    }
    for label in labels_to_remove {
        remove_issue_label(&item.repo, number, &label).await?;
    }

    let assignees_to_add = names_added(&item.assignees, &pending.assignees);
    if !assignees_to_add.is_empty() {
        update_issue_assignees(
            &item.repo,
            number,
            item.kind,
            AssigneeAction::Assign,
            &assignees_to_add,
        )
        .await?;
    }
    let assignees_to_remove = names_removed(&item.assignees, &pending.assignees);
    if !assignees_to_remove.is_empty() {
        update_issue_assignees(
            &item.repo,
            number,
            item.kind,
            AssigneeAction::Unassign,
            &assignees_to_remove,
        )
        .await?;
    }

    Ok(ItemEditUpdate {
        title: pending.title,
        body: (!pending.body.trim().is_empty()).then_some(pending.body),
        labels: pending.labels,
        assignees: pending.assignees,
        updated_at,
    })
}

fn names_added(current: &[String], target: &[String]) -> Vec<String> {
    target
        .iter()
        .filter(|name| {
            !current
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(name))
        })
        .cloned()
        .collect()
}

fn names_removed(current: &[String], target: &[String]) -> Vec<String> {
    current
        .iter()
        .filter(|name| {
            !target
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(name))
        })
        .cloned()
        .collect()
}

pub(super) fn start_review_comment_submit(
    item: WorkItem,
    target: DiffReviewTarget,
    body: String,
    draft_clear: Option<DraftClearTask>,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = match item.number {
            Some(number) => post_pull_request_review_comment(
                &item.repo,
                number,
                PullRequestReviewCommentTarget {
                    path: &target.path,
                    line: target.line,
                    side: target.side.as_api_value(),
                    start_line: target.start_line,
                    start_side: target.start_side.map(DiffReviewSide::as_api_value),
                },
                &body,
            )
            .await
            .map_err(error_chain_message),
            None => Err("selected item has no pull request number".to_string()),
        };
        let posted = result.is_ok();
        if posted {
            clear_editor_draft_after_success(draft_clear);
        }
        let _ = tx.send(AppMsg::ReviewCommentPosted {
            item_id: item_id.clone(),
            result,
        });
        if posted && let Some(number) = item.number {
            let comments = fetch_comments(&item.repo, number, item.kind)
                .await
                .map_err(error_chain_message);
            let _ = tx.send(AppMsg::CommentsLoaded { item_id, comments });
        }
    });
}

pub(super) fn draft_clear_task(
    draft_key: Option<String>,
    store: Option<SnapshotStore>,
) -> Option<DraftClearTask> {
    Some(DraftClearTask {
        key: draft_key?,
        store: store?,
    })
}

pub(super) fn clear_editor_draft_after_success(draft_clear: Option<DraftClearTask>) {
    let Some(draft_clear) = draft_clear else {
        return;
    };
    if let Err(error) = draft_clear.store.delete_editor_draft(&draft_clear.key) {
        warn!(draft_key = %draft_clear.key, error = %error, "failed to clear submitted editor draft");
    }
}

pub(super) fn start_review_draft_create(item: WorkItem, body: String, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = match item.number {
            Some(number) => create_pending_pull_request_review(&item.repo, number, &body)
                .await
                .map(|review_id| PendingReviewState { review_id, body })
                .map_err(|error| error.to_string()),
            None => Err("selected item has no pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::ReviewDraftCreated { item_id, result });
    });
}

pub(super) fn start_review_submit(
    item: WorkItem,
    event: PullRequestReviewEvent,
    body: String,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = match item.number {
            Some(number) => submit_pull_request_review(&item.repo, number, event, &body)
                .await
                .map_err(|error| error.to_string()),
            None => Err("selected item has no pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::ReviewSubmitted {
            item_id,
            event,
            result,
        });
    });
}

pub(super) fn start_pending_review_submit(
    item: WorkItem,
    review_id: u64,
    event: PullRequestReviewEvent,
    body: String,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = match item.number {
            Some(number) => {
                submit_pending_pull_request_review(&item.repo, number, review_id, event, &body)
                    .await
                    .map_err(|error| error.to_string())
            }
            None => Err("selected item has no pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::PendingReviewSubmitted {
            item_id,
            review_id,
            event,
            result,
        });
    });
}

pub(super) fn start_pending_review_discard(
    item: WorkItem,
    review_id: u64,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = match item.number {
            Some(number) => discard_pending_pull_request_review(&item.repo, number, review_id)
                .await
                .map_err(|error| error.to_string()),
            None => Err("selected item has no pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::PendingReviewDiscarded {
            item_id,
            review_id,
            result,
        });
    });
}

pub(super) fn start_reaction_submit(
    item: WorkItem,
    target: ReactionTarget,
    content: ReactionContent,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = match item.number {
            Some(number) => {
                let add_result = match target {
                    ReactionTarget::Item => {
                        add_issue_reaction(&item.repo, number, content.api_value()).await
                    }
                    ReactionTarget::IssueComment { comment_id, .. } => {
                        add_issue_comment_reaction(&item.repo, comment_id, content.api_value())
                            .await
                    }
                    ReactionTarget::ReviewComment { comment_id, .. } => {
                        add_pull_request_review_comment_reaction(
                            &item.repo,
                            comment_id,
                            content.api_value(),
                        )
                        .await
                    }
                };
                match add_result {
                    Ok(()) => fetch_comments(&item.repo, number, item.kind)
                        .await
                        .map_err(|error| error.to_string()),
                    Err(error) => Err(error.to_string()),
                }
            }
            None => Err("selected item has no issue or pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::ReactionPosted { item_id, result });
    });
}

pub(super) fn start_label_update(item: WorkItem, action: LabelAction, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = match (item.number, &action) {
            (Some(number), LabelAction::Add(label)) => add_issue_label(&item.repo, number, label)
                .await
                .map_err(|error| error.to_string()),
            (Some(number), LabelAction::Remove(label)) => {
                remove_issue_label(&item.repo, number, label)
                    .await
                    .map_err(|error| error.to_string())
            }
            (None, _) => Err("selected item has no issue or pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::LabelUpdated {
            item_id,
            action,
            result,
        });
    });
}

pub(super) fn start_label_suggestions_load(
    repo: String,
    store: Option<SnapshotStore>,
    tx: UnboundedSender<AppMsg>,
) -> bool {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return false;
    };
    handle.spawn(async move {
        let result = fetch_repository_labels(&repo)
            .await
            .map_err(|error| error.to_string());
        if let Ok(labels) = &result
            && let Some(store) = &store
            && let Err(error) = store.save_label_candidates(&repo, labels)
        {
            warn!(error = %error, repo = %repo, "failed to persist label candidates");
        }
        let _ = tx.send(AppMsg::LabelSuggestionsLoaded { repo, result });
    });
    true
}

pub(super) fn start_assignee_suggestions_load(
    repo: String,
    store: Option<SnapshotStore>,
    tx: UnboundedSender<AppMsg>,
) -> bool {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return false;
    };
    handle.spawn(async move {
        let result = fetch_repository_assignees(&repo)
            .await
            .map_err(|error| error.to_string());
        if let Ok(assignees) = &result
            && let Some(store) = &store
            && let Err(error) = store.save_assignee_candidates(&repo, assignees)
        {
            warn!(error = %error, repo = %repo, "failed to persist assignee candidates");
        }
        let _ = tx.send(AppMsg::AssigneeSuggestionsLoaded { repo, result });
    });
    true
}

pub(super) fn start_reviewer_suggestions_load(
    repo: String,
    store: Option<SnapshotStore>,
    tx: UnboundedSender<AppMsg>,
) -> bool {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return false;
    };
    handle.spawn(async move {
        let result = fetch_repository_assignees(&repo)
            .await
            .map_err(|error| error.to_string());
        if let Ok(reviewers) = &result
            && let Some(store) = &store
            && let Err(error) = store.save_reviewer_candidates(&repo, reviewers)
        {
            warn!(error = %error, repo = %repo, "failed to persist reviewer candidates");
        }
        let _ = tx.send(AppMsg::ReviewerSuggestionsLoaded { repo, result });
    });
    true
}

pub(super) fn start_mention_user_search_load(query: String, tx: UnboundedSender<AppMsg>) -> bool {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return false;
    };
    handle.spawn(async move {
        let result = search_github_users(&query)
            .await
            .map_err(|error| error.to_string());
        let _ = tx.send(AppMsg::MentionUserSearchLoaded { query, result });
    });
    true
}

pub(super) fn start_issue_create(
    pending: PendingIssueCreate,
    draft_clear: Option<DraftClearTask>,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let result = create_issue(
            &pending.repo,
            &pending.title,
            &pending.body,
            &pending.labels,
        )
        .await
        .map_err(error_chain_message);
        if result.is_ok() {
            clear_editor_draft_after_success(draft_clear);
        }
        let _ = tx.send(AppMsg::IssueCreated { result });
    });
}

pub(super) fn start_pr_create(
    pending: PendingPrCreate,
    draft_clear: Option<DraftClearTask>,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let result = create_pull_request(
            &pending.repo,
            &pending.local_dir,
            &pending.head_ref,
            &pending.title,
            &pending.body,
        )
        .await
        .map_err(error_chain_message);
        if result.is_ok() {
            clear_editor_draft_after_success(draft_clear);
        }
        let _ = tx.send(AppMsg::PullRequestCreated { result });
    });
}

pub(super) fn start_pr_action(
    item: WorkItem,
    action: PrAction,
    checkout: Option<PrCheckoutPlan>,
    merge_method: Option<MergeMethod>,
    config: Config,
    store: SnapshotStore,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let item_kind = item.kind;
        if action == PrAction::Checkout {
            let Some(checkout) = checkout else {
                let _ = tx.send(AppMsg::PrCheckoutFinished {
                    result: Err("missing local checkout target".to_string()),
                });
                return;
            };
            let result = run_pr_checkout(item, checkout.directory).await;
            let _ = tx.send(AppMsg::PrCheckoutFinished { result });
            return;
        }

        let result = match item.number {
            Some(number) => match action {
                PrAction::Merge if item.kind == ItemKind::PullRequest => {
                    merge_pull_request(&item.repo, number, merge_method.unwrap_or_default())
                        .await
                        .map_err(|error| error.to_string())
                }
                PrAction::Close if item.kind == ItemKind::PullRequest => {
                    close_pull_request(&item.repo, number)
                        .await
                        .map_err(|error| error.to_string())
                }
                PrAction::Close if item.kind == ItemKind::Issue => close_issue(&item.repo, number)
                    .await
                    .map_err(|error| error.to_string()),
                PrAction::Reopen if item.kind == ItemKind::PullRequest => {
                    reopen_pull_request(&item.repo, number)
                        .await
                        .map_err(|error| error.to_string())
                }
                PrAction::Reopen if item.kind == ItemKind::Issue => {
                    reopen_issue(&item.repo, number)
                        .await
                        .map_err(|error| error.to_string())
                }
                PrAction::Approve if item.kind == ItemKind::PullRequest => {
                    approve_pull_request(&item.repo, number)
                        .await
                        .map_err(|error| error.to_string())
                }
                PrAction::EnableAutoMerge if item.kind == ItemKind::PullRequest => {
                    enable_pull_request_auto_merge(&item.repo, number)
                        .await
                        .map_err(|error| error.to_string())
                }
                PrAction::DisableAutoMerge if item.kind == ItemKind::PullRequest => {
                    disable_pull_request_auto_merge(&item.repo, number)
                        .await
                        .map_err(|error| error.to_string())
                }
                PrAction::Checkout => unreachable!("checkout is handled before remote PR actions"),
                PrAction::RerunFailedChecks if item.kind == ItemKind::PullRequest => {
                    rerun_failed_pull_request_checks(&item.repo, number)
                        .await
                        .map_err(|error| error.to_string())
                }
                PrAction::UpdateBranch if item.kind == ItemKind::PullRequest => {
                    update_pull_request_branch(&item.repo, number)
                        .await
                        .map_err(|error| error.to_string())
                }
                PrAction::ConvertToDraft if item.kind == ItemKind::PullRequest => {
                    convert_pull_request_to_draft(&item.repo, number)
                        .await
                        .map_err(|error| error.to_string())
                }
                PrAction::MarkReadyForReview if item.kind == ItemKind::PullRequest => {
                    mark_pull_request_ready_for_review(&item.repo, number)
                        .await
                        .map_err(|error| error.to_string())
                }
                PrAction::Merge
                | PrAction::Approve
                | PrAction::EnableAutoMerge
                | PrAction::DisableAutoMerge
                | PrAction::RerunFailedChecks
                | PrAction::UpdateBranch
                | PrAction::ConvertToDraft
                | PrAction::MarkReadyForReview => {
                    Err("selected item is not a pull request".to_string())
                }
                PrAction::Close | PrAction::Reopen => {
                    Err("selected item is not an issue or pull request".to_string())
                }
            },
            None => Err("selected item has no issue or pull request number".to_string()),
        };
        let should_refresh = result.is_ok();
        let _ = tx.send(AppMsg::PrActionFinished {
            item_id,
            item_kind,
            action,
            merge_method,
            result,
        });

        if should_refresh {
            if let Some(number) = item.number {
                let item_id = item.id.clone();
                let actions = fetch_pull_request_action_hints(&item.repo, number)
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(AppMsg::ActionHintsLoaded { item_id, actions });
            }
            let _ = tx.send(AppMsg::RefreshStarted {
                scope: RefreshScope::Full,
            });
            let sections = with_background_github_priority(refresh_dashboard(&config)).await;
            let mut save_error = None;
            for section in &sections {
                if section.error.is_some() {
                    continue;
                }
                if let Err(error) = store.save_section(section) {
                    let message = error.to_string();
                    warn!(error = %message, "failed to save refreshed snapshot after PR action");
                    save_error = Some(message);
                    break;
                }
            }
            let _ = tx.send(AppMsg::RefreshFinished {
                sections,
                save_error,
            });
        }
    });
}

pub(super) fn start_milestones_load(item: WorkItem, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = fetch_open_milestones(&item.repo)
            .await
            .map_err(|error| error.to_string());
        let _ = tx.send(AppMsg::MilestonesLoaded { item_id, result });
    });
}

pub(super) fn start_milestone_change(
    item: WorkItem,
    choice: MilestoneChoice,
    config: Config,
    store: SnapshotStore,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let mut changed_milestone = None;
        let result = match item.number {
            Some(number) => match resolve_milestone_choice(&item.repo, choice).await {
                Ok(milestone) => {
                    changed_milestone = milestone.clone();
                    change_issue_milestone(&item.repo, number, milestone.as_ref().map(|m| m.number))
                        .await
                        .map_err(|error| error.to_string())
                }
                Err(error) => Err(error),
            },
            None => Err("selected item has no issue or pull request number".to_string()),
        };
        let should_refresh = result.is_ok();
        let _ = tx.send(AppMsg::MilestoneChanged {
            item_id,
            milestone: changed_milestone,
            result,
        });

        if should_refresh {
            let _ = tx.send(AppMsg::RefreshStarted {
                scope: RefreshScope::Full,
            });
            let sections = with_background_github_priority(refresh_dashboard(&config)).await;
            let mut save_error = None;
            for section in &sections {
                if section.error.is_some() {
                    continue;
                }
                if let Err(error) = store.save_section(section) {
                    let message = error.to_string();
                    warn!(error = %message, "failed to save refreshed snapshot after milestone change");
                    save_error = Some(message);
                    break;
                }
            }
            let _ = tx.send(AppMsg::RefreshFinished {
                sections,
                save_error,
            });
        }
    });
}

pub(super) async fn resolve_milestone_choice(
    repository: &str,
    choice: MilestoneChoice,
) -> std::result::Result<Option<Milestone>, String> {
    match choice {
        MilestoneChoice::Clear => Ok(None),
        MilestoneChoice::Set(milestone) => Ok(Some(milestone)),
        MilestoneChoice::Create(title) => create_milestone(repository, &title)
            .await
            .map(Some)
            .map_err(|error| error.to_string()),
    }
}

pub(super) fn start_reviewer_action(
    item: WorkItem,
    action: ReviewerAction,
    reviewers: Vec<String>,
    config: Config,
    store: SnapshotStore,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = match item.number {
            Some(number) => match action {
                ReviewerAction::Request => {
                    request_pull_request_reviewers(&item.repo, number, &reviewers)
                        .await
                        .map_err(|error| error.to_string())
                }
                ReviewerAction::Remove => {
                    remove_pull_request_reviewers(&item.repo, number, &reviewers)
                        .await
                        .map_err(|error| error.to_string())
                }
            },
            None => Err("selected item has no pull request number".to_string()),
        };
        let should_refresh = result.is_ok();
        let _ = tx.send(AppMsg::ReviewerActionFinished {
            item_id,
            action,
            reviewers,
            result,
        });

        if should_refresh {
            let _ = tx.send(AppMsg::RefreshStarted {
                scope: RefreshScope::Full,
            });
            let sections = with_background_github_priority(refresh_dashboard(&config)).await;
            let mut save_error = None;
            for section in &sections {
                if section.error.is_some() {
                    continue;
                }
                if let Err(error) = store.save_section(section) {
                    let message = error.to_string();
                    warn!(error = %message, "failed to save refreshed snapshot after reviewer action");
                    save_error = Some(message);
                    break;
                }
            }
            let _ = tx.send(AppMsg::RefreshFinished {
                sections,
                save_error,
            });
        }
    });
}

pub(super) fn start_assignee_update(
    item: WorkItem,
    action: AssigneeAction,
    assignees: Vec<String>,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = match item.number {
            Some(number) => {
                update_issue_assignees(&item.repo, number, item.kind, action, &assignees)
                    .await
                    .map_err(|error| error.to_string())
            }
            None => Err("selected item has no issue or pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::AssigneesUpdated {
            item_id,
            action,
            result,
        });
    });
}
