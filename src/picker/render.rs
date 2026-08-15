use crate::config::compile_pattern_specs;
use crate::hints::{assign_hints, HintAssignments};
use crate::model::{
    MatchSpan, PickerAction, PickerSnapshot, RenderLine, RenderSpan, RenderStyle,
    SourcePaneGeometry,
};
use crate::patterns::{find_matches, find_openable_urls};
use crate::renderer::{render_inline_hints, render_visible_inline_hints};

/// Rendered picker state and hint assignments derived from a captured pane snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerView {
    pub lines: Vec<RenderLine>,
    pub assignments: HintAssignments,
    pub match_count: usize,
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

/// Builds the production picker view for the pane the picker took over.
pub fn build_picker_view(snapshot: &PickerSnapshot) -> PickerView {
    let Some(target) = snapshot.source.target_pane() else {
        return PickerView {
            lines: Vec::new(),
            assignments: HintAssignments::new(Vec::new()),
            match_count: 0,
        };
    };

    let matches = find_pane_matches(snapshot, target);
    let assignments = assign_hints(matches.clone());
    let lines = if assignments.is_empty() {
        no_matches_view(snapshot.action, target.content_width, target.content_height)
    } else {
        render_pane(target, &assignments)
    };

    PickerView {
        lines,
        assignments,
        match_count: matches.len(),
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
        OpenSettings, PaneId, PaneTextCaptureMode, PickerReturnContext, Rect, SourcePaneSnapshot,
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
            custom_patterns: Vec::new(),
            open: OpenSettings::default(),
        }
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
