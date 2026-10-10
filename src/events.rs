//! The ticker's one subscription per Herdr server. Events only wake the
//! ticker; it then re-reads state, so a missed event costs one reconcile.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::herdr::EventStream;

/// What the listener tells the ticker.
#[derive(Debug, Clone, PartialEq)]
pub enum Wake {
    /// Subscribed (again).
    Up(PathBuf),
    /// The stream failed; the ticker falls back to its reconcile interval.
    Down(PathBuf),
    Event(String),
}

/// Lifecycle events that can change a group or the tree. Metadata updates are
/// left out on purpose: the ticker's own token reports emit them.
const GLOBAL: [&str; 10] = [
    "workspace.created",
    "workspace.closed",
    "workspace.renamed",
    "tab.created",
    "tab.closed",
    "tab.renamed",
    "pane.created",
    "pane.closed",
    "pane.exited",
    "pane.agent_detected",
];

/// Events after which the per-pane status entries must be rebuilt.
fn changes_panes(event: &str) -> bool {
    matches!(
        event,
        "pane_created" | "pane_closed" | "pane_exited" | "pane_agent_detected"
    )
}

pub fn subscriptions(agent_panes: &[String]) -> Vec<serde_json::Value> {
    GLOBAL
        .iter()
        .map(|t| serde_json::json!({ "type": t }))
        .chain(agent_panes.iter().map(
            |pane| serde_json::json!({ "type": "pane.agent_status_changed", "pane_id": pane }),
        ))
        .collect()
}

const BACKOFF: [u64; 4] = [1, 2, 5, 15];

/// Runs until the process exits. `agent_panes` lists the panes to watch for
/// status changes; it is asked again on every (re)subscribe because one
/// closed pane makes Herdr reject the whole subscription.
pub fn listen(socket: PathBuf, agent_panes: impl Fn() -> Option<Vec<String>>, tx: Sender<Wake>) {
    let mut failures = 0usize;
    loop {
        let opened = agent_panes()
            .ok_or_else(|| anyhow::anyhow!("agent list failed"))
            .and_then(|panes| EventStream::open(&socket, &subscriptions(&panes)));
        let mut stream = match opened {
            Ok(stream) => stream,
            Err(_) => {
                if tx.send(Wake::Down(socket.clone())).is_err() {
                    return;
                }
                let secs = BACKOFF[failures.min(BACKOFF.len() - 1)];
                failures += 1;
                std::thread::sleep(Duration::from_secs(secs));
                continue;
            }
        };
        failures = 0;
        if tx.send(Wake::Up(socket.clone())).is_err() {
            return;
        }
        loop {
            match stream.next_event() {
                Ok(event) => {
                    let rebuild = changes_panes(&event);
                    if tx.send(Wake::Event(event)).is_err() {
                        return;
                    }
                    if rebuild {
                        break;
                    }
                }
                Err(_) => {
                    if tx.send(Wake::Down(socket.clone())).is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(BACKOFF[0]));
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_entries_follow_the_agent_panes() {
        let subs = subscriptions(&["w1:p1".into(), "w2:p3".into()]);
        assert_eq!(subs.len(), GLOBAL.len() + 2);
        assert_eq!(
            subs.last().unwrap(),
            &serde_json::json!({ "type": "pane.agent_status_changed", "pane_id": "w2:p3" })
        );
        assert!(
            !subs
                .iter()
                .any(|s| s["type"] == "workspace.metadata_updated")
        );
    }

    #[test]
    fn pane_lifecycle_events_rebuild_the_subscription() {
        assert!(changes_panes("pane_closed"));
        assert!(changes_panes("pane_agent_detected"));
        assert!(!changes_panes("pane_agent_status_changed"));
    }
}
