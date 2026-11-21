use std::collections::HashSet;

use nsh_core::prelude::*;

use crate::git::CommitEntry;
use crate::theme::Palette;

const MAX_SHOWN_LANES: usize = 9;
const LANE_WIDTH: f32 = 10.0;
const ROW_HEIGHT: f32 = 26.0;

#[derive(Clone, PartialEq, Eq)]
struct Pending<'a> {
    repo: &'a str,
    hash: &'a str,
    color: usize,
}

struct Row {
    active: usize,
    before: Vec<Option<usize>>,
    after: Vec<Option<usize>>,
    routes: Vec<usize>,
    color: usize,
}

pub(super) struct Graph {
    rows: Vec<Row>,
    width: usize,
}

fn allocate(lanes: &mut Vec<Option<Pending<'_>>>) -> usize {
    if let Some(index) = lanes.iter().position(Option::is_none) {
        return index;
    }
    let index = lanes.len();
    lanes.push(None);
    index
}

pub(super) fn layout(commits: &[CommitEntry]) -> Graph {
    let visible: HashSet<_> = commits
        .iter()
        .map(|commit| (commit.repo_id.as_str(), commit.hash.as_str()))
        .collect();
    let mut pending: Vec<Option<Pending<'_>>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    let mut width = 1;
    for commit in commits {
        let repo = commit.repo_id.as_str();
        let active = pending
            .iter()
            .position(|lane| {
                lane.as_ref()
                    .is_some_and(|lane| lane.repo == repo && lane.hash == commit.hash)
            })
            .unwrap_or_else(|| allocate(&mut pending));
        let color = pending[active].as_ref().map_or(active, |lane| lane.color);
        let before = pending
            .iter()
            .take(MAX_SHOWN_LANES)
            .map(|lane| lane.as_ref().map(|lane| lane.color))
            .collect();
        pending[active] = None;

        let mut routes = Vec::new();
        for parent in commit
            .parents
            .iter()
            .filter(|hash| visible.contains(&(repo, hash.as_str())))
        {
            let target = pending
                .iter()
                .position(|lane| {
                    lane.as_ref()
                        .is_some_and(|lane| lane.repo == repo && lane.hash == parent)
                })
                .unwrap_or_else(|| {
                    let index = if routes.is_empty() {
                        active
                    } else {
                        allocate(&mut pending)
                    };
                    pending[index] = Some(Pending {
                        repo,
                        hash: parent,
                        color: if routes.is_empty() { color } else { index },
                    });
                    index
                });
            routes.push(target);
        }
        while pending.last().is_some_and(Option::is_none) {
            pending.pop();
        }
        width = width.max((active + 1).max(pending.len()).min(MAX_SHOWN_LANES));
        let after = pending
            .iter()
            .take(MAX_SHOWN_LANES)
            .map(|lane| lane.as_ref().map(|lane| lane.color))
            .collect();
        rows.push(Row {
            active,
            before,
            after,
            routes,
            color,
        });
    }
    Graph { rows, width }
}

impl Graph {
    pub(super) fn row(&self, index: usize, p: Palette) -> AnyElement {
        let row = &self.rows[index];
        let x = |index: usize| px(index as f32 * LANE_WIDTH + 5.0);
        let color = |index: usize| [p.accent, p.green, p.amber, p.text_muted][index % 4];
        let mut strokes = Vec::new();
        for (start, colors) in [(0.0, &row.before), (ROW_HEIGHT / 2.0, &row.after)] {
            for (lane, paint) in colors.iter().enumerate() {
                if let Some(paint) = paint {
                    strokes.push(
                        view()
                            .absolute()
                            .left(x(lane))
                            .top(px(start))
                            .w(px(1.0))
                            .h(px(ROW_HEIGHT / 2.0))
                            .bg(color(*paint))
                            .into_element(),
                    );
                }
            }
        }
        if row.active < MAX_SHOWN_LANES {
            for &lane in &row.routes {
                if lane < MAX_SHOWN_LANES && lane != row.active {
                    strokes.push(
                        view()
                            .absolute()
                            .left(x(lane.min(row.active)))
                            .top(px(ROW_HEIGHT / 2.0))
                            .w(px(lane.abs_diff(row.active) as f32 * LANE_WIDTH + 1.0))
                            .h(px(1.0))
                            .bg(color(row.color))
                            .into_element(),
                    );
                }
            }
            strokes.push(
                view()
                    .absolute()
                    .left(x(row.active) - px(2.0))
                    .top(px(ROW_HEIGHT / 2.0 - 2.0))
                    .w(px(5.0))
                    .h(px(5.0))
                    .rounded(px(2.5))
                    .bg(color(row.color))
                    .into_element(),
            );
        }
        view()
            .relative()
            .h(px(ROW_HEIGHT))
            .w(px(self.width as f32 * LANE_WIDTH))
            .flex_none()
            .children(strokes)
            .into_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(repo: &str, hash: &str, parents: &[&str]) -> CommitEntry {
        CommitEntry {
            repo_id: repo.into(),
            hash: hash.into(),
            parents: parents.iter().map(|parent| (*parent).into()).collect(),
            repo_name: repo.into(),
            repo_path: String::new(),
            short_hash: hash.into(),
            author: String::new(),
            date: String::new(),
            subject: String::new(),
            refs: String::new(),
        }
    }

    #[test]
    fn connects_merge_parents_without_joining_repositories() {
        let graph = layout(&[
            commit("a", "top", &["p", "q"]),
            commit("b", "p", &[]),
            commit("a", "p", &["q"]),
            commit("a", "q", &[]),
        ]);
        assert_eq!(graph.rows[0].routes, vec![0, 1]);
        assert_eq!(graph.rows[1].active, 2);
        assert_eq!(graph.rows[2].active, 0);
        assert_eq!(graph.rows[2].routes, vec![1]);
        assert_eq!(graph.rows[3].active, 1);
    }
}
