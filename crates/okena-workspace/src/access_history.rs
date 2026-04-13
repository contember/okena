//! Tracks per-project last-access timestamps for "recently used" sorting.

use std::collections::HashMap;
use std::time::Instant;

#[derive(Debug, Default)]
pub struct ProjectAccessHistory {
    access_times: HashMap<String, Instant>,
}

impl ProjectAccessHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that a project was just accessed.
    pub fn touch(&mut self, project_id: &str) {
        self.access_times.insert(project_id.to_string(), Instant::now());
    }

    pub fn accessed_at(&self, project_id: &str) -> Option<Instant> {
        self.access_times.get(project_id).copied()
    }

    /// Compare two project IDs by recency. Most-recent-first ordering;
    /// previously accessed projects sort before never-accessed ones.
    pub fn cmp_by_recency(&self, a: &str, b: &str) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let time_a = self.access_times.get(a);
        let time_b = self.access_times.get(b);
        match (time_a, time_b) {
            (Some(ta), Some(tb)) => tb.cmp(ta),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn touch_and_accessed_at() {
        let mut history = ProjectAccessHistory::new();
        assert!(history.accessed_at("p1").is_none());
        history.touch("p1");
        assert!(history.accessed_at("p1").is_some());
    }

    #[test]
    fn cmp_by_recency_prefers_accessed() {
        let mut history = ProjectAccessHistory::new();
        history.touch("p1");
        assert_eq!(history.cmp_by_recency("p1", "p2"), Ordering::Less);
        assert_eq!(history.cmp_by_recency("p2", "p1"), Ordering::Greater);
        assert_eq!(history.cmp_by_recency("p2", "p3"), Ordering::Equal);
    }
}
