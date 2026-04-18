//! Centralized port discovery poller: builds the process tree once per cycle
//! and distributes listening ports to all services awaiting detection.

use super::{PortDetectionState, ServiceManager, ServiceStatus};
use crate::port_detect;
use gpui::{Context, WeakEntity};
use std::time::Duration;

impl ServiceManager {
    /// Register a service for centralized port detection polling.
    /// The centralized poller calls `ss`/`lsof`/`netstat` once per cycle
    /// and distributes results to all registered services.
    pub(super) fn start_port_detection(
        &mut self,
        project_id: &str,
        service_name: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (project_id.to_string(), service_name.to_string());
        if self.instances.get(&key).and_then(|i| i.terminal_id.as_ref()).is_none() {
            return;
        }
        self.port_detection_active.insert(
            key,
            PortDetectionState {
                polls_remaining: 10,
                found_any: false,
                stable_count: 0,
            },
        );
        self.ensure_port_detection_poller(cx);
    }

    /// Ensure the centralized port detection poller is running.
    /// One poller handles all services: builds the process tree once,
    /// calls the port scanner once, then distributes results.
    fn ensure_port_detection_poller(&mut self, cx: &mut Context<Self>) {
        if self.port_detection_running {
            return;
        }
        self.port_detection_running = true;
        let backend = self.backend.clone();

        cx.spawn(async move |this: WeakEntity<ServiceManager>, cx| {
            // Initial delay — let newly started services bind their ports
            cx.background_executor().timer(Duration::from_secs(2)).await;

            loop {
                // Collect all services that need port detection + their terminal IDs
                let services: Vec<((String, String), String)> = this
                    .update(cx, |this, _| {
                        this.port_detection_active
                            .keys()
                            .filter_map(|key| {
                                let inst = this.instances.get(key)?;
                                if inst.status != ServiceStatus::Running {
                                    return None;
                                }
                                let tid = inst.terminal_id.clone()?;
                                Some((key.clone(), tid))
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                if services.is_empty() {
                    let _ = this.update(cx, |this, _| {
                        this.port_detection_active.clear();
                        this.port_detection_running = false;
                    });
                    return;
                }

                // Background: get PIDs per service, build process tree ONCE,
                // scan ports ONCE, distribute results.
                let backend_ref = backend.clone();
                let results: Vec<((String, String), Vec<u16>)> = cx
                    .background_executor()
                    .spawn(async move {
                        // Get root PIDs for all services in one batch.
                        // On Linux+dtach this reads /proc once instead of spawning lsof per terminal.
                        let terminal_ids: Vec<&str> =
                            services.iter().map(|(_, tid)| tid.as_str()).collect();
                        let batch_pids = backend_ref.get_batch_service_pids(&terminal_ids);
                        let service_root_pids: Vec<((String, String), Vec<u32>)> = services
                            .iter()
                            .map(|(key, tid)| {
                                let pids = batch_pids
                                    .get(tid.as_str())
                                    .cloned()
                                    .unwrap_or_default();
                                (key.clone(), pids)
                            })
                            .collect();

                        // Build process tree ONCE for all services
                        let tree = port_detect::build_process_tree();

                        // Expand to descendant PIDs per service
                        let mut all_pids: std::collections::HashSet<u32> = std::collections::HashSet::new();
                        let service_pid_sets: Vec<(
                            (String, String),
                            std::collections::HashSet<u32>,
                        )> = service_root_pids
                            .into_iter()
                            .map(|(key, roots)| {
                                let mut pids = std::collections::HashSet::new();
                                for &pid in &roots {
                                    pids.extend(port_detect::descendants_from_tree(&tree, pid));
                                }
                                all_pids.extend(&pids);
                                (key, pids)
                            })
                            .collect();

                        if all_pids.is_empty() {
                            return service_pid_sets
                                .into_iter()
                                .map(|(k, _)| (k, Vec::new()))
                                .collect();
                        }

                        // ONE system call for port scanning
                        let all_port_pairs = port_detect::get_listening_port_pairs();

                        // Distribute ports to each service
                        service_pid_sets
                            .into_iter()
                            .map(|(key, pids)| {
                                let ports = port_detect::ports_for_pids(&all_port_pairs, &pids);
                                (key, ports)
                            })
                            .collect()
                    })
                    .await;

                // Update instances and port detection state
                let has_remaining = this
                    .update(cx, |this, cx| {
                        let mut changed = false;
                        let mut keys_to_remove = Vec::new();

                        for (key, ports) in results {
                            let Some(state) = this.port_detection_active.get_mut(&key) else {
                                continue;
                            };

                            state.polls_remaining = state.polls_remaining.saturating_sub(1);

                            if !ports.is_empty() {
                                let ports_changed =
                                    if let Some(inst) = this.instances.get_mut(&key) {
                                        if inst.status == ServiceStatus::Running
                                            && inst.detected_ports != ports
                                        {
                                            inst.detected_ports = ports;
                                            true
                                        } else {
                                            false
                                        }
                                    } else {
                                        false
                                    };

                                if state.found_any && !ports_changed {
                                    state.stable_count += 1;
                                    if state.stable_count >= 2 {
                                        keys_to_remove.push(key.clone());
                                        continue;
                                    }
                                } else {
                                    state.stable_count = 0;
                                }
                                state.found_any = true;
                                if ports_changed {
                                    changed = true;
                                }
                            }

                            if state.polls_remaining == 0 {
                                keys_to_remove.push(key.clone());
                            }
                        }

                        for key in keys_to_remove {
                            this.port_detection_active.remove(&key);
                        }

                        if changed {
                            cx.notify();
                        }

                        !this.port_detection_active.is_empty()
                    })
                    .unwrap_or(false);

                if !has_remaining {
                    let _ = this.update(cx, |this, _| {
                        this.port_detection_running = false;
                    });
                    return;
                }

                cx.background_executor().timer(Duration::from_secs(3)).await;
            }
        })
        .detach();
    }
}
