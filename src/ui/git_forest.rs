use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use nsh_base::input;
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::git::{CommitEntry, RepoInfo};
use crate::git_log_query::{self, Branch, Query};
use crate::state::{self, RootOwnerHolder, app_signal, current_palette};

const PAGE_SIZE: usize = 80;
const MAX_LIMIT: usize = 500;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Grouping {
    #[default]
    Graph,
    Date,
    Repository,
}

impl Grouping {
    fn label(self) -> &'static str {
        match self {
            Self::Graph => "提交图",
            Self::Date => "按日期",
            Self::Repository => "按仓库",
        }
    }

    pub(super) fn key(self, commit: &CommitEntry) -> Option<&str> {
        match self {
            Self::Graph => None,
            Self::Date => Some(commit.date.get(..10).unwrap_or(&commit.date)),
            Self::Repository => Some(&commit.repo_name),
        }
    }
}

#[derive(Clone, Default)]
struct Filters {
    repo_id: String,
    branch_name: String,
    query: Query,
}

#[derive(Clone, Default)]
pub(super) struct Results {
    pub commits: Arc<Vec<CommitEntry>>,
    pub grouping: Grouping,
    pub loading: bool,
    pub more: bool,
    pub scan_limited: bool,
    pub errors: Arc<Vec<String>>,
}

#[derive(Clone, Default)]
struct Forest {
    generation: u64,
    branch_generation: u64,
    workspace: String,
    filters: Filters,
    results: Results,
    branch_list: Arc<Vec<(RepoInfo, Vec<Branch>)>>,
    revealed: Option<CommitEntry>,
    loaded: bool,
}

fn forest() -> Signal<Forest> {
    global::<Forest>()
}

pub(super) fn rows() -> Results {
    forest().with(|s| {
        let mut result = s.results.clone();
        if !s.loaded && s.filters.query == Query::default() && s.filters.repo_id.is_empty() {
            result.commits = Arc::new(app_signal().with(|app| app.git_commits.clone()));
        }
        if let Some(revealed) = s.revealed.as_ref()
            && !result
                .commits
                .iter()
                .any(|entry| same_commit(entry, revealed))
        {
            Arc::make_mut(&mut result.commits).insert(0, revealed.clone());
        }
        result
    })
}

pub(super) fn same_commit(a: &CommitEntry, b: &CommitEntry) -> bool {
    a.repo_id == b.repo_id && a.hash == b.hash
}

pub(super) fn setup() {
    let source = computed(|| {
        app_signal().with(|s| {
            (
                s.show_git_panel,
                s.cfg.workspace_path.clone(),
                s.repos.clone(),
            )
        })
    });
    effect(move || {
        let (open, workspace, _) = source.get();
        let moved = forest().with_untracked(|s| s.workspace != workspace);
        if moved {
            let previous = forest().get_untracked();
            forest().set(Forest {
                workspace,
                generation: previous.generation.wrapping_add(1),
                branch_generation: previous.branch_generation.wrapping_add(1),
                ..Forest::default()
            });
        }
        if open {
            request(false);
        } else {
            forest().update(|s| {
                s.generation = s.generation.wrapping_add(1);
                s.results.loading = false;
            });
        }
    });
}

fn change(update: impl FnOnce(&mut Filters)) {
    forest().update(|s| {
        update(&mut s.filters);
        s.filters.query.limit = PAGE_SIZE;
        s.revealed = None;
    });
    request(true);
}

fn clear() {
    change(|filters| *filters = Filters::default());
}

fn refresh() {
    forest().update(|s| s.branch_list = Arc::default());
    request(false);
}

fn load_more() {
    forest().update(|s| {
        s.filters.query.limit = (s.filters.query.limit.max(PAGE_SIZE) + PAGE_SIZE).min(MAX_LIMIT);
    });
    request(false);
}

pub(super) fn reveal(commit: CommitEntry) {
    forest().update(|s| {
        s.workspace = app_signal().with_untracked(|app| app.cfg.workspace_path.clone());
        s.filters = Filters::default();
        s.revealed = Some(commit.clone());
    });
    app_signal().update(|s| {
        s.show_git_panel = true;
        s.file_history = None;
    });
    super::git_pane::select_commit(commit);
    request(false);
}

fn request(debounce: bool) {
    let (workspace, repos) =
        app_signal().with_untracked(|s| (s.cfg.workspace_path.clone(), s.repos.clone()));
    let Some(owner) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) else {
        return;
    };
    forest().update(|s| {
        s.generation = s.generation.wrapping_add(1);
        s.results.loading = !repos.is_empty();
        s.results.errors = Arc::default();
    });
    let (generation, filters) = forest().with_untracked(|s| (s.generation, s.filters.clone()));
    let install = move || {
        if !forest().with_untracked(|s| s.generation == generation) {
            return;
        }
        spawn(async move {
            let result = blocking(move || fetch(&repos, filters)).await;
            if !app_signal()
                .with_untracked(|s| s.show_git_panel && s.cfg.workspace_path == workspace)
            {
                return;
            }
            forest().update(|s| {
                if s.generation != generation {
                    return;
                }
                s.loaded = true;
                s.results = match result {
                    Ok(mut result) => {
                        result.grouping = s.results.grouping;
                        result
                    }
                    Err(err) => Results {
                        grouping: s.results.grouping,
                        errors: Arc::new(vec![err.to_string()]),
                        ..Results::default()
                    },
                };
            });
        });
    };
    owner.run(|| {
        if debounce {
            set_timeout(Duration::from_millis(150), install);
        } else {
            install();
        }
    });
}

fn fetch(repos: &[RepoInfo], mut filters: Filters) -> Results {
    filters.query.limit = filters.query.limit.clamp(PAGE_SIZE, MAX_LIMIT);
    let mut result = Results::default();
    let mut streams = Vec::new();
    let mut errors = Vec::new();
    for repo in repos
        .iter()
        .filter(|repo| filters.repo_id.is_empty() || filters.repo_id == repo.id)
    {
        match git_log_query::query(repo, &filters.query) {
            Ok(page) => {
                streams.push(VecDeque::from(page.commits));
                result.more |= page.more;
                result.scan_limited |= page.scan_limited;
            }
            Err(err) => errors.push(format!("{}：{err:#}", repo.name)),
        }
    }
    // Merge the per-repository topological sequences without reordering a parent
    // ahead of its child when commit clocks disagree.
    let mut entries = Vec::new();
    while let Some(index) = streams
        .iter()
        .enumerate()
        .filter_map(|(index, commits)| commits.front().map(|commit| (index, &commit.date)))
        .max_by(|(_, a), (_, b)| a.cmp(b))
        .map(|(index, _)| index)
    {
        entries.extend(streams[index].pop_front());
    }
    result.commits = Arc::new(entries);
    result.errors = Arc::new(errors);
    result.more &= filters.query.limit < MAX_LIMIT;
    result
}

fn label_button(
    id: &str,
    label: String,
    active: bool,
    action: impl Fn() + 'static,
) -> impl IntoElement {
    let p = current_palette();
    view()
        .test_id(id)
        .label(label.clone())
        .h(px(23.0))
        .px(px(7.0))
        .rounded(px(4.0))
        .bg(if active { p.accent_soft } else { p.surface })
        .border_1()
        .border_color(p.border)
        .cursor(Cursor::Pointer)
        .items_center()
        .justify_center()
        .on_press(action)
        .child(text(label).text_size(px(10.5)).text_color(if active {
            p.accent_soft_text
        } else {
            p.text_primary
        }))
}

fn field(
    id: &str,
    hint: &'static str,
    width: f32,
    value: impl Fn() -> String + 'static,
    update: impl Fn(&mut Query, String) + 'static,
) -> impl IntoElement {
    let p = current_palette();
    input()
        .test_id(id)
        .w(px(width))
        .h(px(23.0))
        .px(px(7.0))
        .rounded(px(4.0))
        .bg(p.surface)
        .border_1()
        .border_color(p.border)
        .text_size(px(10.5))
        .text_color(p.text_primary)
        .placeholder(hint)
        .value(value)
        .on_focus(|| state::set_text_input_focused(true))
        .on_blur(|| state::set_text_input_focused(false))
        .on_change(move |value: &str| change(|filters| update(&mut filters.query, value.into())))
}

pub(super) fn controls() -> impl IntoElement {
    view()
        .test_id("git-forest-controls")
        .flex_col()
        .gap(px(4.0))
        .px(px(8.0))
        .py(px(5.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(field(
                    "git-filter-input",
                    "提交信息 / 哈希 / 正则",
                    170.0,
                    || forest().with(|s| s.filters.query.text.clone()),
                    |q, v| q.text = v,
                ))
                .child(field(
                    "git-path-filter",
                    "路径 / 目录 / glob",
                    136.0,
                    || forest().with(|s| s.filters.query.path.clone()),
                    |q, v| q.path = v,
                ))
                .child(field(
                    "git-author-filter",
                    "作者 / 我",
                    105.0,
                    || forest().with(|s| s.filters.query.author.clone()),
                    |q, v| q.author = v,
                )),
        )
        .child(dynamic(|| {
            let (filters, grouping) = forest().with(|s| (s.filters.clone(), s.results.grouping));
            let repo = app_signal().with(|s| {
                s.repos
                    .iter()
                    .find(|repo| repo.id == filters.repo_id)
                    .map(|repo| repo.name.clone())
            });
            let branch = (!filters.branch_name.is_empty()).then_some(filters.branch_name);
            view()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(label_button(
                    "git-repo-filter",
                    repo.clone().unwrap_or_else(|| "全部仓库".into()),
                    repo.is_some(),
                    repo_menu,
                ))
                .child(label_button(
                    "git-branch-filter",
                    branch.clone().unwrap_or_else(|| "全部分支".into()),
                    branch.is_some(),
                    branch_menu,
                ))
                .child(label_button(
                    "git-period-filter",
                    period(filters.query.since_days),
                    filters.query.since_days > 0,
                    period_menu,
                ))
                .child(label_button(
                    "git-group-filter",
                    grouping.label().into(),
                    grouping != Grouping::Graph,
                    grouping_menu,
                ))
                .child(label_button("git-reload", "刷新".into(), false, refresh))
                .child(label_button(
                    "git-clear-filters",
                    "清除筛选".into(),
                    false,
                    clear,
                ))
        }))
}

fn repo_menu() {
    let current = forest().with_untracked(|s| s.filters.repo_id.clone());
    let repos = app_signal().with_untracked(|s| s.repos.clone());
    let mut items = vec![
        MenuItem::new("全部仓库")
            .checked(current.is_empty())
            .on_select(|| {
                change(|s| {
                    s.repo_id.clear();
                    s.branch_name.clear();
                    s.query.branch.clear();
                })
            }),
    ];
    items.extend(repos.into_iter().map(|repo| {
        MenuItem::new(repo.name)
            .checked(current == repo.id)
            .on_select(move || {
                change(|s| {
                    s.repo_id = repo.id.clone();
                    s.branch_name.clear();
                    s.query.branch.clear();
                });
            })
    }));
    state::show_popup_menu(items);
}

fn branch_menu() {
    let cached = forest().with_untracked(|s| s.branch_list.clone());
    if !cached.is_empty() {
        return show_branch_menu(&cached);
    }
    let Some(owner) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) else {
        return;
    };
    let (workspace, repos) =
        app_signal().with_untracked(|s| (s.cfg.workspace_path.clone(), s.repos.clone()));
    let generation = forest().with_untracked(|s| s.branch_generation.wrapping_add(1));
    forest().update(|s| s.branch_generation = generation);
    owner.run(|| {
        spawn(async move {
            let result = blocking(move || {
                repos
                    .into_iter()
                    .map(|repo| {
                        let branches = git_log_query::list_branches(&repo).unwrap_or_default();
                        (repo, branches)
                    })
                    .collect::<Vec<_>>()
            })
            .await;
            if !forest().with_untracked(|s| s.branch_generation == generation)
                || !app_signal()
                    .with_untracked(|s| s.show_git_panel && s.cfg.workspace_path == workspace)
            {
                return;
            }
            if let Ok(branches) = result {
                let branches = Arc::new(branches);
                forest().update(|s| s.branch_list = branches.clone());
                show_branch_menu(&branches);
            }
        })
    });
}

fn show_branch_menu(branches: &[(RepoInfo, Vec<Branch>)]) {
    let filter = forest().with_untracked(|s| s.filters.clone());
    let mut items = vec![
        MenuItem::new("全部分支")
            .checked(filter.branch_name.is_empty())
            .on_select(|| {
                change(|s| {
                    s.branch_name.clear();
                    s.query.branch.clear();
                })
            }),
    ];
    for (repo, entries) in branches
        .iter()
        .filter(|(repo, _)| filter.repo_id.is_empty() || filter.repo_id == repo.id)
    {
        let options: Vec<_> = entries
            .iter()
            .map(|branch| {
                let repo_id = repo.id.clone();
                let branch = branch.clone();
                MenuItem::new(branch.name.clone())
                    .checked(filter.repo_id == repo_id && filter.query.branch == branch.full_ref)
                    .on_select(move || {
                        change(|s| {
                            s.repo_id = repo_id.clone();
                            s.branch_name = branch.name.clone();
                            s.query.branch = branch.full_ref.clone();
                        })
                    })
            })
            .collect();
        if !options.is_empty() {
            items.push(MenuItem::submenu(repo.name.clone(), options));
        }
    }
    state::show_popup_menu(items);
}

fn period(days: u32) -> String {
    match days {
        0 => "全部时间".into(),
        365 => "近一年".into(),
        days => format!("近 {days} 天"),
    }
}

fn period_menu() {
    let current = forest().with_untracked(|s| s.filters.query.since_days);
    state::show_popup_menu(
        [0, 7, 30, 365]
            .into_iter()
            .map(|days| {
                MenuItem::new(period(days))
                    .checked(current == days)
                    .on_select(move || change(|s| s.query.since_days = days))
            })
            .collect(),
    );
}

fn grouping_menu() {
    let current = forest().with_untracked(|s| s.results.grouping);
    state::show_popup_menu(
        [Grouping::Graph, Grouping::Date, Grouping::Repository]
            .into_iter()
            .map(|grouping| {
                MenuItem::new(grouping.label())
                    .checked(current == grouping)
                    .on_select(move || {
                        forest().update(|s| s.results.grouping = grouping);
                    })
            })
            .collect(),
    );
}

pub(super) fn more_button() -> impl IntoElement {
    label_button("git-load-more", "加载更多".into(), false, load_more)
}
