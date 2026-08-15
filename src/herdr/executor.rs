use crate::herdr::client::{HerdrClient, LaunchLayoutNode};
use crate::herdr::layout::{
    derive_layout_recreation_plan, derive_source_geometry, derive_source_pane_geometries,
    LayoutSnapshot,
};
use crate::herdr::snapshot::{build_source_snapshot, PickerLaunchFiles};
use crate::model::{
    LayoutNode, OpenSettings, PaneId, PatternSpec, PickerAction, PickerOutcome,
    PickerReturnContext, PickerScope, PickerSnapshot, SourcePaneGeometry,
};
use crate::viewport::map_visible_viewport;
use anyhow::{bail, Context, Result};
use std::path::Path;

/// Captures every source pane: its working directory and its unwrapped visible text.
pub fn capture_panes<C: HerdrClient>(
    client: &mut C,
    layout: &LayoutSnapshot,
) -> Result<Vec<SourcePaneGeometry>> {
    let mut panes = derive_source_pane_geometries(layout);
    let infos = match layout.workspace_id.as_deref() {
        Some(workspace_id) => client.pane_list(workspace_id)?,
        None => Vec::new(),
    };

    for pane in &mut panes {
        pane.cwd = infos
            .iter()
            .find(|info| info.pane_id == pane.pane_id.0)
            .and_then(|info| info.foreground_cwd.clone().or_else(|| info.cwd.clone()));

        if pane.content_height == 0 {
            continue;
        }
        let text = client.pane_read_visible(&pane.pane_id, pane.content_height)?;
        let viewport = map_visible_viewport(
            text.lines().map(str::to_string).collect(),
            pane.content_width,
            pane.content_height,
        );
        pane.logical_lines = viewport.logical_lines.clone();
        pane.visible_viewport = Some(viewport);
    }
    Ok(panes)
}

/// Captures source state and atomically applies the temporary picker layout.
pub fn launch_layout_tab_picker<C: HerdrClient>(
    client: &mut C,
    target: &PaneId,
    binary_path: &Path,
    action: PickerAction,
    scope: PickerScope,
    custom_patterns: Vec<PatternSpec>,
    open: OpenSettings,
) -> Result<()> {
    let layout = client.pane_layout(target)?;
    let plan = derive_layout_recreation_plan(&layout, target)?;
    let geometry = derive_source_geometry(&layout, target);

    if geometry.source_content_rect.height == 0 {
        bail!("target pane {target} has zero visible content height");
    }

    let source_panes = capture_panes(client, &layout)?;

    let return_context = PickerReturnContext {
        return_tab_id: layout
            .tab_id
            .clone()
            .context("pane layout did not include return tab id")?,
        return_pane_id: target.clone(),
        zoom_picker: layout.zoomed && layout_target_is_focused(&layout, target),
    };

    let mut snapshot = build_source_snapshot(
        &layout,
        target,
        source_panes,
        return_context.clone(),
        action,
        custom_patterns,
    )?;
    snapshot.scope = scope;
    snapshot.open = open;

    let files = PickerLaunchFiles::create(&snapshot)?;

    let root = convert_layout(
        &plan.root,
        target,
        binary_path,
        &files.snapshot_path,
        &files.ready_path,
    );

    let workspace_id = layout
        .workspace_id
        .as_deref()
        .context("pane layout did not include workspace id")?;
    let tab_label = match (action, scope) {
        (PickerAction::Copy, PickerScope::TargetPane) => "Herdr Pluck",
        (PickerAction::Copy, PickerScope::AllPanes) => "Herdr Pluck: All Panes",
        (PickerAction::OpenUrl, _) => "Herdr Pluck: Open URL",
    };
    let applied = match client.apply_layout(workspace_id, tab_label, &root) {
        Ok(value) => value,
        Err(error) => {
            let _ = files.cleanup();
            return Err(error);
        }
    };

    let focus_result = client.focus_pane(&applied.picker_pane_id);

    if let Err(error) = focus_result.and_then(|_| files.signal_ready()) {
        if let Err(cleanup) = cleanup_session(client, &return_context, &applied.tab_id) {
            eprintln!("launch cleanup also failed: {cleanup:#}");
        }

        if let Err(cleanup) = files.cleanup() {
            eprintln!("launch file cleanup also failed: {cleanup:#}");
        }

        return Err(error);
    }

    Ok(())
}

fn convert_layout(
    node: &LayoutNode,
    target: &PaneId,
    binary: &Path,
    snapshot: &Path,
    ready: &Path,
) -> LaunchLayoutNode {
    match node {
        LayoutNode::Pane { source_pane_id, .. } if source_pane_id == target => {
            LaunchLayoutNode::Pane {
                command: vec![
                    binary.to_string_lossy().into_owned(),
                    "pick".into(),
                    "--snapshot".into(),
                    snapshot.to_string_lossy().into_owned(),
                    "--ready".into(),
                    ready.to_string_lossy().into_owned(),
                ],
            }
        }
        LayoutNode::Pane { source_pane_id, .. } => LaunchLayoutNode::Pane {
            command: vec![
                binary.to_string_lossy().into_owned(),
                "mirror".into(),
                "--snapshot".into(),
                snapshot.to_string_lossy().into_owned(),
                "--pane".into(),
                source_pane_id.0.clone(),
                "--ready".into(),
                ready.to_string_lossy().into_owned(),
            ],
        },
        LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
            ..
        } => LaunchLayoutNode::Split {
            direction: *direction,
            ratio: *ratio,
            first: Box::new(convert_layout(first, target, binary, snapshot, ready)),
            second: Box::new(convert_layout(second, target, binary, snapshot, ready)),
        },
    }
}

fn layout_target_is_focused(
    layout: &crate::herdr::layout::LayoutSnapshot,
    target: &PaneId,
) -> bool {
    layout
        .focused_pane_id
        .as_ref()
        .is_some_and(|id| id == &target.0)
        || layout
            .panes
            .iter()
            .any(|p| p.pane_id == target.0 && p.focused)
}

/// Restores the source tab and closes only the explicit temporary tab.
pub fn cleanup_session<C: HerdrClient>(
    client: &mut C,
    session: &PickerReturnContext,
    temporary_tab_id: &str,
) -> Result<()> {
    if temporary_tab_id.is_empty() {
        bail!("temporary picker tab id is missing");
    }

    if temporary_tab_id == session.return_tab_id {
        bail!(
            "refusing to close source tab {} as temporary picker tab",
            temporary_tab_id
        );
    }

    let mut first = None;

    if let Err(e) = client.focus_tab(&session.return_tab_id) {
        first = Some(e);
    }

    if let Err(e) = client.close_tab(temporary_tab_id) {
        if first.is_none() {
            first = Some(e);
        }
    }

    first.map_or(Ok(()), Err)
}

pub fn zoom_picker<C: HerdrClient>(
    client: &mut C,
    snapshot: &PickerSnapshot,
    pane_id: &PaneId,
) -> Result<()> {
    if snapshot.session.zoom_picker {
        client.zoom_pane(pane_id)?;
    }

    Ok(())
}

pub fn run_snapshot_picker(snapshot: &PickerSnapshot) -> Result<PickerOutcome> {
    crate::picker::run_picker(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::client::AppliedLayout;
    use crate::herdr::layout::{LayoutPane, LayoutSnapshot, LayoutSplit};
    use crate::model::{
        PaneTextCaptureMode, Rect, SourcePaneGeometry, SourcePaneSnapshot, SplitDirection,
        VisibleViewport,
    };
    use anyhow::anyhow;

    #[derive(Default)]
    struct FakeClient {
        layout: Option<LayoutSnapshot>,
        calls: Vec<String>,
        launch_paths: Option<(std::path::PathBuf, std::path::PathBuf)>,
        fail_focus_pane: bool,
        fail_focus_tab: bool,
        pane_infos: Vec<crate::herdr::client::PaneInfo>,
    }

    impl HerdrClient for FakeClient {
        fn pane_list(&mut self, workspace_id: &str) -> Result<Vec<crate::herdr::client::PaneInfo>> {
            self.calls.push(format!("pane_list:{workspace_id}"));
            Ok(self.pane_infos.clone())
        }

        fn pane_layout(&mut self, _pane: &PaneId) -> Result<LayoutSnapshot> {
            self.calls.push("pane_layout".into());
            self.layout.take().context("missing fake layout")
        }

        fn pane_read_visible(&mut self, pane: &PaneId, lines: u16) -> Result<String> {
            self.calls.push(format!("pane_read:{pane}:{lines}"));
            Ok(format!("https://example.com/{pane}"))
        }

        fn apply_layout(
            &mut self,
            workspace_id: &str,
            _tab_label: &str,
            root: &LaunchLayoutNode,
        ) -> Result<AppliedLayout> {
            self.calls.push(format!("apply:{workspace_id}"));
            self.launch_paths = picker_paths(root);
            Ok(AppliedLayout {
                tab_id: "w1:t2".into(),
                picker_pane_id: PaneId::new("w1:p2"),
            })
        }

        fn focus_pane(&mut self, pane: &PaneId) -> Result<()> {
            self.calls.push(format!("focus_pane:{pane}"));
            let (_, ready) = self.launch_paths.as_ref().context("missing launch paths")?;
            assert!(!ready.exists(), "barrier released before picker focus");
            if self.fail_focus_pane {
                Err(anyhow!("focus failed"))
            } else {
                Ok(())
            }
        }

        fn zoom_pane(&mut self, pane: &PaneId) -> Result<()> {
            self.calls.push(format!("zoom:{pane}"));
            Ok(())
        }

        fn focus_tab(&mut self, tab_id: &str) -> Result<()> {
            self.calls.push(format!("focus_tab:{tab_id}"));
            if self.fail_focus_tab {
                Err(anyhow!("tab focus failed"))
            } else {
                Ok(())
            }
        }

        fn close_tab(&mut self, tab_id: &str) -> Result<()> {
            self.calls.push(format!("close_tab:{tab_id}"));
            Ok(())
        }
    }

    fn picker_paths(node: &LaunchLayoutNode) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        match node {
            LaunchLayoutNode::Pane { command }
                if command.get(1).is_some_and(|argument| argument == "pick") =>
            {
                Some((command.get(3)?.into(), command.get(5)?.into()))
            }
            LaunchLayoutNode::Split { first, second, .. } => {
                picker_paths(first).or_else(|| picker_paths(second))
            }
            _ => None,
        }
    }

    fn source_layout(zoomed: bool) -> LayoutSnapshot {
        LayoutSnapshot {
            area: Rect::new(0, 0, 80, 24),
            focused_pane_id: Some("w1:p1".into()),
            panes: vec![LayoutPane {
                focused: true,
                pane_id: "w1:p1".into(),
                rect: Rect::new(0, 0, 80, 24),
            }],
            splits: Vec::new(),
            tab_id: Some("w1:t1".into()),
            workspace_id: Some("w1".into()),
            zoomed,
        }
    }

    fn two_pane_layout() -> LayoutSnapshot {
        LayoutSnapshot {
            area: Rect::new(0, 0, 80, 24),
            focused_pane_id: Some("w1:p1".into()),
            panes: vec![
                LayoutPane {
                    focused: true,
                    pane_id: "w1:p1".into(),
                    rect: Rect::new(0, 0, 40, 24),
                },
                LayoutPane {
                    focused: false,
                    pane_id: "w1:p2".into(),
                    rect: Rect::new(40, 0, 40, 24),
                },
            ],
            splits: vec![LayoutSplit {
                direction: SplitDirection::Right,
                ratio: 0.5,
                rect: Rect::new(0, 0, 80, 24),
            }],
            tab_id: Some("w1:t1".into()),
            workspace_id: Some("w1".into()),
            zoomed: false,
        }
    }

    fn picker_snapshot(zoom_picker: bool) -> PickerSnapshot {
        PickerSnapshot {
            source: SourcePaneSnapshot {
                target_pane_id: PaneId::new("w1:p1"),
                source_tab_id: "w1:t1".into(),
                workspace_id: "w1".into(),
                source_panes: vec![SourcePaneGeometry {
                    pane_id: PaneId::new("w1:p1"),
                    outer_rect: Rect::new(0, 0, 80, 24),
                    content_rect: Rect::new(0, 0, 80, 24),
                    content_width: 80,
                    content_height: 24,
                    logical_lines: Vec::new(),
                    visible_viewport: Some(VisibleViewport {
                        rows: Vec::new(),
                        logical_lines: Vec::new(),
                        segments: Vec::new(),
                    }),
                    cwd: None,
                }],
                capture_mode: PaneTextCaptureMode::ExactVisibleUnwrapped,
            },
            session: PickerReturnContext {
                return_tab_id: "w1:t1".into(),
                return_pane_id: PaneId::new("w1:p1"),
                zoom_picker,
            },
            action: PickerAction::Copy,
            scope: PickerScope::TargetPane,
            custom_patterns: Vec::new(),
            open: OpenSettings::default(),
        }
    }

    #[test]
    fn conversion_preserves_argv_and_split() {
        let tree = LayoutNode::Split {
            direction: SplitDirection::Right,
            ratio: 0.37,
            first: Box::new(LayoutNode::Pane {
                source_pane_id: PaneId::new("a"),
                rect: Rect::new(0, 0, 1, 1),
            }),
            second: Box::new(LayoutNode::Pane {
                source_pane_id: PaneId::new("b"),
                rect: Rect::new(0, 0, 1, 1),
            }),
            rect: Rect::new(0, 0, 2, 1),
        };
        let converted = convert_layout(
            &tree,
            &PaneId::new("b"),
            Path::new("/a b/π'"),
            Path::new("/s p"),
            Path::new("/r p"),
        );
        let LaunchLayoutNode::Split {
            direction,
            ratio,
            second,
            ..
        } = converted
        else {
            panic!("expected split layout");
        };
        assert_eq!(direction, SplitDirection::Right);
        assert_eq!(ratio, 0.37);
        assert!(matches!(
            second.as_ref(),
            LaunchLayoutNode::Pane { command }
                if command.first().is_some_and(|value| value == "/a b/π'")
                    && command.get(1).is_some_and(|value| value == "pick")
        ));
    }

    #[test]
    fn launch_focuses_picker_before_releasing_barrier() {
        let mut client = FakeClient {
            layout: Some(source_layout(false)),
            ..FakeClient::default()
        };

        launch_layout_tab_picker(
            &mut client,
            &PaneId::new("w1:p1"),
            Path::new("/tmp/herdr pluck"),
            PickerAction::Copy,
            PickerScope::TargetPane,
            Vec::new(),
            OpenSettings::default(),
        )
        .unwrap();

        assert_eq!(
            client.calls,
            [
                "pane_layout",
                "pane_list:w1",
                "pane_read:w1:p1:24",
                "apply:w1",
                "focus_pane:w1:p2"
            ]
        );
        let (snapshot, ready) = client.launch_paths.unwrap();
        assert!(ready.exists());
        let files = PickerLaunchFiles {
            snapshot_path: snapshot,
            marker_temp_path: ready.with_extension("ready.tmp"),
            ready_path: ready,
        };
        files.cleanup().unwrap();
    }

    #[test]
    fn every_pane_is_captured_and_non_target_panes_mirror() {
        let mut client = FakeClient {
            layout: Some(two_pane_layout()),
            ..FakeClient::default()
        };

        launch_layout_tab_picker(
            &mut client,
            &PaneId::new("w1:p1"),
            Path::new("/tmp/herdr-pluck"),
            PickerAction::Copy,
            PickerScope::TargetPane,
            Vec::new(),
            OpenSettings::default(),
        )
        .unwrap();

        assert!(client
            .calls
            .iter()
            .any(|call| call.starts_with("pane_read:w1:p1:")));
        assert!(client
            .calls
            .iter()
            .any(|call| call.starts_with("pane_read:w1:p2:")));

        let (snapshot_path, ready) = client.launch_paths.clone().unwrap();
        let snapshot = crate::herdr::snapshot::read_snapshot_file(&snapshot_path).unwrap();
        let mirrored = snapshot
            .source
            .source_panes
            .iter()
            .find(|pane| pane.pane_id == PaneId::new("w1:p2"))
            .expect("second pane captured");
        assert!(mirrored
            .logical_lines
            .iter()
            .any(|line| line.contains("w1:p2")));

        let files = PickerLaunchFiles {
            snapshot_path,
            marker_temp_path: ready.with_extension("ready.tmp"),
            ready_path: ready,
        };
        files.cleanup().unwrap();
    }

    #[test]
    fn capture_prefers_foreground_cwd_and_falls_back_to_cwd() {
        use crate::herdr::client::PaneInfo;

        let mut client = FakeClient {
            layout: Some(two_pane_layout()),
            pane_infos: vec![
                PaneInfo {
                    pane_id: "w1:p1".into(),
                    cwd: Some("/repo".into()),
                    foreground_cwd: Some("/repo/sub".into()),
                },
                PaneInfo {
                    pane_id: "w1:p2".into(),
                    cwd: Some("/other".into()),
                    foreground_cwd: None,
                },
            ],
            ..FakeClient::default()
        };
        let layout = client.layout.clone().unwrap();

        let panes = capture_panes(&mut client, &layout).unwrap();

        assert_eq!(panes[0].cwd, Some(std::path::PathBuf::from("/repo/sub")));
        assert_eq!(panes[1].cwd, Some(std::path::PathBuf::from("/other")));
    }

    #[test]
    fn conversion_gives_non_target_panes_a_mirror_command() {
        let tree = LayoutNode::Split {
            direction: SplitDirection::Right,
            ratio: 0.5,
            first: Box::new(LayoutNode::Pane {
                source_pane_id: PaneId::new("a"),
                rect: Rect::new(0, 0, 1, 1),
            }),
            second: Box::new(LayoutNode::Pane {
                source_pane_id: PaneId::new("b"),
                rect: Rect::new(0, 0, 1, 1),
            }),
            rect: Rect::new(0, 0, 2, 1),
        };

        let converted = convert_layout(
            &tree,
            &PaneId::new("b"),
            Path::new("/bin/pluck"),
            Path::new("/s"),
            Path::new("/r"),
        );

        let LaunchLayoutNode::Split { first, .. } = converted else {
            panic!("expected split layout");
        };
        assert!(matches!(
            first.as_ref(),
            LaunchLayoutNode::Pane { command }
                if command == &vec![
                    "/bin/pluck".to_string(),
                    "mirror".to_string(),
                    "--snapshot".to_string(),
                    "/s".to_string(),
                    "--pane".to_string(),
                    "a".to_string(),
                    "--ready".to_string(),
                    "/r".to_string(),
                ]
        ));
    }

    #[test]
    fn failed_focus_compensates_with_returned_tab_id_and_preserves_primary_error() {
        let mut client = FakeClient {
            layout: Some(source_layout(false)),
            fail_focus_pane: true,
            fail_focus_tab: true,
            ..FakeClient::default()
        };

        let error = launch_layout_tab_picker(
            &mut client,
            &PaneId::new("w1:p1"),
            Path::new("/tmp/herdr-pluck"),
            PickerAction::Copy,
            PickerScope::TargetPane,
            Vec::new(),
            OpenSettings::default(),
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "focus failed");
        assert!(client.calls.contains(&"focus_tab:w1:t1".into()));
        assert!(client.calls.contains(&"close_tab:w1:t2".into()));
        let (snapshot, ready) = client.launch_paths.unwrap();
        assert!(!snapshot.exists() && !ready.exists());
    }

    #[test]
    fn cleanup_attempts_close_after_focus_failure_and_rejects_source_tab() {
        let session = picker_snapshot(false).session;
        let mut client = FakeClient {
            fail_focus_tab: true,
            ..FakeClient::default()
        };

        let error = cleanup_session(&mut client, &session, "w1:t2").unwrap_err();

        assert_eq!(error.to_string(), "tab focus failed");
        assert_eq!(client.calls, ["focus_tab:w1:t1", "close_tab:w1:t2"]);
        assert!(cleanup_session(&mut client, &session, "w1:t1")
            .unwrap_err()
            .to_string()
            .contains("refusing"));
    }

    #[test]
    fn zooms_only_when_snapshot_requests_it() {
        let mut client = FakeClient::default();
        zoom_picker(&mut client, &picker_snapshot(false), &PaneId::new("w1:p2")).unwrap();
        zoom_picker(&mut client, &picker_snapshot(true), &PaneId::new("w1:p2")).unwrap();
        assert_eq!(client.calls, ["zoom:w1:p2"]);
    }
}
