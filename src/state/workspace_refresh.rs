use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use nsh_core::prelude::blocking;

use super::{
    AppState, app_signal, refresh::OpenReviewDocuments, retry_file_navigation, save_config_quiet,
    spawn_from_root,
};
use crate::git::{self, ChangedFile, RepoInfo, RepoStatus, StashEntry};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Scope {
    Full,
    Repositories(HashSet<String>),
}

impl Scope {
    fn new(ids: Option<HashSet<String>>, inventory_ready: bool) -> Self {
        match ids {
            Some(ids) if inventory_ready && !ids.is_empty() => Self::Repositories(ids),
            _ => Self::Full,
        }
    }

    fn ids(&self) -> Option<&HashSet<String>> {
        match self {
            Self::Full => None,
            Self::Repositories(ids) => Some(ids),
        }
    }

    fn merge(&mut self, other: Self) {
        match other {
            Self::Full => *self = Self::Full,
            Self::Repositories(more) => {
                if let Self::Repositories(ids) = self {
                    ids.extend(more);
                }
            }
        }
    }
}

#[derive(Clone)]
struct ActiveRefresh {
    scope: Scope,
    superseded: Arc<AtomicBool>,
    files_pending: bool,
}

#[derive(Clone, Default)]
pub(super) struct WorkspaceRefresh {
    active: Option<ActiveRefresh>,
    queued: Option<Scope>,
    inventory_ready: bool,
    inventory_error: Option<String>,
    files_ready: bool,
    changes_ready: bool,
    stashes_ready: bool,
    file_errors: BTreeMap<String, String>,
    change_errors: BTreeMap<String, String>,
    stash_errors: BTreeMap<String, String>,
}

impl WorkspaceRefresh {
    fn queue(&mut self, scope: Scope) {
        if let Some(queued) = &mut self.queued {
            queued.merge(scope);
        } else {
            self.queued = Some(scope);
        }
    }

    fn stop_active(&mut self) -> Option<Scope> {
        let active = self.active.take()?;
        active.superseded.store(true, Ordering::Relaxed);
        Some(active.scope)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum WorkspaceData {
    Files,
    Changes,
    Stashes,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceNotice {
    Loading(&'static str),
    Failed(String),
}

impl AppState {
    pub(crate) fn is_refreshing(&self) -> bool {
        self.workspace_refresh.active.is_some()
    }

    pub(super) fn file_scan_pending(&self) -> bool {
        self.command_busy
            || self.workspace_refresh.queued.is_some()
            || self.workspace_refresh.active.as_ref().is_some_and(|active| active.files_pending)
    }

    pub(crate) fn workspace_notice(&self, data: WorkspaceData) -> Option<WorkspaceNotice> {
        let refresh = &self.workspace_refresh;
        if let Some(err) = &refresh.inventory_error {
            return Some(WorkspaceNotice::Failed(err.clone()));
        }
        if !refresh.inventory_ready && self.is_refreshing() {
            return Some(WorkspaceNotice::Loading("正在发现仓库…"));
        }
        let (ready, errors, loading) = match data {
            WorkspaceData::Files => (refresh.files_ready, &refresh.file_errors, "正在读取文件…"),
            WorkspaceData::Changes => (
                refresh.changes_ready,
                &refresh.change_errors,
                "正在读取 Git 状态…",
            ),
            WorkspaceData::Stashes => (
                refresh.stashes_ready,
                &refresh.stash_errors,
                "正在读取 Stash…",
            ),
        };
        if let Some(err) = errors.values().next() {
            return Some(WorkspaceNotice::Failed(err.clone()));
        }
        (!ready && self.is_refreshing()).then_some(WorkspaceNotice::Loading(loading))
    }

    pub(crate) fn stop_workspace_refresh(&mut self) {
        self.workspace_refresh.stop_active();
    }

    pub(super) fn pause_workspace_refresh_for_git_write(&mut self) {
        if let Some(scope) = self.workspace_refresh.stop_active() {
            self.workspace_refresh.queue(scope);
            self.refresh_gen = self.refresh_gen.wrapping_add(1);
        }
    }
}

#[derive(Clone, Copy)]
enum Trigger {
    Explicit,
    Focus,
    Watch,
}

pub fn refresh_workspace_async(only_repos: Option<HashSet<String>>) {
    request(only_repos, Trigger::Explicit);
}

pub(crate) fn refresh_workspace_on_focus() {
    request(None, Trigger::Focus);
}

pub(super) fn refresh_on_watch(only_repos: Option<HashSet<String>>) {
    request(only_repos, Trigger::Watch);
}

fn request(ids: Option<HashSet<String>>, trigger: Trigger) {
    let skip = app_signal().with_untracked(|s| {
        s.cfg.workspace_path.trim().is_empty()
            || (matches!(trigger, Trigger::Focus)
                && (!s.workspace_refresh.files_ready || !s.workspace_refresh.changes_ready)
                && s.workspace_refresh
                    .active
                    .as_ref()
                    .is_some_and(|active| active.scope == Scope::Full))
    });
    if skip {
        return;
    }

    let mut job = None;
    app_signal().update(|s| {
        let refresh = &mut s.workspace_refresh;
        let mut scope = Scope::new(
            ids,
            refresh.inventory_ready && refresh.inventory_error.is_none(),
        );
        if s.command_busy || (refresh.active.is_some() && !matches!(trigger, Trigger::Explicit)) {
            refresh.queue(scope);
            return;
        }
        if let Some(queued) = refresh.queued.take() {
            scope.merge(queued);
        }
        if let Some(active) = refresh.stop_active() {
            scope.merge(active);
        }

        let superseded = Arc::new(AtomicBool::new(false));
        refresh.active = Some(ActiveRefresh {
            scope: scope.clone(),
            superseded: superseded.clone(),
            files_pending: true,
        });
        s.refresh_gen = s.refresh_gen.wrapping_add(1);
        let discovery = (scope == Scope::Full).then(|| Discovery {
            workspace: s.cfg.workspace_path.clone(),
            hidden: s.cfg.hidden_repos.iter().cloned().collect(),
            include_root: s.cfg.extra_bool("include_root_repo", true),
            bindings: s.cfg.workspace_repository_ids(),
        });
        job = Some(RefreshJob {
            generation: s.refresh_gen,
            superseded,
            previous_repos: s.repos.clone(),
            discovery,
            review: OpenReviewDocuments::capture(s, scope.ids()),
            scope,
        });
    });
    if let Some(job) = job {
        spawn_from_root(job.run());
    }
}

struct Discovery {
    workspace: String,
    hidden: HashSet<String>,
    include_root: bool,
    bindings: BTreeMap<String, String>,
}

struct Inventory {
    repos: Vec<RepoInfo>,
    hidden_labels: BTreeMap<String, String>,
    bindings: Option<BTreeMap<String, String>>,
}

impl Discovery {
    fn read(mut self, previous: &[RepoInfo]) -> anyhow::Result<Inventory> {
        let old_bindings = self.bindings.clone();
        let mut repos = git::discover_workspace_repository_inventory(
            &self.workspace,
            previous,
            &mut self.bindings,
        )?;
        let hidden_labels = repos
            .iter()
            .filter(|repo| self.hidden.contains(&repo.id))
            .map(|repo| (repo.id.clone(), repo.label().to_owned()))
            .collect();
        repos
            .retain(|repo| !self.hidden.contains(&repo.id) && (self.include_root || !repo.is_root));
        Ok(Inventory {
            repos,
            hidden_labels,
            bindings: (self.bindings != old_bindings).then_some(self.bindings),
        })
    }
}

struct RefreshJob {
    generation: u64,
    superseded: Arc<AtomicBool>,
    previous_repos: Vec<RepoInfo>,
    discovery: Option<Discovery>,
    review: OpenReviewDocuments,
    scope: Scope,
}

impl RefreshJob {
    async fn run(self) {
        let start = Instant::now();
        let Self {
            generation,
            superseded,
            previous_repos,
            discovery,
            review,
            scope,
        } = self;
        let repos = if let Some(discovery) = discovery {
            let result = blocking(move || discovery.read(&previous_repos)).await;
            let result = result
                .map_err(anyhow::Error::from)
                .and_then(|result| result);
            let inventory = match result {
                Ok(inventory) => inventory,
                Err(err) => {
                    update_current(generation, |s| {
                        s.workspace_refresh.inventory_error = Some(format!("{err:#}"));
                    });
                    finish(generation, start, false);
                    return;
                }
            };
            let repos = inventory.repos.clone();
            let save_bindings = inventory.bindings.is_some();
            if !update_current(generation, |s| apply_inventory(s, inventory)) {
                return;
            }
            if save_bindings {
                save_config_quiet();
            }
            repos
        } else {
            previous_repos
        };
        let targets: Arc<[RepoInfo]> = repos
            .iter()
            .filter(|repo| scope.ids().is_none_or(|ids| ids.contains(&repo.id)))
            .cloned()
            .collect();
        // The three panels can publish independently. Revalidating an open
        // document can keep the older job alive, but can never hold up Files.
        tokio::join!(
            refresh_files(generation, targets.clone(), superseded.clone()),
            refresh_changes(generation, targets.clone(), superseded.clone(), review),
            refresh_stashes(generation, targets, superseded, repos),
        );
        finish(generation, start, true);
    }
}

fn is_current(generation: u64) -> bool {
    app_signal().with_untracked(|s| s.refresh_gen == generation && s.is_refreshing())
}

fn update_current(generation: u64, update: impl FnOnce(&mut AppState)) -> bool {
    if !is_current(generation) {
        return false;
    }
    app_signal().update(update);
    true
}

fn apply_inventory(s: &mut AppState, inventory: Inventory) {
    s.workspace_refresh.inventory_ready = true;
    s.workspace_refresh.inventory_error = None;
    s.hidden_repo_labels = inventory.hidden_labels;
    if let Some(bindings) = inventory.bindings {
        s.cfg
            .repository_id_bindings
            .insert(s.cfg.workspace_path.clone(), bindings);
    }
    if s.repos == inventory.repos {
        return;
    }
    for repo in &inventory.repos {
        if !s.repos.iter().any(|previous| previous.path == repo.path) {
            s.expanded_project_repos.insert(repo.id.clone());
        }
    }
    let repos: HashMap<_, _> = inventory.repos.iter().map(|r| (r.id.as_str(), r)).collect();
    s.expanded_project_repos
        .retain(|id| repos.contains_key(id.as_str()));
    let matching_repo =
        |id: &str, path: &str| repos.get(id).copied().filter(|repo| repo.path == path);
    s.statuses.retain_mut(|status| {
        let Some(repo) = matching_repo(&status.repo.id, &status.repo.path) else {
            return false;
        };
        status.repo.clone_from(repo);
        for file in &mut status.files {
            file.repo_name = repo.label().to_owned();
        }
        true
    });
    if !s.workspace_files.is_empty() {
        let mut files: Vec<_> = s
            .workspace_files
            .iter()
            .filter_map(|file| {
                let repo = matching_repo(&file.repo_id, &file.repo_path)?;
                let mut file = file.clone();
                file.repo_name = repo.label().to_owned();
                file.color_idx = repo.color_idx;
                Some(file)
            })
            .collect();
        git::sort_workspace_files(&mut files);
        s.workspace_files = files.into();
    }
    s.stashes.retain_mut(|stash| {
        let Some(repo) = matching_repo(&stash.repo_id, &stash.repo_path) else {
            return false;
        };
        stash.repo_name = repo.label().to_owned();
        true
    });
    for tab in &mut s.code_tabs {
        if let Some(repo) = matching_repo(&tab.file.repo_id, &tab.file.repo_path) {
            tab.file.repo_name = repo.label().to_owned();
            tab.file.color_idx = repo.color_idx;
        }
    }
    let refresh = &mut s.workspace_refresh;
    for errors in [
        &mut refresh.file_errors,
        &mut refresh.change_errors,
        &mut refresh.stash_errors,
    ] {
        errors.retain(|id, _| id.is_empty() || repos.contains_key(id.as_str()));
    }
    s.repos = inventory.repos;
    reconcile_change_selection(s);
}

type ReadErrors = BTreeMap<String, String>;

struct RepositoryData<T> {
    values: Vec<T>,
    updated: HashSet<String>,
    errors: ReadErrors,
}

fn read_repo_data<T>(
    targets: &[RepoInfo],
    superseded: &AtomicBool,
    read: impl Fn(&RepoInfo) -> anyhow::Result<Vec<T>>,
) -> RepositoryData<T> {
    let mut data = RepositoryData {
        values: Vec::new(),
        updated: HashSet::new(),
        errors: BTreeMap::new(),
    };
    for repo in targets {
        if superseded.load(Ordering::Relaxed) {
            break;
        }
        match read(repo) {
            Ok(values) => {
                data.updated.insert(repo.id.clone());
                data.values.extend(values);
            }
            Err(err) => {
                data.errors
                    .insert(repo.id.clone(), format!("{} · {err:#}", repo.label()));
            }
        }
    }
    data
}

fn update_errors(errors: &mut ReadErrors, targets: &[RepoInfo], updates: ReadErrors) {
    errors.remove("");
    for repo in targets {
        errors.remove(&repo.id);
    }
    errors.extend(updates);
}

async fn refresh_files(generation: u64, targets: Arc<[RepoInfo]>, superseded: Arc<AtomicBool>) {
    if !is_current(generation) {
        return;
    }
    let previous = app_signal().with_untracked(|s| s.workspace_files.clone());
    let repos = targets.clone();
    let result = blocking(move || {
        let mut data = read_repo_data(&repos, &superseded, git::read_repository_files);
        data.values.extend(
            previous
                .iter()
                .filter(|f| !data.updated.contains(&f.repo_id))
                .cloned(),
        );
        git::sort_workspace_files(&mut data.values);
        let files = if data.values == previous.as_ref() {
            previous
        } else {
            data.values.into()
        };
        (files, data.errors)
    })
    .await;
    let mut restore_session = false;
    if !update_current(generation, |s| {
        let refresh = &mut s.workspace_refresh;
        if let Some(active) = &mut refresh.active {
            active.files_pending = false;
        }
        match result {
            Ok((files, errors)) => {
                s.workspace_files = files;
                refresh.files_ready = true;
                update_errors(&mut refresh.file_errors, &targets, errors);
                restore_session = refresh.file_errors.is_empty();
            }
            Err(err) => {
                refresh
                    .file_errors
                    .insert(String::new(), format!("读取文件失败 · {err}"));
            }
        }
    }) {
        return;
    }
    retry_file_navigation();
    if restore_session {
        trace_ready("project_files_ready");
        crate::session_runtime::restore_after_refresh();
    }
}

async fn refresh_changes(
    generation: u64,
    targets: Arc<[RepoInfo]>,
    superseded: Arc<AtomicBool>,
    review: OpenReviewDocuments,
) {
    if !is_current(generation) {
        return;
    }
    let repos = targets.clone();
    let result = blocking(move || {
        repos
            .iter()
            .take_while(|_| !superseded.load(Ordering::Relaxed))
            .map(git::inspect_repo)
            .collect::<Vec<_>>()
    })
    .await;
    if !is_current(generation) {
        return;
    }
    let statuses = match result {
        Ok(statuses) => statuses,
        Err(err) => {
            update_current(generation, |s| {
                s.workspace_refresh
                    .change_errors
                    .insert(String::new(), format!("读取 Git 状态失败 · {err}"));
            });
            return;
        }
    };
    let review = (!review.is_empty()).then(|| {
        let statuses = statuses.clone();
        blocking(move || review.read(&statuses))
    });
    update_current(generation, |s| apply_statuses(s, &targets, statuses));
    if let Some(review) = review {
        if let Ok(mut review) = review.await {
            if !is_current(generation) {
                return;
            }
            review.prepare_diff(generation).await;
            update_current(generation, |s| review.apply(s));
        } else {
            update_current(generation, |s| {
                s.workspace_refresh
                    .change_errors
                    .insert(String::new(), "校验打开的文件失败".into());
            });
        }
    }
}

fn apply_statuses(s: &mut AppState, targets: &[RepoInfo], statuses: Vec<RepoStatus>) {
    let mut next: HashMap<_, _> = std::mem::take(&mut s.statuses)
        .into_iter()
        .map(|status| (status.repo.id.clone(), status))
        .collect();
    let mut errors = BTreeMap::new();
    for mut status in statuses {
        let id = status.repo.id.clone();
        if let Some(err) = &status.error {
            errors.insert(id.clone(), format!("{} · {err}", status.repo.label()));
            // Keep the previous change selection visible if reading Git fails.
            if let Some(previous) = next.remove(&id) {
                status.branch = previous.branch;
                status.upstream = previous.upstream;
                status.ahead = previous.ahead;
                status.behind = previous.behind;
                status.detached = previous.detached;
                status.files = previous.files;
            }
        }
        next.insert(id, status);
    }
    s.statuses = s
        .repos
        .iter()
        .filter_map(|repo| next.remove(&repo.id))
        .collect();
    s.workspace_refresh.changes_ready = true;
    update_errors(&mut s.workspace_refresh.change_errors, targets, errors);
    reconcile_change_selection(s);
}

fn reconcile_change_selection(s: &mut AppState) {
    let changes: HashMap<String, &ChangedFile> = s
        .statuses
        .iter()
        .flat_map(|status| status.files.iter())
        .map(|file| (file.key(), file))
        .collect();
    for key in changes.keys() {
        if s.known_change_keys.insert(key.clone()) {
            s.checked_changes.insert(key.clone());
        }
    }
    s.known_change_keys.retain(|key| changes.contains_key(key));
    s.checked_changes.retain(|key| changes.contains_key(key));
    s.included_hunks.retain(|key, _| changes.contains_key(key));
    if let Some(selection) = &mut s.change_selection {
        if let Some(file) = changes.get(&selection.file.key()) {
            selection.file.clone_from(file);
        } else {
            s.change_selection = None;
        }
    }
}

async fn refresh_stashes(
    generation: u64,
    targets: Arc<[RepoInfo]>,
    superseded: Arc<AtomicBool>,
    all_repos: Vec<RepoInfo>,
) {
    if !is_current(generation) {
        return;
    }
    let repos = targets.clone();
    let result =
        blocking(move || read_repo_data(&repos, &superseded, git::read_repository_stashes)).await;
    update_current(generation, |s| {
        let refresh = &mut s.workspace_refresh;
        match result {
            Ok(data) => {
                let mut grouped: HashMap<String, Vec<StashEntry>> = HashMap::new();
                let retained = std::mem::take(&mut s.stashes)
                    .into_iter()
                    .filter(|stash| !data.updated.contains(&stash.repo_id));
                for stash in data.values.into_iter().chain(retained) {
                    grouped
                        .entry(stash.repo_id.clone())
                        .or_default()
                        .push(stash);
                }
                s.stashes = all_repos
                    .iter()
                    .filter_map(|repo| grouped.remove(&repo.id))
                    .flatten()
                    .collect();
                refresh.stashes_ready = true;
                update_errors(&mut refresh.stash_errors, &targets, data.errors);
            }
            Err(err) => {
                refresh
                    .stash_errors
                    .insert(String::new(), format!("读取 Stash 失败 · {err}"));
            }
        }
    });
}

fn finish(generation: u64, start: Instant, discovered: bool) {
    let mut queued = None;
    if !update_current(generation, |s| {
        s.workspace_refresh.active = None;
        s.last_refresh_ms = start.elapsed().as_millis() as u64;
        if !s.command_busy {
            queued = s.workspace_refresh.queued.take();
        }
    }) {
        return;
    }
    if discovered {
        trace_ready("commit_ready");
    }
    if let Some(queued) = queued {
        refresh_workspace_async(queued.ids().cloned());
    }
    retry_file_navigation();
}

fn trace_ready(event: &str) {
    if std::env::var_os("NEMOROUS_TRACE_TIMING").is_none() {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let (repos, changes, diff_rev) = app_signal().with_untracked(|s| {
        (
            s.repos.len(),
            s.statuses
                .iter()
                .map(|status| status.files.len())
                .sum::<usize>(),
            s.working_diff.as_ref().map_or(0, |diff| diff.content_rev),
        )
    });
    eprintln!(
        "NEMOROUS_TIMING event={event} epoch_ms={now} repos={repos} changes={changes} diff_rev={diff_rev}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(id: &str) -> Scope {
        Scope::Repositories(HashSet::from([id.to_owned()]))
    }

    #[test]
    fn queued_watch_requests_keep_all_repositories_and_full_discovery() {
        let mut refresh = WorkspaceRefresh::default();
        refresh.queue(repo("one"));
        refresh.queue(repo("two"));
        assert_eq!(
            refresh.queued,
            Some(Scope::Repositories(HashSet::from(["one".into(), "two".into()])))
        );
        refresh.queue(Scope::Full);
        refresh.queue(repo("three"));
        assert_eq!(refresh.queued, Some(Scope::Full));
    }

    #[test]
    fn git_write_invalidates_in_flight_results_and_preserves_their_scope() {
        let mut state = AppState::for_config(crate::config::Config::default());
        let superseded = Arc::new(AtomicBool::new(false));
        state.workspace_refresh.active = Some(ActiveRefresh {
            scope: Scope::Full,
            superseded: superseded.clone(),
            files_pending: true,
        });
        state.workspace_refresh.queue(repo("one"));
        state.pause_workspace_refresh_for_git_write();
        assert!(superseded.load(Ordering::Relaxed));
        assert!(!state.is_refreshing());
        assert_eq!(state.refresh_gen, 1);
        assert_eq!(state.workspace_refresh.queued, Some(Scope::Full));
    }
}
