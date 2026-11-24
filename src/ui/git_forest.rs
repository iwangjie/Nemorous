use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use nsh_base::input;
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::git::{CommitEntry, RepoInfo};
use crate::git_log_query::{self, Branch, Query};
use crate::state::{self, RootOwnerHolder, app_signal, current_palette};
use super::icons::{self, *};

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

    pub(super) fn key<'a>(self, commit: &'a CommitEntry, local_date: &'a str) -> Option<&'a str> {
        match self {
            Self::Graph => None,
            Self::Date => Some(local_date.get(..10).unwrap_or(local_date)),
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
    branch_revision: u64,
    workspace: String,
    filters: Filters,
    results: Results,
    branch_list: Arc<Vec<(RepoInfo, Vec<Branch>)>>,
    revealed: Option<CommitEntry>,
}

fn forest() -> Signal<Forest> {
    global::<Forest>()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Request {
    Immediate,
    FilterEdit,
    RepositoryRefresh,
}

pub(super) fn rows() -> Results {
    forest().with(|s| {
        let mut result = s.results.clone();
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
                s.show_git_panel && s.file_history.is_none(),
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
            invalidate_branches();
            request(Request::Immediate);
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
    request(Request::FilterEdit);
}

fn clear() {
    change(|filters| {
        *filters = Filters {
            query: Query {
                literal_text: filters.query.literal_text,
                case_sensitive: filters.query.case_sensitive,
                ..Query::default()
            },
            ..Filters::default()
        };
    });
}

fn refresh() {
    invalidate_branches();
    request(Request::Immediate);
}

fn invalidate_branches() {
    forest().update(|s| {
        s.branch_list = Arc::default();
        s.branch_revision = s.branch_revision.wrapping_add(1);
    });
}

pub(crate) fn repository_history_changed(repo_ids: &HashSet<String>) {
    invalidate_branches();
    let open_log = app_signal().with_untracked(|s| s.show_git_panel && s.file_history.is_none());
    if open_log
        && forest().with_untracked(|s| {
            s.filters.repo_id.is_empty() || repo_ids.contains(&s.filters.repo_id)
        })
    {
        request(Request::RepositoryRefresh);
    }
}

fn load_more() {
    forest().update(|s| {
        s.filters.query.limit = (s.filters.query.limit.max(PAGE_SIZE) + PAGE_SIZE).min(MAX_LIMIT);
    });
    request(Request::Immediate);
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
    request(Request::Immediate);
}

fn request(reason: Request) {
    let background = reason == Request::RepositoryRefresh;
    let (workspace, repos) =
        app_signal().with_untracked(|s| (s.cfg.workspace_path.clone(), s.repos.clone()));
    let Some(owner) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) else {
        return;
    };
    forest().update(|s| {
        s.generation = s.generation.wrapping_add(1);
        s.results.loading = !repos.is_empty() && (!background || s.results.commits.is_empty());
        s.results.errors = Arc::default();
    });
    let (generation, filters) = forest().with_untracked(|s| (s.generation, s.filters.clone()));
    let install = move || {
        if !forest().with_untracked(|s| s.generation == generation) {
            return;
        }
        spawn(async move {
            let result = blocking(move || fetch(&repos, filters)).await;
            if !app_signal().with_untracked(|s| {
                s.show_git_panel && s.file_history.is_none() && s.cfg.workspace_path == workspace
            }) || !forest().with_untracked(|s| s.generation == generation)
            {
                return;
            }
            let mut page = match result {
                Ok(page) => page,
                Err(err) => Results {
                    errors: Arc::new(vec![err.to_string()]),
                    ..Results::default()
                },
            };
            if background
                && !page.errors.is_empty()
                && forest().with_untracked(|s| !s.results.commits.is_empty())
            {
                forest().update(|s| {
                    s.results.loading = false;
                    s.results.errors = page.errors;
                });
                return;
            }
            let first = page.commits.first().cloned();
            let commits = page.commits.clone();
            forest().update(|s| {
                page.grouping = s.results.grouping;
                if let Some(revealed) = &mut s.revealed
                    && let Some(entry) = page
                        .commits
                        .iter()
                        .find(|entry| same_commit(entry, revealed))
                {
                    *revealed = entry.clone();
                }
                s.results = page;
            });
            let selected = app_signal().with_untracked(|s| s.selected_commit.clone());
            if let Some(selected) = &selected {
                if let Some(entry) = commits.iter().find(|entry| same_commit(entry, selected)) {
                    if entry != selected {
                        app_signal().update(|s| s.selected_commit = Some(entry.clone()));
                    }
                    return;
                }
                if forest().with_untracked(|s| {
                    s.revealed
                        .as_ref()
                        .is_some_and(|entry| same_commit(entry, selected))
                }) {
                    return;
                }
            }
            if let Some(commit) = first {
                super::git_pane::load_commit_selection(commit, !background);
            } else if selected.is_some() {
                super::git_pane::clear_commit_selection();
            }
        });
    };
    owner.run(|| {
        if reason == Request::FilterEdit {
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
                let commits = page.commits.into_iter().map(|commit| {
                    let timestamp = chrono::DateTime::parse_from_rfc3339(&commit.date)
                        .ok()
                        .map(|date| date.timestamp());
                    (timestamp, commit)
                });
                streams.push(commits.peekable());
                result.more |= page.more;
                result.scan_limited |= page.scan_limited;
            }
            Err(err) => errors.push(format!("{}：{err:#}", repo.label())),
        }
    }
    // Compare author instants across repositories while preserving each Git
    // sequence, where a parent must remain after its child.
    let mut entries = Vec::new();
    while let Some(index) = streams
        .iter_mut()
        .enumerate()
        .filter_map(|(index, commits)| commits.peek().map(|(timestamp, _)| (index, *timestamp)))
        .max_by_key(|(_, timestamp)| *timestamp)
        .map(|(index, _)| index)
    {
        entries.extend(streams[index].next().map(|(_, commit)| commit));
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
        .max_w(px(200.0))
        .px(px(7.0))
        .rounded(px(4.0))
        .bg(if active { p.accent_soft } else { p.surface })
        .border_1()
        .border_color(p.border)
        .cursor(Cursor::Pointer)
        .items_center()
        .justify_center()
        .on_press(action)
        .child(
            text(label)
                .text_size(px(10.5))
                .text_color(if active { p.accent_soft_text } else { p.text_primary })
                .number_of_lines(1u32),
        )
}

fn filter_button(
    id: &'static str,
    label: &'static str,
    value: impl Fn() -> Option<String> + Copy + 'static,
    action: impl Fn() + 'static,
) -> nsh_core::element::View {
    let p = current_palette();
    view()
        .test_id(id)
        .label(label)
        .h(px(23.0))
        .max_w(px(128.0))
        .px(px(5.0))
        .gap(px(3.0))
        .flex_row()
        .items_center()
        .rounded(px(4.0))
        .bg(move || if value().is_some() { p.accent_soft } else { transparent_black() })
        .cursor(Cursor::Pointer)
        .hover(move |style| style.bg(p.surface_hover))
        .on_press(action)
        .child(
            text(move || value().unwrap_or_else(|| label.to_owned()))
                .text_size(px(11.0))
                .text_color(p.text_control)
                .number_of_lines(1u32),
        )
        .child(icons::icon(ICO_CHEVRON_DOWN, 9.0, p.text_muted))
}

pub(super) fn icon_action(
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    active: bool,
    enabled: bool,
    action: impl Fn() + 'static,
) -> impl IntoElement {
    let p = current_palette();
    let color = if !enabled { p.text_muted.opacity(0.5) } else { p.text_control };
    view()
        .test_id(id)
        .label(label)
        .tooltip(super::tooltip_label(label.into(), p))
        .w(px(25.0))
        .h(px(23.0))
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(if active { p.accent_soft } else { transparent_black() })
        .cursor(if enabled { Cursor::Pointer } else { Cursor::Default })
        .hover(move |style| if enabled { style.bg(p.surface_hover) } else { style })
        .on_press(move || {
            if enabled {
                action();
            }
        })
        .child(icons::icon(icon, 13.0, color))
}

fn match_mode(
    id: &'static str,
    symbol: &'static str,
    label: &'static str,
    active: impl Fn(&Query) -> bool + 'static,
    toggle: impl Fn(&mut Query) + 'static,
) -> impl IntoElement {
    let p = current_palette();
    view()
        .test_id(id)
        .label(label)
        .tooltip(super::tooltip_label(label.into(), p))
        .w(px(19.0))
        .h(px(18.0))
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .bg(move || forest().with(|s| {
            if active(&s.filters.query) { p.accent_soft } else { transparent_black() }
        }))
        .cursor(Cursor::Pointer)
        .hover(move |style| style.bg(p.surface_hover))
        .on_press(move || change(|s| toggle(&mut s.query)))
        .child(text(symbol).text_size(px(10.5)).text_color(p.text_control))
}

fn text_filter(
    id: &'static str,
    input_id: &'static str,
    label: &'static str,
    hint: &'static str,
    read: fn(&Query) -> &String,
    write: fn(&mut Query, String),
) -> impl IntoElement {
    let p = current_palette();
    let open = signal(false);
    let anchor = element_ref();
    let draft = signal(String::new());
    let value = move || forest().with(|s| {
        let value = read(&s.filters.query);
        (!value.is_empty()).then(|| value.clone())
    });
    let close = move || {
        open.set(false);
        state::set_text_input_focused(false);
    };
    let apply = move |entered: &str| {
        change(|s| write(&mut s.query, entered.trim().to_owned()));
        close();
    };
    view()
        .child(
            filter_button(id, label, value, move || {
                if open.get_untracked() {
                    close();
                } else {
                    draft.set(value().unwrap_or_default());
                    open.set(true);
                }
            })
            .element_ref(&anchor),
        )
        .child(when(move || open.get(), move || {
            overlay()
                .anchor(&anchor)
                .align(Align::Start)
                .offset(px(4.0))
                .margin(px(8.0))
                .on_cancel(close)
                .on_press_outside(move |_: &Point<Points>| close())
                .child(
                    view()
                        .w(px(245.0))
                        .p(px(6.0))
                        .gap(px(4.0))
                        .flex_row()
                        .items_center()
                        .bg(p.surface)
                        .border_1()
                        .border_color(p.border_strong)
                        .rounded(px(5.0))
                        .child(
                            input()
                                .test_id(input_id)
                                .label(label)
                                .flex_1()
                                .h(px(24.0))
                                .px(px(6.0))
                                .rounded(px(3.0))
                                .bg(p.surface2)
                                .text_color(p.text_primary)
                                .text_size(px(11.0))
                                .placeholder(hint)
                                .auto_focus(true)
                                .value(move || draft.get())
                                .on_change(move |v: &str| draft.set(v.to_owned()))
                                .on_focus(|| state::set_text_input_focused(true))
                                .on_blur(|| state::set_text_input_focused(false))
                                .on_submit(apply),
                        )
                        .child(icon_action("git-apply-filter", "应用筛选", ICO_CHECK, false, true,
                            move || apply(&draft.get_untracked()))),
                )
        }))
}

pub(super) fn controls() -> impl IntoElement {
    let p = current_palette();
    view()
        .test_id("git-forest-controls")
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(2.0))
        .px(px(5.0))
        .py(px(3.0))
        .border_b_1()
        .border_color(p.border)
        .child(
            view()
                .w(px(176.0))
                .h(px(23.0))
                .px(px(4.0))
                .gap(px(2.0))
                .flex_row()
                .items_center()
                .rounded(px(4.0))
                .bg(p.surface2)
                .border_1()
                .border_color(p.border)
                .child(icons::icon(ICO_SEARCH, 12.0, p.text_muted))
                .child(
                    input()
                        .test_id("git-filter-input")
                        .label("搜索提交信息或哈希")
                        .flex_1()
                        .min_w(px(0.0))
                        .h_full()
                        .text_size(px(11.0))
                        .text_color(p.text_primary)
                        .placeholder("文本或哈希")
                        .value(|| forest().with(|s| s.filters.query.text.clone()))
                        .on_focus(|| state::set_text_input_focused(true))
                        .on_blur(|| state::set_text_input_focused(false))
                        .on_change(|v: &str| change(|s| s.query.text = v.to_owned())),
                )
                .child(match_mode("git-search-regex", ".*", "正则表达式", |q| !q.literal_text,
                    |q| q.literal_text = !q.literal_text))
                .child(match_mode("git-search-case", "Cc", "区分大小写", |q| q.case_sensitive,
                    |q| q.case_sensitive = !q.case_sensitive)),
        )
        .child(filter_button("git-branch-filter", "分支", || forest().with(|s| {
            (!s.filters.branch_name.is_empty()).then(|| s.filters.branch_name.clone())
        }), branch_menu))
        .child(text_filter(
            "git-author-filter-button", "git-author-filter", "作者", "作者姓名、邮箱或我",
            |q| &q.author,
            |q, value| q.author = value,
        ))
        .child(filter_button("git-period-filter", "日期", || forest().with(|s| {
            (s.filters.query.since_days > 0).then(|| period(s.filters.query.since_days))
        }), period_menu))
        .child(text_filter(
            "git-path-filter-button", "git-path-filter", "路径", "仓库相对路径或 glob",
            |q| &q.path,
            |q, value| q.path = value,
        ))
        .child(when(|| app_signal().with(|s| s.repos.len() > 1), || {
            filter_button("git-repo-filter", "仓库", || {
                let id = forest().with(|s| s.filters.repo_id.clone());
                app_signal().with(|s| s.repos.iter().find(|repo| repo.id == id).map(|repo| repo.label().to_owned()))
            }, repo_menu)
        }))
        .child(dynamic(|| {
            let active = forest().with(|s| s.results.grouping != Grouping::Graph);
            icon_action("git-group-filter", "提交图选项", ICO_BRANCH, active, true, grouping_menu)
        }))
        .child(view().flex_1())
        .child(dynamic(|| {
            let active = forest().with(|s| {
                let q = &s.filters.query;
                !s.filters.repo_id.is_empty() || !q.branch.is_empty() || !q.text.is_empty()
                    || !q.author.is_empty() || !q.path.is_empty() || q.since_days > 0
            });
            icon_action("git-clear-filters", "清除筛选", ICO_CLOSE, false, active, clear)
        }))
        .child(icon_action("git-reload", "刷新 Git 记录", ICO_REFRESH, false, true, refresh))
        .child(icon_action("git-log-view-options", "提交列表视图选项", ICO_EYE, false, true, super::git_pane::show_log_options))
        .child(icon_action("git-log-actions", "更多 Git 操作", ICO_MORE, false, true, || {
            state::show_popup_menu(vec![
                MenuItem::new("GitHub PR / CI").on_select(open_github),
            ]);
        }))
}

fn open_github() {
    let filter = forest().with_untracked(|s| s.filters.repo_id.clone());
    let selected = app_signal().with_untracked(|s| {
        s.selected_commit.as_ref().map(|commit| commit.repo_id.clone())
    });
    let repo_id = if filter.is_empty() {
        selected.as_deref()
    } else {
        Some(filter.as_str())
    };
    super::github_panel::open_for_repository(repo_id);
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
        MenuItem::new(repo.label().to_owned())
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
    let (generation, revision) =
        forest().with_untracked(|s| (s.branch_generation.wrapping_add(1), s.branch_revision));
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
                || !app_signal().with_untracked(|s| {
                    s.show_git_panel
                        && s.file_history.is_none()
                        && s.cfg.workspace_path == workspace
                })
            {
                return;
            }
            if forest().with_untracked(|s| s.branch_revision != revision) {
                branch_menu();
            } else if let Ok(branches) = result {
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
            items.push(MenuItem::submenu(repo.label().to_owned(), options));
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
