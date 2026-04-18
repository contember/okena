//! Pure-data tests of LayoutNode tree operations using simulation helpers
//! (no GPUI context required).

use crate::state::{LayoutNode, SplitDirection};
use okena_terminal::shell_config::ShellType;

fn terminal_node(id: &str) -> LayoutNode {
    LayoutNode::Terminal {
        terminal_id: Some(id.to_string()),
        minimized: false,
        detached: false,
        shell_type: ShellType::Default,
        zoom_level: 1.0,
    }
}

/// Simulate split_terminal: replace a node with a Split containing it + new terminal
fn simulate_split(node: &mut LayoutNode, direction: SplitDirection) {
    let old_node = node.clone();
    *node = LayoutNode::Split {
        direction,
        sizes: vec![50.0, 50.0],
        children: vec![old_node, LayoutNode::new_terminal()],
    };
    node.normalize();
}

/// Simulate add_tab: replace a node with a Tabs containing it + new terminal
fn simulate_add_tab(node: &mut LayoutNode) {
    let old_node = node.clone();
    *node = LayoutNode::Tabs {
        children: vec![old_node, LayoutNode::new_terminal()],
        active_tab: 1,
    };
}

/// Simulate close_terminal: remove child at index, replacing parent with sibling if 2 children
fn simulate_close(layout: &mut LayoutNode, path: &[usize]) -> bool {
    if path.is_empty() {
        return false; // would set layout to None in real code
    }
    let parent_path = &path[..path.len() - 1];
    let child_index = path[path.len() - 1];

    if let Some(parent) = layout.get_at_path_mut(parent_path) {
        match parent {
            LayoutNode::Split { children, sizes, .. } => {
                if children.len() <= 2 {
                    let remaining_index = if child_index == 0 { 1 } else { 0 };
                    if let Some(remaining) = children.get(remaining_index).cloned() {
                        *parent = remaining;
                        return true;
                    }
                } else {
                    children.remove(child_index);
                    if child_index < sizes.len() {
                        sizes.remove(child_index);
                    }
                    return true;
                }
            }
            LayoutNode::Tabs { children, .. } => {
                if children.len() <= 2 {
                    let remaining_index = if child_index == 0 { 1 } else { 0 };
                    if let Some(remaining) = children.get(remaining_index).cloned() {
                        *parent = remaining;
                        return true;
                    }
                } else {
                    children.remove(child_index);
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

#[test]
fn test_split_terminal_creates_split() {
    let mut layout = terminal_node("t1");
    simulate_split(&mut layout, SplitDirection::Vertical);

    match &layout {
        LayoutNode::Split { direction, children, sizes } => {
            assert_eq!(*direction, SplitDirection::Vertical);
            assert_eq!(children.len(), 2);
            assert_eq!(sizes.len(), 2);
            assert!(matches!(&children[0], LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "t1"));
            assert!(matches!(&children[1], LayoutNode::Terminal { terminal_id: None, .. }));
        }
        _ => panic!("Expected split"),
    }
}

#[test]
fn test_nested_split_normalizes() {
    // Split a terminal that's already inside a split of the same direction
    let mut layout = LayoutNode::Split {
        direction: SplitDirection::Horizontal,
        sizes: vec![50.0, 50.0],
        children: vec![terminal_node("t1"), terminal_node("t2")],
    };
    // Split t1 horizontally — should flatten
    if let Some(node) = layout.get_at_path_mut(&[0]) {
        simulate_split(node, SplitDirection::Horizontal);
    }
    layout.normalize();

    match &layout {
        LayoutNode::Split { direction, children, .. } => {
            assert_eq!(*direction, SplitDirection::Horizontal);
            // Should be flattened to 3 children
            assert_eq!(children.len(), 3);
        }
        _ => panic!("Expected flattened split"),
    }
}

#[test]
fn test_add_tab_creates_tab_group() {
    let mut layout = terminal_node("t1");
    simulate_add_tab(&mut layout);

    match &layout {
        LayoutNode::Tabs { children, active_tab } => {
            assert_eq!(children.len(), 2);
            assert_eq!(*active_tab, 1);
        }
        _ => panic!("Expected tabs"),
    }
}

#[test]
fn test_add_tab_to_existing_tabs() {
    let mut layout = LayoutNode::Tabs {
        children: vec![terminal_node("t1"), terminal_node("t2")],
        active_tab: 0,
    };
    if let LayoutNode::Tabs { children, active_tab } = &mut layout {
        children.push(LayoutNode::new_terminal());
        *active_tab = children.len() - 1;
    }
    match &layout {
        LayoutNode::Tabs { children, active_tab } => {
            assert_eq!(children.len(), 3);
            assert_eq!(*active_tab, 2);
        }
        _ => panic!("Expected tabs"),
    }
}

#[test]
fn test_close_terminal_sibling_replaces_parent() {
    let mut layout = LayoutNode::Split {
        direction: SplitDirection::Horizontal,
        sizes: vec![50.0, 50.0],
        children: vec![terminal_node("t1"), terminal_node("t2")],
    };
    simulate_close(&mut layout, &[0]);
    assert!(matches!(&layout, LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "t2"));
}

#[test]
fn test_close_terminal_from_3_child_split() {
    let mut layout = LayoutNode::Split {
        direction: SplitDirection::Horizontal,
        sizes: vec![33.0, 33.0, 34.0],
        children: vec![terminal_node("t1"), terminal_node("t2"), terminal_node("t3")],
    };
    simulate_close(&mut layout, &[1]);
    match &layout {
        LayoutNode::Split { children, sizes, .. } => {
            assert_eq!(children.len(), 2);
            assert_eq!(sizes.len(), 2);
            // t1 and t3 remain
            let ids: Vec<_> = children.iter().map(|c| match c {
                LayoutNode::Terminal { terminal_id: Some(id), .. } => id.as_str(),
                _ => "",
            }).collect();
            assert_eq!(ids, vec!["t1", "t3"]);
        }
        _ => panic!("Expected split with 2 children"),
    }
}

#[test]
fn test_close_terminal_from_3_child_sizes_consistent() {
    // Verify that closing a child from a 3-child split keeps sizes in sync
    // and that the remaining sizes sum correctly
    let mut layout = LayoutNode::Split {
        direction: SplitDirection::Horizontal,
        sizes: vec![25.0, 50.0, 25.0],
        children: vec![terminal_node("t1"), terminal_node("t2"), terminal_node("t3")],
    };

    // Close the middle terminal (index 1, size 50.0)
    simulate_close(&mut layout, &[1]);
    match &layout {
        LayoutNode::Split { children, sizes, .. } => {
            assert_eq!(children.len(), 2);
            assert_eq!(sizes.len(), 2);
            // Sizes should be [25.0, 25.0] — the middle entry was removed
            assert_eq!(sizes, &vec![25.0, 25.0]);
        }
        _ => panic!("Expected split with 2 children"),
    }

    // Close the first terminal (index 0) — should collapse to single terminal
    let mut layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![30.0, 40.0, 30.0],
        children: vec![terminal_node("t1"), terminal_node("t2"), terminal_node("t3")],
    };
    simulate_close(&mut layout, &[0]);
    match &layout {
        LayoutNode::Split { children, sizes, .. } => {
            assert_eq!(children.len(), 2);
            assert_eq!(sizes.len(), 2);
            assert_eq!(sizes, &vec![40.0, 30.0]);
        }
        _ => panic!("Expected split with 2 children"),
    }
}

#[test]
fn test_move_tab() {
    let mut layout = LayoutNode::Tabs {
        children: vec![terminal_node("t1"), terminal_node("t2"), terminal_node("t3")],
        active_tab: 0,
    };
    // Move tab at index 0 to index 2
    if let LayoutNode::Tabs { children, active_tab } = &mut layout {
        let tab = children.remove(0);
        children.insert(2.min(children.len()), tab);
        // active_tab was 0, which was the moved tab, so update
        *active_tab = 2.min(children.len() - 1);
    }
    match &layout {
        LayoutNode::Tabs { children, active_tab } => {
            let ids: Vec<_> = children.iter().map(|c| match c {
                LayoutNode::Terminal { terminal_id: Some(id), .. } => id.as_str(),
                _ => "",
            }).collect();
            assert_eq!(ids, vec!["t2", "t3", "t1"]);
            assert_eq!(*active_tab, 2);
        }
        _ => panic!("Expected tabs"),
    }
}

#[test]
fn test_equalize_parent_split_via_path() {
    // Simulates what equalize_to_focused does for pane equalization:
    // given a layout path, find the parent split and equalize its sizes.
    let mut layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![70.0, 30.0],
        children: vec![terminal_node("t1"), terminal_node("t2")],
    };
    // Focused terminal at path [1] → parent split at path []
    let parent_path: &[usize] = &[];
    if let Some(node) = layout.get_at_path_mut(parent_path) {
        if let LayoutNode::Split { sizes, children, .. } = node {
            let n = children.len();
            *sizes = vec![100.0 / n as f32; n];
        }
    }
    if let LayoutNode::Split { sizes, .. } = &layout {
        assert_eq!(sizes, &vec![50.0, 50.0]);
    } else {
        panic!("Expected split");
    }
}

#[test]
fn test_equalize_nested_parent_split_via_path() {
    // Nested split: focused terminal at path [0, 1] → parent at [0]
    let mut layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        sizes: vec![60.0, 40.0],
        children: vec![
            LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                sizes: vec![80.0, 20.0],
                children: vec![terminal_node("t1"), terminal_node("t2")],
            },
            terminal_node("t3"),
        ],
    };
    let parent_path: &[usize] = &[0];
    if let Some(node) = layout.get_at_path_mut(parent_path) {
        if let LayoutNode::Split { sizes, children, .. } = node {
            let n = children.len();
            *sizes = vec![100.0 / n as f32; n];
        }
    }
    // Outer split unchanged
    if let LayoutNode::Split { sizes, children, .. } = &layout {
        assert_eq!(sizes, &vec![60.0, 40.0]);
        // Inner split equalized
        if let LayoutNode::Split { sizes: inner, .. } = &children[0] {
            assert_eq!(inner, &vec![50.0, 50.0]);
        } else {
            panic!("Expected inner split");
        }
    } else {
        panic!("Expected split");
    }
}
