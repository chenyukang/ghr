use super::*;

impl AppState {
    pub(super) fn show_top_menu_switcher(&mut self) {
        let candidates = self.top_menu_switcher_candidates();
        if candidates.is_empty() {
            self.top_menu_switcher = None;
            self.status = "no top menu items".to_string();
            return;
        }

        let selected = candidates
            .iter()
            .position(|candidate| candidate.key == self.active_view)
            .unwrap_or(0);
        self.finish_details_visit(Instant::now());
        self.command_palette = None;
        self.project_switcher = None;
        self.theme_switcher = None;
        self.recent_items_dialog = None;
        self.project_add_dialog = None;
        self.project_remove_dialog = None;
        self.cache_clear_dialog = None;
        self.top_menu_switcher = Some(TopMenuSwitcher {
            query: String::new(),
            selected,
        });
        self.status = "top menu switch".to_string();
    }

    pub(super) fn dismiss_top_menu_switcher(&mut self) {
        self.top_menu_switcher = None;
        self.status = "top menu switch cancelled".to_string();
    }

    pub(super) fn handle_top_menu_switcher_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.dismiss_top_menu_switcher(),
            KeyCode::Enter => self.submit_top_menu_switcher_selection(),
            KeyCode::Down | KeyCode::Tab => self.move_top_menu_switcher_selection(1),
            KeyCode::Up | KeyCode::BackTab => self.move_top_menu_switcher_selection(-1),
            KeyCode::PageDown => self.move_top_menu_switcher_selection(8),
            KeyCode::PageUp => self.move_top_menu_switcher_selection(-8),
            KeyCode::Backspace => {
                if let Some(switcher) = &mut self.top_menu_switcher {
                    switcher.query.pop();
                    switcher.selected = 0;
                }
            }
            KeyCode::Char(value)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(switcher) = &mut self.top_menu_switcher {
                    switcher.query.push(value);
                    switcher.selected = 0;
                }
            }
            _ => {}
        }
    }

    pub(super) fn move_top_menu_switcher_selection(&mut self, delta: isize) {
        let Some(query) = self
            .top_menu_switcher
            .as_ref()
            .map(|switcher| switcher.query.clone())
        else {
            return;
        };
        let len = self.top_menu_switcher_candidates_for_query(&query).len();
        if let Some(switcher) = &mut self.top_menu_switcher {
            switcher.selected = move_wrapping(switcher.selected, len, delta);
        }
    }

    pub(super) fn submit_top_menu_switcher_selection(&mut self) {
        let Some(switcher) = &self.top_menu_switcher else {
            return;
        };
        let candidates = self.top_menu_switcher_candidates_for_query(&switcher.query);
        let selected = switcher.selected.min(candidates.len().saturating_sub(1));
        let Some(candidate) = candidates.get(selected) else {
            self.status = "no matching top menu item".to_string();
            return;
        };

        let key = candidate.key.clone();
        let label = candidate.label.clone();
        self.top_menu_switcher = None;
        self.switch_top_menu_view(key);
        self.status = format!("top menu switched: {label}");
    }

    pub(super) fn top_menu_switcher_candidates(&self) -> Vec<ViewTab> {
        self.view_tabs()
    }

    pub(super) fn top_menu_switcher_candidates_for_query(&self, query: &str) -> Vec<ViewTab> {
        self.top_menu_switcher_candidates()
            .into_iter()
            .filter(|view| project_switcher_candidate_matches(view, query))
            .collect()
    }

    pub(super) fn show_theme_switcher(&mut self, config: &Config) {
        let candidates = self.theme_switcher_candidates();
        let current = Self::configured_theme_choice(config);
        let selected = candidates
            .iter()
            .position(|candidate| candidate.choice == current)
            .unwrap_or(0);
        self.finish_details_visit(Instant::now());
        self.command_palette = None;
        self.project_switcher = None;
        self.top_menu_switcher = None;
        self.recent_items_dialog = None;
        self.project_add_dialog = None;
        self.project_remove_dialog = None;
        self.cache_clear_dialog = None;
        self.theme_switcher = Some(ThemeSwitcher {
            query: String::new(),
            selected,
            current,
        });
        self.status = "set color theme".to_string();
    }

    pub(super) fn dismiss_theme_switcher(&mut self) {
        self.theme_switcher = None;
        self.status = "set color theme cancelled".to_string();
    }

    pub(super) fn handle_theme_switcher_key(
        &mut self,
        key: KeyEvent,
        config: &mut Config,
        paths: &Paths,
    ) {
        match key.code {
            KeyCode::Esc => self.dismiss_theme_switcher(),
            KeyCode::Enter => self.submit_theme_switcher_selection(config, paths),
            KeyCode::Down | KeyCode::Tab => self.move_theme_switcher_selection(1),
            KeyCode::Up | KeyCode::BackTab => self.move_theme_switcher_selection(-1),
            KeyCode::PageDown => self.move_theme_switcher_selection(8),
            KeyCode::PageUp => self.move_theme_switcher_selection(-8),
            KeyCode::Backspace => {
                if let Some(switcher) = &mut self.theme_switcher {
                    switcher.query.pop();
                    switcher.selected = 0;
                }
            }
            KeyCode::Char(value)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(switcher) = &mut self.theme_switcher {
                    switcher.query.push(value);
                    switcher.selected = 0;
                }
            }
            _ => {}
        }
    }

    pub(super) fn move_theme_switcher_selection(&mut self, delta: isize) {
        let Some(query) = self
            .theme_switcher
            .as_ref()
            .map(|switcher| switcher.query.clone())
        else {
            return;
        };
        let len = self.theme_switcher_candidates_for_query(&query).len();
        if let Some(switcher) = &mut self.theme_switcher {
            switcher.selected = move_wrapping(switcher.selected, len, delta);
        }
    }

    pub(super) fn submit_theme_switcher_selection(&mut self, config: &mut Config, paths: &Paths) {
        let Some(switcher) = &self.theme_switcher else {
            return;
        };
        let candidates = self.theme_switcher_candidates_for_query(&switcher.query);
        let selected = switcher.selected.min(candidates.len().saturating_sub(1));
        let Some(candidate) = candidates.get(selected) else {
            self.status = "no matching color theme".to_string();
            return;
        };

        let choice = candidate.choice;
        self.theme_switcher = None;
        self.set_color_theme(choice, config, paths);
    }

    pub(super) fn theme_switcher_candidates(&self) -> Vec<ThemeCandidate> {
        let mut candidates = vec![ThemeCandidate {
            choice: ThemeChoice::Auto,
            label: "auto".to_string(),
            detail: "follow system appearance".to_string(),
        }];
        candidates.extend(ThemeName::ALL.iter().map(|theme_name| ThemeCandidate {
            choice: ThemeChoice::Name(*theme_name),
            label: theme_name.as_str().to_string(),
            detail: format!("fixed {} theme", theme_family_label(theme_name.family())),
        }));
        candidates
    }

    pub(super) fn theme_switcher_candidates_for_query(&self, query: &str) -> Vec<ThemeCandidate> {
        self.theme_switcher_candidates()
            .into_iter()
            .filter(|candidate| theme_candidate_matches(candidate, query))
            .collect()
    }

    pub(super) fn show_project_switcher(&mut self) {
        let candidates = self.project_switcher_candidates();
        if candidates.is_empty() {
            self.project_switcher = None;
            self.status = "no configured projects".to_string();
            return;
        }

        let selected = candidates
            .iter()
            .position(|candidate| candidate.key == self.active_view)
            .unwrap_or(0);
        self.finish_details_visit(Instant::now());
        self.command_palette = None;
        self.top_menu_switcher = None;
        self.theme_switcher = None;
        self.recent_items_dialog = None;
        self.project_add_dialog = None;
        self.project_remove_dialog = None;
        self.cache_clear_dialog = None;
        self.project_switcher = Some(ProjectSwitcher {
            query: String::new(),
            selected,
        });
        self.status = "project switch".to_string();
    }

    pub(super) fn dismiss_project_switcher(&mut self) {
        self.project_switcher = None;
        self.status = "project switch cancelled".to_string();
    }

    pub(super) fn handle_project_switcher_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.dismiss_project_switcher(),
            KeyCode::Enter => self.submit_project_switcher_selection(),
            KeyCode::Down | KeyCode::Tab => self.move_project_switcher_selection(1),
            KeyCode::Up | KeyCode::BackTab => self.move_project_switcher_selection(-1),
            KeyCode::PageDown => self.move_project_switcher_selection(8),
            KeyCode::PageUp => self.move_project_switcher_selection(-8),
            KeyCode::Backspace => {
                if let Some(switcher) = &mut self.project_switcher {
                    switcher.query.pop();
                    switcher.selected = 0;
                }
            }
            KeyCode::Char(value)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(switcher) = &mut self.project_switcher {
                    switcher.query.push(value);
                    switcher.selected = 0;
                }
            }
            _ => {}
        }
    }

    pub(super) fn move_project_switcher_selection(&mut self, delta: isize) {
        let Some(query) = self
            .project_switcher
            .as_ref()
            .map(|switcher| switcher.query.clone())
        else {
            return;
        };
        let len = self.project_switcher_candidates_for_query(&query).len();
        if let Some(switcher) = &mut self.project_switcher {
            switcher.selected = move_wrapping(switcher.selected, len, delta);
        }
    }

    pub(super) fn submit_project_switcher_selection(&mut self) {
        let Some(switcher) = &self.project_switcher else {
            return;
        };
        let candidates = self.project_switcher_candidates_for_query(&switcher.query);
        let selected = switcher.selected.min(candidates.len().saturating_sub(1));
        let Some(candidate) = candidates.get(selected) else {
            self.status = "no matching project".to_string();
            return;
        };

        let key = candidate.key.clone();
        let label = candidate.label.clone();
        self.project_switcher = None;
        self.switch_project_view(key);
        self.status = format!("project switched: {label}");
    }

    pub(super) fn project_switcher_candidates(&self) -> Vec<ViewTab> {
        self.view_tabs()
            .into_iter()
            .filter(|view| view.key.starts_with("repo:"))
            .collect()
    }

    pub(super) fn project_switcher_candidates_for_query(&self, query: &str) -> Vec<ViewTab> {
        self.project_switcher_candidates()
            .into_iter()
            .filter(|view| project_switcher_candidate_matches(view, query))
            .collect()
    }

    pub(super) fn show_recent_items_dialog(&mut self) {
        let now = Instant::now();
        self.sync_recent_details_visit(now);
        self.finish_details_visit(now);
        self.command_palette = None;
        self.project_switcher = None;
        self.top_menu_switcher = None;
        self.theme_switcher = None;
        self.recent_items_dialog = None;
        self.project_add_dialog = None;
        self.project_remove_dialog = None;
        self.cache_clear_dialog = None;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.filter_input_active = false;
        self.recent_items_dialog = Some(RecentItemsDialog::default());
        self.status = "recent items".to_string();
    }

    pub(super) fn dismiss_recent_items_dialog(&mut self) {
        self.recent_items_dialog = None;
        self.status = "recent items closed".to_string();
    }

    pub(super) fn handle_recent_items_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.dismiss_recent_items_dialog(),
            KeyCode::Enter => self.submit_recent_item_selection(),
            KeyCode::Down | KeyCode::Tab => self.move_recent_item_selection(1),
            KeyCode::Up | KeyCode::BackTab => self.move_recent_item_selection(-1),
            KeyCode::PageDown => self.move_recent_item_selection(8),
            KeyCode::PageUp => self.move_recent_item_selection(-8),
            KeyCode::Backspace => {
                if let Some(dialog) = &mut self.recent_items_dialog {
                    dialog.query.pop();
                    dialog.selected = 0;
                }
            }
            KeyCode::Char(value)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(dialog) = &mut self.recent_items_dialog {
                    dialog.query.push(value);
                    dialog.selected = 0;
                }
            }
            _ => {}
        }
    }

    pub(super) fn move_recent_item_selection(&mut self, delta: isize) {
        let Some(query) = self
            .recent_items_dialog
            .as_ref()
            .map(|dialog| dialog.query.clone())
        else {
            return;
        };
        let len = self.recent_item_candidates_for_query(&query).len();
        if let Some(dialog) = &mut self.recent_items_dialog {
            dialog.selected = move_wrapping(dialog.selected, len, delta);
        }
    }

    pub(super) fn submit_recent_item_selection(&mut self) {
        self.sync_recent_details_visit(Instant::now());
        let Some(dialog) = &self.recent_items_dialog else {
            return;
        };
        let candidates = self.recent_item_candidates_for_query(&dialog.query);
        let selected = dialog.selected.min(candidates.len().saturating_sub(1));
        let Some(candidate) = candidates.get(selected).cloned() else {
            self.status = "no matching recent item".to_string();
            return;
        };

        self.jump_to_recent_item(&candidate);
    }

    pub(super) fn recent_item_candidates_for_query(&self, query: &str) -> Vec<RecentItem> {
        let query_is_empty = query.trim().is_empty();
        self.recent_items
            .iter()
            .filter(|item| !query_is_empty || !self.recent_item_matches_current_item(item))
            .filter(|item| recent_item_matches_query(item, query))
            .cloned()
            .collect()
    }

    pub(super) fn recent_item_matches_current_item(&self, item: &RecentItem) -> bool {
        self.current_item()
            .is_some_and(|current| recent_item_matches_work_item(item, current))
    }

    pub(super) fn show_project_add_dialog(&mut self) {
        self.finish_details_visit(Instant::now());
        self.command_palette = None;
        self.current_repo_remote_dialog = None;
        self.project_switcher = None;
        self.top_menu_switcher = None;
        self.theme_switcher = None;
        self.recent_items_dialog = None;
        self.project_remove_dialog = None;
        self.cache_clear_dialog = None;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.filter_input_active = false;
        self.comment_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.pr_create_dialog = None;
        self.reaction_dialog = None;
        self.review_submit_dialog = None;
        self.item_edit_dialog = None;
        self.milestone_dialog = None;
        self.assignee_dialog = None;
        self.reviewer_dialog = None;
        self.project_add_dialog = Some(ProjectAddDialog {
            title: String::new(),
            repo_url: String::new(),
            local_dir: String::new(),
            field: ProjectAddField::RepoUrl,
        });
        self.status = "project add".to_string();
    }

    pub(super) fn handle_current_repo_remote_key(
        &mut self,
        key: KeyEvent,
        config: &mut Config,
        paths: &Paths,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        match key.code {
            KeyCode::Esc => self.dismiss_current_repo_remote_dialog(config, store, tx),
            KeyCode::Enter => self.confirm_current_repo_remote(config, paths, store, tx),
            KeyCode::Down | KeyCode::Tab => self.move_current_repo_remote_selection(1),
            KeyCode::Up | KeyCode::BackTab => self.move_current_repo_remote_selection(-1),
            _ => {}
        }
    }

    pub(super) fn move_current_repo_remote_selection(&mut self, delta: isize) {
        let Some(dialog) = &mut self.current_repo_remote_dialog else {
            return;
        };
        dialog.selected = move_wrapping(dialog.selected, dialog.candidates.len(), delta);
        self.status = "choose git remote for current repo".to_string();
    }

    pub(super) fn dismiss_current_repo_remote_dialog(
        &mut self,
        config: &Config,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        self.current_repo_remote_dialog = None;
        self.status = "current repo remote skipped".to_string();
        #[cfg(not(test))]
        trigger_refresh(self, config, store, tx);
        #[cfg(test)]
        let _ = (config, store, tx);
    }

    pub(super) fn confirm_current_repo_remote(
        &mut self,
        config: &mut Config,
        paths: &Paths,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let Some(dialog) = self.current_repo_remote_dialog.take() else {
            return;
        };
        let selected = dialog
            .selected
            .min(dialog.candidates.len().saturating_sub(1));
        let Some(candidate) = dialog.candidates.get(selected).cloned() else {
            self.current_repo_remote_dialog = Some(dialog);
            self.status = "no git remote selected".to_string();
            return;
        };

        let previous_config = config.clone();
        config.add_runtime_repo_with_local_dir_and_remote(
            candidate.repo.clone(),
            Some(dialog.directory.display().to_string()),
            repo_remote_config_value(&candidate.remote),
        );
        if let Err(error) = config.save(&paths.config_path) {
            *config = previous_config;
            self.current_repo_remote_dialog = Some(dialog);
            self.status = format!("remote selection save failed: {error}");
            return;
        }

        let name = config
            .repo_name_for_repo(&candidate.repo)
            .map(str::to_string)
            .unwrap_or_else(|| candidate.repo.clone());
        self.add_project_view_from_config(config, &name);
        self.switch_project_view(repo_view_key(&name));
        self.status = format!(
            "current repo remote: {} -> {}",
            candidate.remote, candidate.repo
        );
        #[cfg(not(test))]
        trigger_refresh(self, config, store, tx);
        #[cfg(test)]
        let _ = (store, tx);
    }

    pub(super) fn dismiss_project_add_dialog(&mut self) {
        self.project_add_dialog = None;
        self.status = "project add cancelled".to_string();
    }

    pub(super) fn handle_project_add_key(
        &mut self,
        key: KeyEvent,
        config: &mut Config,
        paths: &Paths,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        if is_comment_submit_key(key) {
            self.confirm_project_add(config, paths, store, tx);
            return;
        }

        match key.code {
            KeyCode::Esc => self.dismiss_project_add_dialog(),
            KeyCode::Enter | KeyCode::Tab => self.move_project_add_field(1),
            KeyCode::BackTab => self.move_project_add_field(-1),
            KeyCode::Backspace => self.pop_project_add_char(),
            KeyCode::Char(value)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.push_project_add_char(value);
            }
            _ => {}
        }
    }

    pub(super) fn move_project_add_field(&mut self, delta: isize) {
        if let Some(dialog) = &mut self.project_add_dialog {
            dialog.field = dialog.field.next(delta);
            self.status = match dialog.field {
                ProjectAddField::Title => "editing project title".to_string(),
                ProjectAddField::RepoUrl => "editing project repo url".to_string(),
                ProjectAddField::LocalDir => "editing project local_dir".to_string(),
            };
        }
    }

    pub(super) fn push_project_add_char(&mut self, value: char) {
        let Some(dialog) = &mut self.project_add_dialog else {
            return;
        };
        match dialog.field {
            ProjectAddField::Title => dialog.title.push(value),
            ProjectAddField::RepoUrl => dialog.repo_url.push(value),
            ProjectAddField::LocalDir => dialog.local_dir.push(value),
        }
    }

    pub(super) fn pop_project_add_char(&mut self) {
        let Some(dialog) = &mut self.project_add_dialog else {
            return;
        };
        match dialog.field {
            ProjectAddField::Title => {
                dialog.title.pop();
            }
            ProjectAddField::RepoUrl => {
                dialog.repo_url.pop();
            }
            ProjectAddField::LocalDir => {
                dialog.local_dir.pop();
            }
        }
    }

    pub(super) fn confirm_project_add(
        &mut self,
        config: &mut Config,
        paths: &Paths,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let Some(dialog) = self.project_add_dialog.take() else {
            return;
        };
        let Some(repo) = project_add_repo_from_input(&dialog.repo_url) else {
            self.project_add_dialog = Some(dialog);
            self.status = "repo url must be a GitHub repo".to_string();
            return;
        };
        if config
            .repos
            .iter()
            .any(|configured| configured.repo.eq_ignore_ascii_case(&repo))
        {
            self.project_add_dialog = Some(dialog);
            self.status = format!("project already configured: {repo}");
            return;
        }
        let Some(name) = project_add_title(config, &dialog.title, &repo) else {
            self.project_add_dialog = Some(dialog);
            self.status = "project title already exists".to_string();
            return;
        };

        let repo_config = RepoConfig {
            name: name.clone(),
            repo: repo.clone(),
            remote: None,
            local_dir: Some(dialog.local_dir.trim().to_string()),
            show_prs: true,
            show_issues: true,
            labels: Vec::new(),
            pr_labels: Vec::new(),
            issue_labels: Vec::new(),
        };
        config.repos.push(repo_config.clone());
        if let Err(error) = config.save(&paths.config_path) {
            config.repos.pop();
            self.project_add_dialog = Some(dialog);
            self.status = format!("project add failed: {error}");
            return;
        }

        self.add_project_view_from_config(config, &name);
        self.switch_project_view(repo_view_key(&name));
        self.project_add_dialog = None;
        self.status = format!("project added: {name}");
        #[cfg(not(test))]
        trigger_refresh(self, config, store, tx);
        #[cfg(test)]
        let _ = (store, tx);
    }

    pub(super) fn add_project_view_from_config(&mut self, config: &Config, name: &str) {
        let view = repo_view_key(name);
        let existing = self
            .sections
            .iter()
            .map(|section| section.key.clone())
            .collect::<HashSet<_>>();
        let sections = configured_sections(config)
            .into_iter()
            .filter(|section| section_view_key(section) == view)
            .filter(|section| !existing.contains(&section.key))
            .collect::<Vec<_>>();
        for section in sections {
            self.remember_base_filters(&section);
            self.sections.push(section);
        }
        self.clamp_positions();
    }

    pub(super) fn show_project_remove_dialog(&mut self, config: &Config) {
        let candidates = config
            .repos
            .iter()
            .enumerate()
            .map(|(index, repo)| ProjectRemoveCandidate {
                index,
                name: repo.name.clone(),
                repo: repo.repo.clone(),
                local_dir: repo.local_dir.clone(),
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            self.project_remove_dialog = None;
            self.status = "no configured projects".to_string();
            return;
        }

        let active_project = self.active_view.strip_prefix("repo:");
        let selected = active_project
            .and_then(|active| {
                candidates
                    .iter()
                    .position(|candidate| candidate.name.eq_ignore_ascii_case(active))
            })
            .unwrap_or(0);
        self.finish_details_visit(Instant::now());
        self.command_palette = None;
        self.project_switcher = None;
        self.top_menu_switcher = None;
        self.theme_switcher = None;
        self.recent_items_dialog = None;
        self.project_add_dialog = None;
        self.cache_clear_dialog = None;
        self.project_remove_dialog = Some(ProjectRemoveDialog {
            query: String::new(),
            selected,
            candidates,
            confirm: None,
        });
        self.status = "project remove".to_string();
    }

    pub(super) fn dismiss_project_remove_dialog(&mut self) {
        self.project_remove_dialog = None;
        self.status = "project remove cancelled".to_string();
    }

    pub(super) fn handle_project_remove_key(
        &mut self,
        key: KeyEvent,
        config: &mut Config,
        paths: &Paths,
    ) {
        if self
            .project_remove_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.confirm.is_some())
        {
            match key.code {
                KeyCode::Esc => self.dismiss_project_remove_dialog(),
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_project_remove(config, paths)
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc => self.dismiss_project_remove_dialog(),
            KeyCode::Enter => self.start_project_remove_confirmation(),
            KeyCode::Down | KeyCode::Tab => self.move_project_remove_selection(1),
            KeyCode::Up | KeyCode::BackTab => self.move_project_remove_selection(-1),
            KeyCode::PageDown => self.move_project_remove_selection(8),
            KeyCode::PageUp => self.move_project_remove_selection(-8),
            KeyCode::Backspace => {
                if let Some(dialog) = &mut self.project_remove_dialog {
                    dialog.query.pop();
                    dialog.selected = 0;
                }
            }
            KeyCode::Char(value)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(dialog) = &mut self.project_remove_dialog {
                    dialog.query.push(value);
                    dialog.selected = 0;
                }
            }
            _ => {}
        }
    }

    pub(super) fn move_project_remove_selection(&mut self, delta: isize) {
        let Some(dialog) = &mut self.project_remove_dialog else {
            return;
        };
        let len = project_remove_filtered_candidates(dialog).len();
        dialog.selected = move_wrapping(dialog.selected, len, delta);
    }

    pub(super) fn start_project_remove_confirmation(&mut self) {
        let Some(candidate) = self.selected_project_remove_candidate() else {
            self.status = "no matching project".to_string();
            return;
        };
        if let Some(dialog) = &mut self.project_remove_dialog {
            dialog.confirm = Some(candidate.clone());
        }
        self.status = format!("confirm remove project {}", candidate.name);
    }

    pub(super) fn selected_project_remove_candidate(&self) -> Option<ProjectRemoveCandidate> {
        let dialog = self.project_remove_dialog.as_ref()?;
        let matches = project_remove_filtered_candidates(dialog);
        let selected = dialog.selected.min(matches.len().saturating_sub(1));
        matches.get(selected).map(|candidate| (*candidate).clone())
    }

    pub(super) fn confirm_project_remove(&mut self, config: &mut Config, paths: &Paths) {
        let Some(candidate) = self
            .project_remove_dialog
            .as_ref()
            .and_then(|dialog| dialog.confirm.clone())
        else {
            return;
        };
        let Some(index) = project_remove_candidate_config_index(config, &candidate) else {
            self.project_remove_dialog = None;
            self.status = format!("project already removed: {}", candidate.name);
            return;
        };
        let Some(removed) = config.remove_repo_at(index) else {
            self.project_remove_dialog = None;
            self.status = format!("project already removed: {}", candidate.name);
            return;
        };

        if let Err(error) = config.save(&paths.config_path) {
            config.repos.insert(index, removed);
            self.project_remove_dialog = None;
            self.status = format!("project remove failed: {error}");
            return;
        }

        self.project_remove_dialog = None;
        self.remove_project_view(&removed.name);
        self.status = format!("project removed: {}", removed.name);
    }

    pub(super) fn remove_project_view(&mut self, name: &str) {
        let view = repo_view_key(name);
        let removed_section_keys = self
            .sections
            .iter()
            .filter(|section| same_view_key(&section_view_key(section), &view))
            .map(|section| section.key.clone())
            .collect::<Vec<_>>();
        self.sections
            .retain(|section| !same_view_key(&section_view_key(section), &view));
        self.section_index.remove(&view);
        self.selected_index.remove(&view);
        for key in removed_section_keys {
            self.list_scroll_offset.remove(&key);
        }
        self.clamp_positions();
    }

    pub(super) fn show_cache_clear_dialog(&mut self) {
        self.finish_details_visit(Instant::now());
        self.command_palette = None;
        self.project_switcher = None;
        self.top_menu_switcher = None;
        self.theme_switcher = None;
        self.recent_items_dialog = None;
        self.project_add_dialog = None;
        self.project_remove_dialog = None;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.filter_input_active = false;
        self.comment_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.pr_create_dialog = None;
        self.reaction_dialog = None;
        self.review_submit_dialog = None;
        self.item_edit_dialog = None;
        self.milestone_dialog = None;
        self.assignee_dialog = None;
        self.reviewer_dialog = None;
        self.cache_clear_dialog = Some(CacheClearDialog::default());
        self.status = "clear cache".to_string();
    }

    pub(super) fn dismiss_cache_clear_dialog(&mut self) {
        self.cache_clear_dialog = None;
        self.status = "clear cache cancelled".to_string();
    }

    pub(super) fn handle_cache_clear_key(&mut self, key: KeyEvent, store: &SnapshotStore) {
        if self
            .cache_clear_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.confirm.is_some())
        {
            match key.code {
                KeyCode::Esc => self.dismiss_cache_clear_dialog(),
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_cache_clear(store)
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc => self.dismiss_cache_clear_dialog(),
            KeyCode::Enter => self.start_cache_clear_confirmation(),
            KeyCode::Down | KeyCode::Tab => self.move_cache_clear_selection(1),
            KeyCode::Up | KeyCode::BackTab => self.move_cache_clear_selection(-1),
            KeyCode::PageDown => self.move_cache_clear_selection(4),
            KeyCode::PageUp => self.move_cache_clear_selection(-4),
            _ => {}
        }
    }

    pub(super) fn move_cache_clear_selection(&mut self, delta: isize) {
        let Some(dialog) = &mut self.cache_clear_dialog else {
            return;
        };
        dialog.selected = move_wrapping(dialog.selected, CACHE_CLEAR_TARGETS.len(), delta);
    }

    pub(super) fn start_cache_clear_confirmation(&mut self) {
        let Some(target) = self.selected_cache_clear_target() else {
            self.status = "no cache target selected".to_string();
            return;
        };
        if let Some(dialog) = &mut self.cache_clear_dialog {
            dialog.confirm = Some(target);
        }
        self.status = format!("confirm clear {}", cache_clear_target_label(target));
    }

    pub(super) fn selected_cache_clear_target(&self) -> Option<CacheClearTarget> {
        let dialog = self.cache_clear_dialog.as_ref()?;
        CACHE_CLEAR_TARGETS
            .get(
                dialog
                    .selected
                    .min(CACHE_CLEAR_TARGETS.len().saturating_sub(1)),
            )
            .copied()
    }

    pub(super) fn confirm_cache_clear(&mut self, store: &SnapshotStore) {
        let Some(target) = self
            .cache_clear_dialog
            .as_ref()
            .and_then(|dialog| dialog.confirm)
        else {
            return;
        };

        self.cache_clear_dialog = None;
        match self.clear_cache(target, store) {
            Ok(summary) => {
                self.status = cache_clear_summary_status(target, summary);
            }
            Err(error) => {
                self.message_dialog = Some(message_dialog(
                    "Clear Cache Failed",
                    operation_error_body(&error.to_string()),
                ));
                self.status = format!("clear cache failed: {error}");
            }
        }
    }

    pub(super) fn clear_cache(
        &mut self,
        target: CacheClearTarget,
        store: &SnapshotStore,
    ) -> Result<CacheClearSummary> {
        let mut summary = CacheClearSummary::default();
        match target {
            CacheClearTarget::CurrentSection => {
                let keys = self.current_section_keys();
                summary.snapshot_rows = store.clear_snapshots_by_keys(&keys)?;
                summary.list_items = self.clear_list_sections_by_keys(&keys);
            }
            CacheClearTarget::CurrentView => {
                let keys = self.current_view_section_keys();
                summary.snapshot_rows = store.clear_snapshots_by_keys(&keys)?;
                summary.list_items = self.clear_list_sections_by_keys(&keys);
            }
            CacheClearTarget::ListSnapshots => {
                summary.snapshot_rows = store.clear_snapshots()?;
                summary.list_items = self.clear_all_list_sections_local();
            }
            CacheClearTarget::CandidateSuggestions => {
                summary.candidate_rows = store.clear_repo_candidate_cache()?;
                summary.candidate_entries = self.clear_candidate_suggestions_local();
            }
            CacheClearTarget::LoadedDetails => {
                summary.loaded_entries = self.clear_loaded_details_local();
            }
            CacheClearTarget::All => {
                summary.snapshot_rows = store.clear_snapshots()?;
                summary.candidate_rows = store.clear_repo_candidate_cache()?;
                summary.list_items = self.clear_all_list_sections_local();
                summary.candidate_entries = self.clear_candidate_suggestions_local();
                summary.loaded_entries = self.clear_loaded_details_local();
            }
        }
        Ok(summary)
    }

    pub(super) fn current_section_keys(&self) -> Vec<String> {
        self.current_section()
            .map(|section| vec![section.key.clone()])
            .unwrap_or_default()
    }

    pub(super) fn current_view_section_keys(&self) -> Vec<String> {
        self.sections
            .iter()
            .filter(|section| same_view_key(&section_view_key(section), &self.active_view))
            .map(|section| section.key.clone())
            .collect()
    }

    pub(super) fn clear_all_list_sections_local(&mut self) -> usize {
        let keys = self
            .sections
            .iter()
            .map(|section| section.key.clone())
            .collect::<Vec<_>>();
        self.clear_list_sections_by_keys(&keys)
    }

    pub(super) fn clear_list_sections_by_keys(&mut self, keys: &[String]) -> usize {
        self.save_current_conversation_details_state();
        let keys = keys.iter().cloned().collect::<HashSet<_>>();
        let mut item_count = 0;
        for section in &mut self.sections {
            if !keys.contains(&section.key) {
                continue;
            }
            item_count += section.items.len();
            section.items.clear();
            section.total_count = None;
            section.page = 1;
            section.page_size = 0;
            section.refreshed_at = None;
            section.error = None;
            self.list_scroll_offset.remove(&section.key);
        }
        self.clamp_positions();
        if item_count > 0 {
            self.reset_or_restore_current_conversation_details_state();
        }
        item_count
    }

    pub(super) fn clear_candidate_suggestions_local(&mut self) -> usize {
        let count = self.label_suggestions_cache.len()
            + self.assignee_suggestions_cache.len()
            + self.reviewer_suggestions_cache.len();
        self.label_suggestions_cache.clear();
        self.assignee_suggestions_cache.clear();
        self.reviewer_suggestions_cache.clear();
        count
    }

    pub(super) fn clear_loaded_details_local(&mut self) -> usize {
        self.save_current_conversation_details_state();
        let count = self.details.len() + self.diffs.len() + self.action_hints.len();
        self.details.clear();
        self.details_refreshed_at.clear();
        self.diffs.clear();
        self.action_hints.clear();
        self.action_hints_stale.clear();
        self.action_hints_refreshing.clear();
        self.details_stale.clear();
        self.details_refreshing.clear();
        self.pending_details_load = None;
        self.reset_or_restore_current_conversation_details_state();
        count
    }
}

pub(super) fn project_switcher_candidate_matches(candidate: &ViewTab, query: &str) -> bool {
    let query = command_palette_normalized_text(query);
    if query.is_empty() {
        return true;
    }

    let label = command_palette_normalized_text(&candidate.label);
    let key = command_palette_normalized_text(
        candidate
            .key
            .strip_prefix("repo:")
            .unwrap_or(candidate.key.as_str()),
    );
    label.starts_with(&query) || key.starts_with(&query)
}

pub(super) fn theme_candidate_matches(candidate: &ThemeCandidate, query: &str) -> bool {
    let query = command_palette_normalized_text(query);
    if query.is_empty() {
        return true;
    }

    let label = command_palette_normalized_text(&candidate.label);
    let detail = command_palette_normalized_text(&candidate.detail);
    label.contains(&query) || detail.contains(&query)
}

pub(super) fn theme_family_label(family: ThemeFamily) -> &'static str {
    match family {
        ThemeFamily::Dark => "dark",
        ThemeFamily::Light => "light",
    }
}

pub(super) fn recent_item_matches_query(item: &RecentItem, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }

    let haystack = format!(
        "{} {} #{} {} {}",
        recent_item_label(item),
        item.repo,
        item.number,
        item.title,
        item.url
    )
    .to_lowercase();
    query
        .split_whitespace()
        .all(|token| fuzzy_score(token, &haystack).is_some())
}

pub(super) fn project_add_repo_from_input(input: &str) -> Option<String> {
    let value = input.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(repo) = github_repo_from_remote_url(value) {
        return Some(repo);
    }

    let path = value
        .split_once("github.com/")
        .map(|(_, path)| path)
        .unwrap_or(value)
        .trim()
        .trim_start_matches('/')
        .trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let owner = parts.next()?.trim();
    let name = parts.next()?.trim();
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    if owner
        .chars()
        .chain(name.chars())
        .any(|value| value.is_whitespace() || matches!(value, ':' | '\\'))
    {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

pub(super) fn project_add_title(config: &Config, title: &str, repo: &str) -> Option<String> {
    let title = title.trim();
    if !title.is_empty() {
        return (!project_title_exists(config, title)).then(|| title.to_string());
    }

    let short_name = repo.rsplit_once('/').map(|(_, name)| name).unwrap_or(repo);
    if !project_title_exists(config, short_name) {
        return Some(short_name.to_string());
    }
    (!project_title_exists(config, repo)).then(|| repo.to_string())
}

pub(super) fn project_title_exists(config: &Config, title: &str) -> bool {
    config
        .repos
        .iter()
        .any(|repo| repo.name.eq_ignore_ascii_case(title))
}

pub(super) fn project_remove_filtered_candidates(
    dialog: &ProjectRemoveDialog,
) -> Vec<&ProjectRemoveCandidate> {
    dialog
        .candidates
        .iter()
        .filter(|candidate| project_remove_candidate_matches(candidate, &dialog.query))
        .collect()
}

pub(super) fn project_remove_candidate_matches(
    candidate: &ProjectRemoveCandidate,
    query: &str,
) -> bool {
    let query = command_palette_normalized_text(query);
    if query.is_empty() {
        return true;
    }

    let name = command_palette_normalized_text(&candidate.name);
    let repo = command_palette_normalized_text(&candidate.repo);
    name.starts_with(&query) || repo.starts_with(&query)
}

pub(super) fn project_remove_candidate_config_index(
    config: &Config,
    candidate: &ProjectRemoveCandidate,
) -> Option<usize> {
    if config
        .repos
        .get(candidate.index)
        .is_some_and(|repo| repo.name == candidate.name && repo.repo == candidate.repo)
    {
        return Some(candidate.index);
    }

    config
        .repos
        .iter()
        .position(|repo| repo.name == candidate.name && repo.repo == candidate.repo)
}
