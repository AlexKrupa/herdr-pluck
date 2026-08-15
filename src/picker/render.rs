use crate::config::compile_pattern_specs;
use crate::hints::{assign_hints, HintAssignments};
use crate::model::{
    HintAssignment, MatchSpan, PickerAction, PickerScope, PickerSnapshot, RenderLine, RenderSpan,
    RenderStyle, SourcePaneGeometry,
};
use crate::patterns::{find_matches, find_openable_urls};
use crate::renderer::{render_inline_hints, render_visible_inline_hints};
use std::collections::HashMap;

/// Rendered picker state and hint assignments derived from a captured pane snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerView {
    pub lines: Vec<RenderLine>,
    pub assignments: HintAssignments,
}

fn find_pane_matches(snapshot: &PickerSnapshot, pane: &SourcePaneGeometry) -> Vec<MatchSpan> {
    let logical_lines = pane
        .visible_viewport
        .as_ref()
        .map(|viewport| viewport.logical_lines.as_slice())
        .unwrap_or(&pane.logical_lines);
    match snapshot.action {
        PickerAction::Copy => {
            let custom_patterns = compile_pattern_specs(&snapshot.custom_patterns);
            find_matches(logical_lines, &custom_patterns)
        }
        PickerAction::OpenUrl => find_openable_urls(logical_lines),
    }
}

pub fn render_pane(pane: &SourcePaneGeometry, assignments: &HintAssignments) -> Vec<RenderLine> {
    match &pane.visible_viewport {
        Some(viewport) => render_visible_inline_hints(
            viewport,
            assignments,
            pane.content_width,
            pane.content_height,
        ),
        None => render_inline_hints(
            &pane.logical_lines,
            assignments,
            pane.content_width,
            pane.content_height,
        ),
    }
}

/// Panes contributing matches, ordered top-to-bottom then left-to-right.
pub fn panes_in_scope(snapshot: &PickerSnapshot) -> Vec<&SourcePaneGeometry> {
    let mut panes: Vec<&SourcePaneGeometry> = match snapshot.scope {
        PickerScope::TargetPane => snapshot.source.target_pane().into_iter().collect(),
        PickerScope::AllPanes => snapshot.source.source_panes.iter().collect(),
    };
    panes.sort_by_key(|pane| (pane.outer_rect.y, pane.outer_rect.x));
    panes
}

/// The first pane in scope order whose matches include this text.
pub fn pane_for_text<'a>(
    snapshot: &'a PickerSnapshot,
    text: &str,
) -> Option<&'a SourcePaneGeometry> {
    panes_in_scope(snapshot).into_iter().find(|pane| {
        find_pane_matches(snapshot, pane)
            .iter()
            .any(|span| span.text == text)
    })
}

/// Assigns hints once over every pane in scope, so all picker processes agree.
pub fn assign_global_hints(snapshot: &PickerSnapshot) -> HintAssignments {
    let matches = panes_in_scope(snapshot)
        .into_iter()
        .flat_map(|pane| find_pane_matches(snapshot, pane))
        .collect();
    assign_hints(matches)
}

/// Narrows a global assignment to one pane's own occurrences, keeping the global hint strings.
pub fn local_assignments(
    snapshot: &PickerSnapshot,
    pane: &SourcePaneGeometry,
    global: &HintAssignments,
) -> HintAssignments {
    let mut by_text: HashMap<String, Vec<MatchSpan>> = HashMap::new();
    for span in find_pane_matches(snapshot, pane) {
        by_text.entry(span.text.clone()).or_default().push(span);
    }

    let assignments = global
        .assignments()
        .iter()
        .filter_map(|assignment| {
            by_text
                .remove(&assignment.text)
                .map(|occurrences| HintAssignment {
                    hint: assignment.hint.clone(),
                    text: assignment.text.clone(),
                    occurrences,
                })
        })
        .collect();
    HintAssignments::new(assignments)
}

/// Builds the production picker view: global hints for input, local occurrences for drawing.
pub fn build_picker_view(snapshot: &PickerSnapshot) -> PickerView {
    let global = assign_global_hints(snapshot);
    let Some(target) = snapshot.source.target_pane() else {
        return PickerView {
            lines: Vec::new(),
            assignments: global,
        };
    };

    let lines = if global.is_empty() {
        no_matches_view(snapshot.action, target.content_width, target.content_height)
    } else {
        render_pane(target, &local_assignments(snapshot, target, &global))
    };

    PickerView {
        lines,
        assignments: global,
    }
}

fn no_matches_view(action: PickerAction, width: u16, height: u16) -> Vec<RenderLine> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let width = width as usize;
    let height = height as usize;
    let mut lines = Vec::with_capacity(height);
    let message = match action {
        PickerAction::Copy => "Herdr Pluck: no copyable matches found",
        PickerAction::OpenUrl => "Herdr Pluck: no openable URLs found",
    };
    let hint = "Press any non-Enter key to close";

    for row in 0..height {
        let text = match row {
            0 => fit_to_width(message, width),
            2 if height > 2 => fit_to_width(hint, width),
            _ => " ".repeat(width),
        };
        lines.push(RenderLine {
            spans: vec![RenderSpan {
                text,
                style: RenderStyle::Unmatched,
            }],
        });
    }

    lines
}

fn fit_to_width(text: &str, width: usize) -> String {
    let mut output = text.chars().take(width).collect::<String>();
    let current_width = output.chars().count();
    if current_width < width {
        output.push_str(&" ".repeat(width - current_width));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        OpenSettings, PaneId, PaneTextCaptureMode, PickerReturnContext, PickerScope, Rect,
        SourcePaneSnapshot,
    };

    fn snapshot(lines: Vec<&str>, width: u16, height: u16) -> PickerSnapshot {
        PickerSnapshot {
            source: SourcePaneSnapshot {
                target_pane_id: PaneId::new("p1"),
                source_tab_id: "t1".to_string(),
                workspace_id: "w1".to_string(),
                source_panes: vec![SourcePaneGeometry {
                    pane_id: PaneId::new("p1"),
                    outer_rect: Rect::new(0, 0, width, height),
                    content_rect: Rect::new(0, 0, width, height),
                    content_width: width,
                    content_height: height,
                    logical_lines: lines.into_iter().map(str::to_string).collect(),
                    visible_viewport: None,
                    cwd: None,
                }],
                capture_mode: PaneTextCaptureMode::RecentUnwrappedBottomApproximation,
            },
            session: PickerReturnContext {
                return_tab_id: "t1".to_string(),
                return_pane_id: PaneId::new("p1"),
                zoom_picker: false,
            },
            action: PickerAction::Copy,
            scope: PickerScope::TargetPane,
            custom_patterns: Vec::new(),
            open: OpenSettings::default(),
        }
    }

    fn two_pane_snapshot(scope: PickerScope) -> PickerSnapshot {
        let mut snapshot = snapshot(vec!["left https://example.com/a"], 40, 1);
        snapshot.scope = scope;
        snapshot.source.source_panes[0].outer_rect = Rect::new(0, 0, 40, 1);
        snapshot.source.source_panes.push(SourcePaneGeometry {
            pane_id: PaneId::new("p2"),
            outer_rect: Rect::new(40, 0, 40, 1),
            content_rect: Rect::new(40, 0, 40, 1),
            content_width: 40,
            content_height: 1,
            logical_lines: vec!["right https://example.com/b".to_string()],
            visible_viewport: None,
            cwd: None,
        });
        snapshot
    }

    #[test]
    fn all_panes_scope_assigns_one_namespace_in_visual_order() {
        let snapshot = two_pane_snapshot(PickerScope::AllPanes);

        let global = assign_global_hints(&snapshot);

        assert_eq!(global.len(), 2);
        assert_eq!(
            global.copied_text_for_hint("a"),
            Some("https://example.com/a")
        );
        assert_eq!(
            global.copied_text_for_hint("s"),
            Some("https://example.com/b")
        );
    }

    #[test]
    fn target_scope_ignores_other_panes() {
        let snapshot = two_pane_snapshot(PickerScope::TargetPane);

        let global = assign_global_hints(&snapshot);

        assert_eq!(global.len(), 1);
        assert_eq!(
            global.copied_text_for_hint("a"),
            Some("https://example.com/a")
        );
    }

    #[test]
    fn duplicate_text_across_panes_shares_one_hint() {
        let mut snapshot = two_pane_snapshot(PickerScope::AllPanes);
        snapshot.source.source_panes[1].logical_lines =
            vec!["right https://example.com/a".to_string()];

        let global = assign_global_hints(&snapshot);

        assert_eq!(global.len(), 1);
        assert_eq!(global.assignments()[0].occurrences.len(), 2);
    }

    #[test]
    fn local_assignments_keep_the_global_hint_and_only_own_occurrences() {
        let snapshot = two_pane_snapshot(PickerScope::AllPanes);
        let global = assign_global_hints(&snapshot);
        let second = &snapshot.source.source_panes[1];

        let local = local_assignments(&snapshot, second, &global);

        assert_eq!(local.len(), 1);
        assert_eq!(local.assignments()[0].hint, "s");
        assert_eq!(local.assignments()[0].text, "https://example.com/b");
        assert_eq!(local.assignments()[0].occurrences.len(), 1);
        assert_eq!(local.assignments()[0].occurrences[0].line, 0);
    }

    #[test]
    fn picker_view_renders_inline_hints_for_matches() {
        let view = build_picker_view(&snapshot(vec!["open https://example.com/path"], 40, 1));

        assert_eq!(view.assignments.len(), 1);
        assert_eq!(view.lines.len(), 1);
        assert!(view.lines[0]
            .spans
            .iter()
            .any(|span| span.style == RenderStyle::Hint && span.text == "a"));
    }

    #[test]
    fn open_url_view_assigns_hints_only_to_openable_urls() {
        let mut snapshot = snapshot(vec!["https://example.com ftp://host/repo /tmp/file"], 50, 1);
        snapshot.action = PickerAction::OpenUrl;

        let view = build_picker_view(&snapshot);

        assert_eq!(view.assignments.len(), 1);
        assert_eq!(
            view.assignments.copied_text_for_hint("a"),
            Some("https://example.com")
        );
    }

    #[test]
    fn open_url_no_match_view_uses_action_specific_message() {
        let mut snapshot = snapshot(vec!["ftp://host/repo"], 40, 3);
        snapshot.action = PickerAction::OpenUrl;

        let view = build_picker_view(&snapshot);

        assert!(view.lines[0].spans[0].text.contains("no openable URLs"));
    }

    #[test]
    fn picker_view_reports_no_matches_with_full_size_message() {
        let view = build_picker_view(&snapshot(vec!["plain text only"], 20, 3));

        assert_eq!(view.assignments.len(), 0);
        assert_eq!(view.lines.len(), 3);
        assert_eq!(view.lines[0].spans[0].text.len(), 20);
        assert!(view.lines[0].spans[0].text.starts_with("Herdr Pluck"));
        assert!(view.lines[2].spans[0].text.starts_with("Press"));
    }
}
