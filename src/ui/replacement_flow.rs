use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use crate::git::WorkspaceFile;
use crate::local_history::LocalHistoryStore;
use crate::replace_in_files::{self, FileResult, Plan, ReplaceQuery};
use crate::search::SearchHit;
use crate::state::{RootOwnerHolder, app_signal};
use nsh_core::prelude::*;

// 状态机

/// 替换流程的对外阶段。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Planning,
    Confirm {
        files: usize,
        matches: usize,
    },
    Applying,
}

/// 前端发起的替换请求；`hit` 为 `Some` 表示只替换选中命中，`None` 表示全量替换。
#[derive(Clone)]
pub struct Request {
    pub workspace: String,
    pub files: Arc<Vec<WorkspaceFile>>,
    pub scope: Option<PathBuf>,
    pub query: ReplaceQuery,
    pub hit: Option<SearchHit>,
}

#[derive(Clone, Default)]
struct FlowState {
    phase: Phase,
    message: Option<String>,
    /// 发起本次操作时的工作区，用于拒绝跨工作区完成回调。
    workspace: String,
    /// 操作代次：启动、取消都会自增，旧任务据此丢弃结果。
    generation: u64,
    /// 仅全量替换在确认阶段保留计划。
    plan: Option<Plan>,
}

/// 预检结果：单选立即写盘，全量等待确认。
enum Prepared {
    Single(Plan),
    All(Plan),
}

fn read_state<R>(f: impl FnOnce(&FlowState) -> R) -> R {
    global::<FlowState>().with_untracked(f)
}

fn read_reactive<R>(f: impl FnOnce(&FlowState) -> R) -> R {
    global::<FlowState>().with(f)
}

fn write_state(f: impl FnOnce(&mut FlowState)) {
    global::<FlowState>().update(f);
}

fn is_generation(generation: u64) -> bool {
    read_state(|s| s.generation == generation)
}

fn current_workspace() -> String {
    app_signal().with_untracked(|s| s.cfg.workspace_path.clone())
}

fn workspace_is(workspace: &str) -> bool {
    app_signal().with_untracked(|s| s.cfg.workspace_path == workspace)
}

/// 从源码标签与工作区 diff 采集未保存路径，供替换引擎做冲突检查。
fn dirty_snapshot() -> HashSet<String> {
    app_signal().with_untracked(|s| {
        let mut dirty = HashSet::new();
        for tab in &s.code_tabs {
            if tab.is_dirty() {
                dirty.insert(tab.file.key());
            }
        }
        if let Some(diff) = &s.working_diff {
            if diff.is_dirty() {
                dirty.insert(diff.file.key());
            }
        }
        dirty
    })
}

/// 工作区已切换：只清理仍属于本次操作的阶段，绝不覆盖新操作。
fn spawn_from_root(task: impl std::future::Future<Output = ()> + 'static) {
    if let Some(owner) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) {
        owner.run(|| spawn(task));
    }
}

fn abort(generation: u64, workspace: &str) {
    write_state(|s| {
        if s.generation == generation && s.workspace == workspace {
            s.phase = Phase::Idle;
            s.plan = None;
            s.message = Some("工作区已切换，替换已取消".to_string());
        }
    });
}

// 公共 API

pub fn phase() -> Phase {
    read_reactive(|s| s.phase.clone())
}

pub fn message() -> Option<String> {
    read_reactive(|s| s.message.clone())
}

/// 启动替换：先做受检预检，再按单选/全量分派。
pub fn start(request: Request, on_complete: fn()) {
    if !workspace_is(&request.workspace) {
        write_state(|s| {
            if matches!(s.phase, Phase::Idle) {
                s.message = Some("工作区已切换，未开始替换".to_string());
            }
        });
        return;
    }

    let mut generation = 0u64;
    let mut accepted = false;
    write_state(|s| {
        // 正在规划、应用，或已有待确认计划：忽略重复入口。
        if !matches!(s.phase, Phase::Idle) || s.plan.is_some() {
            return;
        }
        s.generation = s.generation.wrapping_add(1);
        s.workspace = request.workspace.clone();
        s.phase = Phase::Planning;
        s.plan = None;
        s.message = None;
        generation = s.generation;
        accepted = true;
    });
    if !accepted {
        return;
    }

    let workspace = request.workspace.clone();
    let files = request.files.clone();
    let scope = request.scope.clone();
    let query = request.query.clone();
    let hit = request.hit.clone();
    let dirty = dirty_snapshot();

    spawn_from_root(async move {
        let prepared = blocking(move || -> anyhow::Result<Prepared> {
            match hit {
                Some(hit) => replace_in_files::prepare_one(
                    files.as_slice(),
                    scope.as_deref(),
                    query,
                    &hit,
                    &dirty,
                )
                .map(Prepared::Single),
                None => {
                    replace_in_files::prepare_all(files.as_slice(), scope.as_deref(), query, &dirty)
                        .map(Prepared::All)
                }
            }
        })
        .await;

        // 预检完成：工作区或代次已变化则丢弃结果，绝不写盘。
        if !workspace_is(&workspace) {
            abort(generation, &workspace);
            return;
        }
        if !is_generation(generation) {
            return;
        }

        match prepared {
            Ok(Err(err)) => {
                write_state(|s| {
                    if s.generation == generation && s.workspace == workspace {
                        s.phase = Phase::Idle;
                        s.plan = None;
                        s.message = Some(format!("替换预检失败：{err}"));
                    }
                });
            }
            Err(_) => fail(generation, "替换预检失败"),
            Ok(Ok(Prepared::All(plan))) if plan.match_count() == 0 => {
                fail(
                    generation,
                    "没有可替换的匹配（未保存或不可编辑的文件已跳过）",
                );
            }
            Ok(Ok(Prepared::All(plan))) => {
                let files = plan.file_count();
                let matches = plan.match_count();
                write_state(|s| {
                    if s.generation == generation && s.workspace == workspace {
                        s.plan = Some(plan);
                        s.phase = Phase::Confirm { files, matches };
                        s.message = Some(format!("确认替换 {files} 个文件，共 {matches} 处？"));
                    }
                });
            }
            Ok(Ok(Prepared::Single(plan))) => {
                // 单选：预检通过后立刻受检写盘。
                let mut proceed = false;
                write_state(|s| {
                    if s.generation == generation && s.workspace == workspace {
                        s.phase = Phase::Applying;
                        proceed = true;
                    }
                });
                if !proceed {
                    return;
                }
                if !workspace_is(&workspace) {
                    abort(generation, &workspace);
                    return;
                }
                // 写盘前重新取脏路径快照：预检与写入之间的改动必须被看到。
                apply_plan(generation, workspace, plan, on_complete).await;
            }
        }
    });
}

/// 执行仅有的全量计划；只接受 `Confirm` 阶段。
pub fn confirm(on_complete: fn()) {
    let workspace = current_workspace();
    let mut job: Option<(u64, Plan)> = None;
    write_state(|s| {
        if !matches!(s.phase, Phase::Confirm { .. }) || s.workspace != workspace {
            return;
        }
        let Some(plan) = s.plan.take() else {
            return;
        };
        s.phase = Phase::Applying;
        job = Some((s.generation, plan));
    });
    let Some((generation, plan)) = job else {
        return;
    };

    spawn_from_root(async move {
        if !is_generation(generation) {
            return;
        }
        if !workspace_is(&workspace) {
            abort(generation, &workspace);
            return;
        }
        // 每个写盘任务启动前都重新采集脏路径。
        apply_plan(generation, workspace, plan, on_complete).await;
    });
}

/// 取消排队/预检/待确认；进入 `Applying` 后不可取消。
pub fn cancel() -> bool {
    let mut allowed = false;
    write_state(|s| {
        if matches!(s.phase, Phase::Applying) {
            return;
        }
        allowed = true;
        s.generation = s.generation.wrapping_add(1);
        s.plan = None;
        if !matches!(s.phase, Phase::Idle) {
            s.phase = Phase::Idle;
            s.message = Some("已取消替换".to_string());
        }
    });
    allowed
}

// 完成回调

fn fail(generation: u64, message: &str) {
    write_state(|s| {
        if s.generation == generation {
            s.phase = Phase::Idle;
            s.plan = None;
            s.message = Some(message.to_string());
        }
    });
}

async fn apply_plan(generation: u64, workspace: String, plan: Plan, on_complete: fn()) {
    let dirty = dirty_snapshot();
    let results = blocking(move || {
        let history = LocalHistoryStore::default_store();
        replace_in_files::execute(&plan, &dirty, Some(&history))
    })
    .await;
    match results {
        Ok(results) => finish(generation, workspace, results, on_complete),
        Err(_) => {
            fail(generation, "写入中断，请检查文件内容");
            if workspace_is(&workspace) {
                on_complete();
            }
        }
    }
}

pub fn reset_message() {
    write_state(|s| s.message = None);
}

fn finish(generation: u64, workspace: String, results: Vec<FileResult>, on_complete: fn()) {
    let same_workspace = workspace_is(&workspace);
    let mut notify = false;
    write_state(|s| {
        if s.generation != generation || s.workspace != workspace {
            return;
        }
        s.phase = Phase::Idle;
        s.plan = None;
        if !same_workspace {
            s.message = Some(format!("工作区已切换；{}", report(&results)));
            return;
        }
        s.message = Some(report(&results));
        notify = true;
    });
    if notify {
        // 只有同工作区、同代次的完成才通知调用方重排搜索/刷新工作区。
        on_complete();
    }
}

fn report(results: &[FileResult]) -> String {
    let files: usize = results.iter().filter(|r| r.changed > 0).count();
    let matches: usize = results.iter().map(|r| r.changed).sum();
    let errors: Vec<&FileResult> = results.iter().filter(|r| r.error.is_some()).collect();

    let mut message = if matches == 0 {
        "未发生替换".to_string()
    } else {
        format!("已替换 {files} 个文件，共 {matches} 处")
    };

    if !errors.is_empty() {
        message.push_str(&format!("；{} 个文件失败", errors.len()));
        let details: Vec<String> = errors
            .iter()
            .take(3)
            .map(|r| {
                format!(
                    "{}: {}",
                    r.file.rel_path,
                    r.error.as_deref().unwrap_or("未知错误")
                )
            })
            .collect();
        message.push_str(&format!("（{}）", details.join("；")));
        if errors.len() > 3 {
            message.push('…');
        }
    }
    message
}
