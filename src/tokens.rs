//! What Herdr's own sidebar shows for organizations: per agent pane the
//! hierarchy tokens and `$org_task`; per coordinator workspace `$org_need`,
//! `$org_work` and `$org_review`. The "needs you" label for `blocked` goes
//! with the row's display name (`sidebar::report_pane`): Herdr replaces both
//! together, so a token report here carries neither.
//!
//! Computed from the state snapshot and sent over the socket only when a
//! value changed or half its TTL passed, so a quiet project costs nothing.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::herdr::{Herdr, PaneMetadata};
use crate::state::{Counts, Node, ProjectView, Status};

pub const TTL: Duration = Duration::from_secs(300);
pub const REFRESH: Duration = Duration::from_secs(150);
pub const NEEDS_YOU: &str = "needs you";
/// Every token `pane_tokens` writes, for clearing a pane.
pub const PANE_TOKENS: [&str; 5] = ["depth", "parent", "role", "tree-order", "org_task"];

/// One resource's wanted metadata.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Pane(String),
    Workspace(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wanted {
    pub target: Target,
    pub tokens: Tokens,
}

/// `$org_task`: what the row is about in a few words.
pub fn task(project: &ProjectView, node: &Node) -> String {
    if node.role == "coordinator" {
        let name = if node.id == crate::organizations::ROOT_ID {
            &project.name
        } else {
            &node.title
        };
        return format!("coord {name}");
    }
    let number = node
        .pr
        .as_ref()
        .map(|p| p.number.clone())
        .unwrap_or_default();
    let with = |word: &str| {
        if number.is_empty() {
            word.to_string()
        } else {
            format!("{word} #{number}")
        }
    };
    match node.status {
        Status::Review if node.group == "landing" => with("approved"),
        Status::Review => with("review"),
        Status::Done if node.pr.as_ref().is_some_and(|p| p.merged()) => with("merged"),
        _ => project
            .node(&node.coordinator)
            .map(|c| {
                if c.id == crate::organizations::ROOT_ID {
                    project.name.clone()
                } else {
                    c.title.clone()
                }
            })
            .unwrap_or_else(|| project.name.clone()),
    }
}

/// The organization tokens of a pane row. Project, group and rank come from
/// the sidebar grouping (`hp_project`, `hp_rank`, `hp_group`); the older
/// `project`, `thread`, `review` and `rank` names are cleared there.
fn pane_tokens(project: &ProjectView, node: &Node) -> Vec<(String, Option<String>)> {
    let root = node.id == crate::organizations::ROOT_ID;
    [
        ("depth", node.depth.to_string()),
        (
            "parent",
            if root {
                "none".into()
            } else {
                node.parent.clone()
            },
        ),
        ("role", node.role.clone()),
        ("tree-order", node.tree_order.to_string()),
        ("org_task", task(project, node)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Some(v)))
    .collect()
}

fn workspace_tokens(counts: &Counts) -> Vec<(String, Option<String>)> {
    [
        ("org_need", counts.need),
        ("org_work", counts.work),
        ("org_review", counts.review),
    ]
    .into_iter()
    .map(|(k, n)| (k.to_string(), (n > 0).then(|| format!("●{n}"))))
    .collect()
}

/// Everything one project wants on its local panes and workspaces. A
/// workspace shared by several coordinators shows the outermost one's counts.
pub fn wanted(project: &ProjectView) -> Vec<Wanted> {
    let mut out = Vec::new();
    for node in project.coordinators.iter().chain(&project.threads) {
        if node.pane_id.is_empty() || !node.machine.is_empty() || node.resolved {
            continue;
        }
        out.push(Wanted {
            target: Target::Pane(node.pane_id.clone()),
            tokens: pane_tokens(project, node),
        });
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut coordinators: Vec<&Node> = project.coordinators.iter().collect();
    coordinators.sort_by_key(|c| c.depth);
    for c in coordinators {
        if c.workspace_id.is_empty() || !c.machine.is_empty() || c.resolved {
            continue;
        }
        if seen.insert(c.workspace_id.clone()) {
            out.push(Wanted {
                target: Target::Workspace(c.workspace_id.clone()),
                tokens: workspace_tokens(&c.counts),
            });
        }
    }
    out
}

type Tokens = Vec<(String, Option<String>)>;

/// Last report per `(socket, target)`.
#[derive(Default)]
pub struct Cache {
    sent: BTreeMap<String, (Tokens, Instant)>,
}

impl Cache {
    fn key(socket: &str, target: &Target) -> String {
        match target {
            Target::Pane(id) => format!("{socket}\u{0}pane\u{0}{id}"),
            Target::Workspace(id) => format!("{socket}\u{0}ws\u{0}{id}"),
        }
    }

    /// Reports what changed or is due for refresh; returns how many reports
    /// were sent. A failed report is retried on the next pass.
    pub fn report(
        &mut self,
        herdr: &Herdr,
        socket: &str,
        wanted: &[Wanted],
        now: Instant,
    ) -> usize {
        let mut sent = 0;
        for want in wanted {
            let key = Self::key(socket, &want.target);
            let fresh = self.sent.get(&key).is_some_and(|(tokens, at)| {
                *tokens == want.tokens && now.duration_since(*at) < REFRESH
            });
            if fresh {
                continue;
            }
            let result = match &want.target {
                Target::Pane(pane) => herdr.pane_report_metadata_rpc(
                    pane,
                    &PaneMetadata {
                        tokens: want.tokens.clone(),
                        ttl: Some(TTL),
                    },
                ),
                Target::Workspace(ws) => {
                    herdr.workspace_report_metadata_rpc(ws, &want.tokens, Some(TTL))
                }
            };
            if result.is_ok() {
                self.sent.insert(key, (want.tokens.clone(), now));
                sent += 1;
            }
        }
        sent
    }

    /// Forgets resources that are no longer wanted, so a pane id reused after
    /// a server restart is reported again.
    pub fn retain(&mut self, socket: &str, wanted: &[Wanted]) {
        let keep: std::collections::BTreeSet<String> = wanted
            .iter()
            .map(|w| Self::key(socket, &w.target))
            .collect();
        let prefix = format!("{socket}\u{0}");
        self.sent
            .retain(|k, _| !k.starts_with(&prefix) || keep.contains(k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use crate::state::Pr;

    fn project() -> ProjectView {
        let root = Node {
            id: "root".into(),
            role: "coordinator".into(),
            coordinator: "root".into(),
            workspace_id: "w1".into(),
            pane_id: "w1:p1".into(),
            counts: Counts {
                need: 1,
                work: 2,
                review: 1,
                ..Counts::default()
            },
            ..Node::default()
        };
        let mobile = Node {
            id: "t-0001".into(),
            title: "mobile redesign".into(),
            role: "coordinator".into(),
            coordinator: "root".into(),
            parent: "root".into(),
            depth: 1,
            workspace_id: "w2".into(),
            pane_id: "w2:p1".into(),
            counts: Counts {
                need: 1,
                ..Counts::default()
            },
            ..Node::default()
        };
        let waiting = Node {
            id: "t-0002".into(),
            title: "billing ui".into(),
            role: "worker".into(),
            coordinator: "t-0001".into(),
            parent: "t-0001".into(),
            depth: 2,
            status: Status::Need,
            group: "waiting-on-you".into(),
            workspace_id: "w2".into(),
            pane_id: "w2:p2".into(),
            ..Node::default()
        };
        let review = Node {
            id: "t-0003".into(),
            role: "worker".into(),
            coordinator: "t-0001".into(),
            status: Status::Review,
            group: "ready-for-review".into(),
            pane_id: "w2:p3".into(),
            pr: Some(Pr {
                number: "1342".into(),
                ..Pr::default()
            }),
            ..Node::default()
        };
        ProjectView {
            slug: "acme".into(),
            name: "Acme Billing Suite".into(),
            coordinators: vec![root, mobile],
            threads: vec![waiting, review],
            ..ProjectView::default()
        }
    }

    fn token<'a>(w: &'a Wanted, name: &str) -> Option<&'a str> {
        w.tokens
            .iter()
            .find(|(k, _)| k == name)
            .and_then(|(_, v)| v.as_deref())
    }

    #[test]
    fn rows_carry_task_tokens_and_coordinator_workspaces_carry_counts() {
        let p = project();
        let wanted = wanted(&p);
        let pane = |id: &str| {
            wanted
                .iter()
                .find(|w| w.target == Target::Pane(id.into()))
                .unwrap()
        };
        assert_eq!(
            token(pane("w1:p1"), "org_task"),
            Some("coord Acme Billing Suite")
        );
        assert_eq!(
            token(pane("w2:p1"), "org_task"),
            Some("coord mobile redesign")
        );
        assert_eq!(token(pane("w2:p2"), "org_task"), Some("mobile redesign"));
        assert_eq!(token(pane("w2:p3"), "org_task"), Some("review #1342"));
        let ws = |id: &str| {
            wanted
                .iter()
                .find(|w| w.target == Target::Workspace(id.into()))
                .unwrap()
        };
        assert_eq!(token(ws("w1"), "org_need"), Some("●1"));
        assert_eq!(token(ws("w1"), "org_work"), Some("●2"));
        assert_eq!(token(ws("w2"), "org_review"), None, "zero clears the token");
        assert!(
            ws("w2")
                .tokens
                .iter()
                .any(|(k, v)| k == "org_review" && v.is_none())
        );
    }

    #[test]
    fn unchanged_metadata_is_not_resent_until_half_the_ttl() {
        let runner = FakeRunner::new();
        let herdr = Herdr::new("herdr", "s.sock", &runner);
        let p = project();
        let wanted = wanted(&p);
        let mut cache = Cache::default();
        let t0 = Instant::now();
        assert_eq!(cache.report(&herdr, "s.sock", &wanted, t0), wanted.len());
        assert_eq!(
            cache.report(&herdr, "s.sock", &wanted, t0 + Duration::from_secs(60)),
            0
        );
        assert_eq!(
            cache.report(&herdr, "s.sock", &wanted, t0 + REFRESH),
            wanted.len()
        );
        let requests = runner.socket_requests.borrow();
        assert!(requests[0].1.contains("pane.report_metadata"));
        assert!(requests[0].1.contains("\"ttl_ms\":300000"));
        assert_eq!(runner.calls.borrow().len(), 0, "no CLI fork");
    }

    #[test]
    fn token_reports_leave_the_display_name_and_state_labels_alone() {
        // Herdr replaces a source's display name and state labels together
        // when a report carries either; `sidebar::report_pane` owns both.
        let runner = FakeRunner::new();
        let herdr = Herdr::new("herdr", "s.sock", &runner);
        Cache::default().report(&herdr, "s.sock", &wanted(&project()), Instant::now());
        let requests = runner.socket_requests.borrow();
        assert!(!requests.is_empty());
        assert!(requests.iter().all(|(_, line)| {
            !line.contains("state_labels") && !line.contains("display_agent")
        }));
    }

    #[test]
    fn a_changed_value_is_sent_at_once() {
        let runner = FakeRunner::new();
        let herdr = Herdr::new("herdr", "s.sock", &runner);
        let mut p = project();
        let mut cache = Cache::default();
        let t0 = Instant::now();
        cache.report(&herdr, "s.sock", &wanted(&p), t0);
        // `$org_task` follows the status: a thread ready for review says so.
        p.threads[0].status = Status::Review;
        p.threads[0].group = "ready-for-review".into();
        assert_eq!(cache.report(&herdr, "s.sock", &wanted(&p), t0), 1);
    }
}
