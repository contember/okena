//! Host sections for the manual sidebar layout.
//!
//! `project_order` is one merged list with a contiguous block per connection
//! (see `okena_workspace::remote_apply`). The *Separate* remote layout regroups
//! that list into `LOCAL`, then a `REMOTE` heading, then one block per remote
//! host, without changing the merged indices the drag/drop handlers dispatch.
//! The pure split lives here so the render path and the keyboard cursor walk
//! the identical order. The host CPU / memory widgets live here too.

use gpui::{App, Div, ParentElement, Styled, div, px, relative, rgb};
use okena_core::api::ApiSystemStats;
use okena_transport::client::{ConnectionStatus, LOCAL_DAEMON_CONNECTION_ID};
use okena_workspace::settings::HostStatsStyle;
use okena_workspace::state::Workspace;

use super::Sidebar;

/// Which block of the Separate layout a section is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SectionKind {
    /// `LOCAL` heading: the local machine's projects, stats and `+`.
    Local,
    /// `REMOTE` heading. Just a label — hosts follow as their own sections.
    RemoteGroup,
    /// One remote connection: its status, stats, `+` and projects.
    Host(String),
}

/// Presentation data for a section header.
#[derive(Clone, Debug)]
pub(crate) struct SidebarSectionInfo {
    pub kind: SectionKind,
    /// The connection's display name (unused for the two headings).
    pub name: String,
    /// `host:port` for a remote host.
    pub endpoint: Option<String>,
    pub status: Option<ConnectionStatus>,
    /// Whether the connection is TLS-encrypted (`true` when not a remote).
    pub tls: bool,
    pub stats: Option<ApiSystemStats>,
    pub stats_style: HostStatsStyle,
}

/// One rendered section: an optional header plus its `project_order` entries
/// as `(merged_index, id)`. `info == None` is the mixed layout — no header.
pub(crate) struct OrderedSection {
    pub info: Option<SidebarSectionInfo>,
    pub entries: Vec<(usize, String)>,
}

/// Connection id encoded in a mirrored id (`remote:<conn>:<rest>`), or `None`
/// for a bare local id. The loopback daemon counts as local.
pub(crate) fn remote_connection_of(id: &str) -> Option<&str> {
    let conn = id.strip_prefix("remote:")?.split(':').next()?;
    (!conn.is_empty() && conn != LOCAL_DAEMON_CONNECTION_ID).then_some(conn)
}

/// A host block: `None` is local, `Some(conn_id)` a remote host; entries are
/// `(merged_index, id)`.
pub(crate) type HostBlock = (Option<String>, Vec<(usize, String)>);

/// Host blocks: local entries first, then every host in `remote_hosts` (in
/// that order, even when it has no entries yet), then any host that still has
/// entries but is no longer listed, by id. Entries keep their merged index so
/// reorder actions stay valid. `None` is the local block.
pub(crate) fn split_order_by_host(order: &[String], remote_hosts: &[String]) -> Vec<HostBlock> {
    let mut sections: Vec<HostBlock> = vec![(None, Vec::new())];
    for host in remote_hosts {
        if !sections.iter().any(|(k, _)| k.as_deref() == Some(host)) {
            sections.push((Some(host.clone()), Vec::new()));
        }
    }

    let mut stragglers: Vec<HostBlock> = Vec::new();
    for (index, id) in order.iter().enumerate() {
        let key = remote_connection_of(id).map(str::to_string);
        if let Some((_, entries)) = sections.iter_mut().find(|(k, _)| *k == key) {
            entries.push((index, id.clone()));
        } else if let Some((_, entries)) = stragglers.iter_mut().find(|(k, _)| *k == key) {
            entries.push((index, id.clone()));
        } else {
            stragglers.push((key, vec![(index, id.clone())]));
        }
    }
    stragglers.sort_by(|(a, _), (b, _)| a.cmp(b));
    sections.extend(stragglers);
    sections
}

/// Memory in use as a whole percent (0 when the total is unknown).
pub(crate) fn memory_percent(stats: &ApiSystemStats) -> u64 {
    stats
        .memory_used_bytes
        .saturating_mul(100)
        .checked_div(stats.memory_total_bytes)
        .unwrap_or(0)
}

/// `CPU 12% · MEM 4.2/16.0 GB` — the long one-line form, used for tooltips.
pub(crate) fn format_host_stats(stats: &ApiSystemStats) -> String {
    format!(
        "CPU {:.0}% · MEM {}/{} GB",
        stats.cpu_usage.clamp(0.0, 100.0),
        format_gib_tenths(stats.memory_used_bytes),
        format_gib_tenths(stats.memory_total_bytes),
    )
}

fn format_gib_tenths(bytes: u64) -> String {
    let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    format!("{gib:.1}")
}

/// Human label for a connection that is not (yet) connected.
pub(crate) fn status_label(status: &ConnectionStatus) -> &'static str {
    match status {
        ConnectionStatus::Connected => "Connected",
        ConnectionStatus::Connecting => "Connecting…",
        ConnectionStatus::Pairing => "Pairing…",
        ConnectionStatus::Reconnecting { .. } => "Reconnecting…",
        ConnectionStatus::Disconnected => "Disconnected",
        ConnectionStatus::Error(_) => "Error",
    }
}

/// The host stats widget for `style`, or `None` when stats are off.
pub(crate) fn host_stats_widget(
    style: HostStatsStyle,
    stats: &ApiSystemStats,
    t: &okena_core::theme::ThemeColors,
    cx: &App,
) -> Option<Div> {
    match style {
        HostStatsStyle::Off => None,
        HostStatsStyle::Text => Some(host_stats_text(stats, t, cx)),
        HostStatsStyle::Gauge => Some(host_stats_gauge(stats, t, cx)),
    }
}

/// `CPU 41%  MEM 72%` exactly as the status bar spells it: label muted, value
/// in the load colour.
fn host_stats_text(stats: &ApiSystemStats, t: &okena_core::theme::ThemeColors, cx: &App) -> Div {
    use okena_ui::metrics::{cpu_metric_color, memory_metric_color};
    use okena_ui::tokens::ui_text_sm;

    let cpu = stats.cpu_usage.clamp(0.0, 100.0);
    let mem = memory_percent(stats).min(100);
    let metric = |label: &'static str, value: String, color: u32| {
        div()
            .flex()
            .items_baseline()
            .gap(px(3.0))
            .whitespace_nowrap()
            .child(div().text_color(rgb(t.text_muted)).child(label))
            .child(div().text_color(rgb(color)).child(value))
    };

    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .flex_shrink_0()
        .text_size(ui_text_sm(cx))
        .child(metric(
            "CPU",
            format!("{cpu:.0}%"),
            cpu_metric_color(cpu, t),
        ))
        .child(metric(
            "MEM",
            format!("{mem}%"),
            memory_metric_color(mem, t),
        ))
}

/// `CPU` and `MEM` labels, each followed by a 3×10px gauge that fills from the
/// bottom in the load colour. The exact numbers are in the row's tooltip.
fn host_stats_gauge(stats: &ApiSystemStats, t: &okena_core::theme::ThemeColors, cx: &App) -> Div {
    use okena_ui::metrics::{cpu_metric_color, memory_metric_color, metric_track_color};
    use okena_ui::tokens::ui_text_xs;

    let cpu = stats.cpu_usage.clamp(0.0, 100.0);
    let mem = memory_percent(stats).min(100);
    let gauge = |fraction: f32, color: u32| {
        div()
            .w(px(3.0))
            .h(px(10.0))
            .rounded(px(2.0))
            .bg(metric_track_color(t))
            .overflow_hidden()
            .flex()
            .flex_col()
            .justify_end()
            .child(
                div()
                    .w_full()
                    .h(relative(fraction.clamp(0.0, 1.0)))
                    .bg(rgb(color)),
            )
    };
    let metric = |label: &'static str, fraction: f32, color: u32| {
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(ui_text_xs(cx))
                    .text_color(rgb(t.text_muted))
                    .child(label),
            )
            .child(gauge(fraction, color))
    };

    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .flex_shrink_0()
        .child(metric("CPU", cpu / 100.0, cpu_metric_color(cpu, t)))
        .child(metric(
            "MEM",
            mem as f32 / 100.0,
            memory_metric_color(mem, t),
        ))
}

impl Sidebar {
    /// The manual layout's `project_order`, regrouped into sections according
    /// to the `remote_layout` setting. Mixed layout yields a single headerless
    /// section in `project_order` order. Separate yields `LOCAL`, `REMOTE`,
    /// then every remote connection (connected or not, by name) so a host
    /// that is reconnecting keeps its place and its context menu.
    pub(crate) fn ordered_sections(&self, workspace: &Workspace, cx: &App) -> Vec<OrderedSection> {
        let order = &workspace.data().project_order;
        let settings = self.sidebar_settings(cx);
        if !settings.remote_layout.is_separate() {
            return vec![OrderedSection {
                info: None,
                entries: order.iter().cloned().enumerate().collect(),
            }];
        }

        let mut connections = self.remote_connection_snapshots(cx);
        // Stable display order: by name, then id, so a reconnect never
        // reshuffles sections.
        connections.sort_by(|a, b| {
            a.config
                .name
                .cmp(&b.config.name)
                .then_with(|| a.config.id.cmp(&b.config.id))
        });
        let local_stats = connections
            .iter()
            .find(|c| c.config.id == LOCAL_DAEMON_CONNECTION_ID)
            .and_then(|c| c.system_stats.clone());
        let remote_ids: Vec<String> = connections
            .iter()
            .filter(|c| c.config.id != LOCAL_DAEMON_CONNECTION_ID)
            .map(|c| c.config.id.clone())
            .collect();
        let stats_style = settings.host_stats;
        let stats_if_on = |stats: Option<ApiSystemStats>| stats.filter(|_| stats_style.is_on());

        let mut sections = Vec::new();
        for (key, entries) in split_order_by_host(order, &remote_ids) {
            match key {
                None => sections.push(OrderedSection {
                    info: Some(SidebarSectionInfo {
                        kind: SectionKind::Local,
                        name: "Local".to_string(),
                        endpoint: None,
                        status: None,
                        tls: true,
                        stats: stats_if_on(local_stats.clone()),
                        stats_style,
                    }),
                    entries,
                }),
                Some(conn_id) => {
                    if sections.len() == 1 {
                        sections.push(OrderedSection {
                            info: Some(SidebarSectionInfo {
                                kind: SectionKind::RemoteGroup,
                                name: "Remote".to_string(),
                                endpoint: None,
                                status: None,
                                tls: true,
                                stats: None,
                                stats_style,
                            }),
                            entries: Vec::new(),
                        });
                    }
                    let snap = connections.iter().find(|c| c.config.id == conn_id);
                    sections.push(OrderedSection {
                        info: Some(SidebarSectionInfo {
                            kind: SectionKind::Host(conn_id.clone()),
                            name: snap
                                .map(|c| c.config.name.clone())
                                .unwrap_or_else(|| conn_id.clone()),
                            endpoint: snap.map(|c| c.config.display_endpoint()),
                            status: snap.map(|c| c.status.clone()),
                            tls: snap.is_none_or(|c| c.config.tls),
                            stats: stats_if_on(snap.and_then(|c| c.system_stats.clone())),
                            stats_style,
                        }),
                        entries,
                    });
                }
            }
        }
        // No remote connections at all: still show the REMOTE heading so the
        // layout reads the same and the bottom "+ Add Connection" has a home.
        if sections.len() == 1 {
            sections.push(OrderedSection {
                info: Some(SidebarSectionInfo {
                    kind: SectionKind::RemoteGroup,
                    name: "Remote".to_string(),
                    endpoint: None,
                    status: None,
                    tls: true,
                    stats: None,
                    stats_style,
                }),
                entries: Vec::new(),
            });
        }
        sections
    }

    /// Flattened `ordered_sections` ids, for walkers that only need the order
    /// (keyboard cursor).
    pub(crate) fn ordered_project_ids(&self, workspace: &Workspace, cx: &App) -> Vec<String> {
        self.ordered_sections(workspace, cx)
            .into_iter()
            .flat_map(|s| s.entries.into_iter().map(|(_, id)| id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn remote_connection_of_reads_prefix_and_treats_loopback_as_local() {
        assert_eq!(remote_connection_of("remote:pi:abc"), Some("pi"));
        assert_eq!(
            remote_connection_of("remote:pi:folder:with:colons"),
            Some("pi")
        );
        assert_eq!(remote_connection_of("remote:local-daemon:abc"), None);
        assert_eq!(remote_connection_of("plain-id"), None);
        assert_eq!(remote_connection_of("remote:"), None);
    }

    #[test]
    fn split_puts_local_first_then_listed_hosts_then_stragglers() {
        let order = ids(&[
            "remote:zeta:p1",
            "remote:local-daemon:l1",
            "remote:alpha:p2",
            "l2",
            "remote:gone:p3",
            "remote:alpha:p4",
        ]);
        let sections = split_order_by_host(&order, &ids(&["zeta", "alpha", "empty"]));
        let keys: Vec<Option<&str>> = sections.iter().map(|(k, _)| k.as_deref()).collect();
        assert_eq!(
            keys,
            vec![
                None,
                Some("zeta"),
                Some("alpha"),
                Some("empty"),
                Some("gone")
            ]
        );
        // Merged indices survive the regrouping.
        assert_eq!(
            sections[0].1,
            vec![
                (1, "remote:local-daemon:l1".to_string()),
                (3, "l2".to_string())
            ]
        );
        assert_eq!(
            sections[2].1,
            vec![
                (2, "remote:alpha:p2".to_string()),
                (5, "remote:alpha:p4".to_string())
            ]
        );
        assert!(sections[3].1.is_empty());
        assert_eq!(sections[4].1, vec![(4, "remote:gone:p3".to_string())]);
    }

    #[test]
    fn split_dedups_listed_hosts_and_sorts_stragglers_by_id() {
        let order = ids(&["remote:b:1", "remote:a:1"]);
        let sections = split_order_by_host(&order, &ids(&["x", "x"]));
        let keys: Vec<Option<&str>> = sections.iter().map(|(k, _)| k.as_deref()).collect();
        assert_eq!(keys, vec![None, Some("x"), Some("a"), Some("b")]);
    }

    #[test]
    fn memory_percent_handles_zero_total() {
        let gib = 1024u64 * 1024 * 1024;
        let stats = ApiSystemStats {
            cpu_usage: 0.0,
            memory_used_bytes: 4 * gib,
            memory_total_bytes: 16 * gib,
        };
        assert_eq!(memory_percent(&stats), 25);
        let unknown = ApiSystemStats {
            memory_total_bytes: 0,
            ..stats
        };
        assert_eq!(memory_percent(&unknown), 0);
    }

    #[test]
    fn format_host_stats_is_compact_and_clamped() {
        let gib = 1024u64 * 1024 * 1024;
        let stats = ApiSystemStats {
            cpu_usage: 12.4,
            memory_used_bytes: 4 * gib + gib / 5,
            memory_total_bytes: 16 * gib,
        };
        assert_eq!(format_host_stats(&stats), "CPU 12% · MEM 4.2/16.0 GB");
        let over = ApiSystemStats {
            cpu_usage: 140.0,
            memory_used_bytes: 0,
            memory_total_bytes: 0,
        };
        assert_eq!(format_host_stats(&over), "CPU 100% · MEM 0.0/0.0 GB");
    }
}
