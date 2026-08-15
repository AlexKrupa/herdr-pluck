use crate::model::{OpenSettings, PickerSnapshot};
use crate::url_opener::{SystemUrlOpener, UrlOpener};
use anyhow::{anyhow, bail, Context, Result};
use std::io::Write;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The picker waits for the open command - a handler that never exits would block the overlay.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

pub fn open_selection(snapshot: &PickerSnapshot, text: &str) -> Result<()> {
    open_selection_with(&SystemUrlOpener, snapshot, text)
}

fn open_selection_with(
    url_opener: &impl UrlOpener,
    snapshot: &PickerSnapshot,
    text: &str,
) -> Result<()> {
    if snapshot.open.command.is_empty() {
        return url_opener
            .open(text)
            .map(|_| ())
            .with_context(|| format!("failed to open selected match {text:?}"));
    }

    let matched = crate::picker::pane_for_text(snapshot, text);
    let settings = OpenSettings {
        command: snapshot.open.command.clone(),
        cwd: matched
            .and_then(|pane| pane.cwd.clone())
            .or_else(|| snapshot.open.cwd.clone()),
    };
    let pane_id = matched
        .map(|pane| pane.pane_id.0.clone())
        .unwrap_or_else(|| snapshot.source.target_pane_id.0.clone());

    run_open_command(&settings, text, &pane_id, &snapshot.session.return_tab_id)
}

/// Spawns the configured command with the match on stdin and the source pane in the environment.
fn run_open_command(
    settings: &OpenSettings,
    text: &str,
    pane_id: &str,
    tab_id: &str,
) -> Result<()> {
    let (program, args) = settings
        .command
        .split_first()
        .ok_or_else(|| anyhow!("open command is empty"))?;

    let mut command = Command::new(program);
    command
        .args(args)
        .env("HERDR_PLUCK_PANE_ID", pane_id)
        .env("HERDR_PLUCK_TAB_ID", tab_id)
        .stdin(Stdio::piped());
    if let Some(cwd) = &settings.cwd {
        command.current_dir(cwd);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn open command {program:?}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("open command stdin unavailable"))?
        .write_all(text.as_bytes())
        .context("failed to write the match to the open command")?;

    // A timed-out handler is killed without a message - the picker tab is mid-teardown.
    let Some(status) = wait_with_timeout(&mut child, OPEN_TIMEOUT)? else {
        return Ok(());
    };
    if !status.success() {
        bail!("open command {program:?} failed: {status}");
    }
    Ok(())
}

/// Returns `None` after killing the child on timeout.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Result<Option<ExitStatus>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .context("failed to wait for the open command")?
        {
            return Ok(Some(status));
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        OpenSettings, PaneId, PaneTextCaptureMode, PickerAction, PickerReturnContext, PickerScope,
        Rect, SourcePaneGeometry, SourcePaneSnapshot,
    };
    use crate::url_opener::{OpenUrlSuccess, UrlOpenError};
    use std::cell::RefCell;
    use std::path::PathBuf;

    #[derive(Default)]
    struct FakeUrlOpener {
        opened: RefCell<Vec<String>>,
    }

    impl UrlOpener for FakeUrlOpener {
        fn open(&self, url: &str) -> Result<OpenUrlSuccess, UrlOpenError> {
            self.opened.borrow_mut().push(url.to_string());
            Ok(OpenUrlSuccess {
                tool: "fake".to_string(),
            })
        }
    }

    fn snapshot(open: OpenSettings) -> PickerSnapshot {
        PickerSnapshot {
            source: SourcePaneSnapshot {
                target_pane_id: PaneId::new("w1:p1"),
                source_tab_id: "w1:t1".to_string(),
                workspace_id: "w1".to_string(),
                source_panes: vec![SourcePaneGeometry {
                    pane_id: PaneId::new("w1:p1"),
                    outer_rect: Rect::new(0, 0, 40, 1),
                    content_rect: Rect::new(0, 0, 40, 1),
                    content_width: 40,
                    content_height: 1,
                    logical_lines: Vec::new(),
                    visible_viewport: None,
                    cwd: None,
                }],
                capture_mode: PaneTextCaptureMode::ExactVisibleUnwrapped,
            },
            session: PickerReturnContext {
                return_tab_id: "w1:t1".to_string(),
                return_pane_id: PaneId::new("w1:p1"),
                zoom_picker: false,
            },
            action: PickerAction::Copy,
            scope: PickerScope::TargetPane,
            custom_patterns: Vec::new(),
            open,
        }
    }

    fn out_path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("pluck-open-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pluck-open-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn open_runs_in_the_matched_pane_directory() {
        let out = out_path("matched-cwd");
        let left = temp_dir("left");
        let right = temp_dir("right");
        let mut snapshot = snapshot(OpenSettings {
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("pwd > {}", out.display()),
            ],
            cwd: Some(temp_dir("fallback")),
        });
        snapshot.scope = PickerScope::AllPanes;
        snapshot.source.source_panes[0].logical_lines =
            vec!["left https://example.com/a".to_string()];
        snapshot.source.source_panes[0].cwd = Some(left.clone());
        snapshot.source.source_panes.push(SourcePaneGeometry {
            pane_id: PaneId::new("p2"),
            outer_rect: Rect::new(40, 0, 40, 1),
            content_rect: Rect::new(40, 0, 40, 1),
            content_width: 40,
            content_height: 1,
            logical_lines: vec!["right https://example.com/b".to_string()],
            visible_viewport: None,
            cwd: Some(right.clone()),
        });

        open_selection_with(
            &FakeUrlOpener::default(),
            &snapshot,
            "https://example.com/b",
        )
        .unwrap();

        let recorded = std::fs::read_to_string(&out).unwrap();
        assert_eq!(
            recorded.trim(),
            right.canonicalize().unwrap().to_str().unwrap()
        );
        assert_ne!(
            recorded.trim(),
            left.canonicalize().unwrap().to_str().unwrap()
        );
    }

    #[test]
    fn open_falls_back_to_the_configured_cwd_when_no_pane_matches() {
        let out = out_path("fallback-cwd");
        let fallback = temp_dir("fallback-only");
        let mut snapshot = snapshot(OpenSettings {
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("pwd > {}", out.display()),
            ],
            cwd: Some(fallback.clone()),
        });
        snapshot.source.source_panes[0].cwd = Some(temp_dir("ignored"));

        open_selection_with(
            &FakeUrlOpener::default(),
            &snapshot,
            "https://example.com/absent",
        )
        .unwrap();

        let recorded = std::fs::read_to_string(&out).unwrap();
        assert_eq!(
            recorded.trim(),
            fallback.canonicalize().unwrap().to_str().unwrap()
        );
    }

    #[test]
    fn no_configured_command_falls_back_to_the_system_opener() {
        let opener = FakeUrlOpener::default();

        open_selection_with(
            &opener,
            &snapshot(OpenSettings::default()),
            "https://example.com",
        )
        .unwrap();

        assert_eq!(*opener.opened.borrow(), vec!["https://example.com"]);
    }

    #[test]
    fn a_configured_command_bypasses_the_system_opener() {
        let out = out_path("dispatch");
        let opener = FakeUrlOpener::default();
        let open = OpenSettings {
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("cat > {}", out.display()),
            ],
            cwd: None,
        };

        open_selection_with(&opener, &snapshot(open), "src/main.rs").unwrap();

        assert_eq!(std::fs::read_to_string(&out).unwrap(), "src/main.rs");
        assert!(opener.opened.borrow().is_empty());
    }

    #[test]
    fn passes_match_on_stdin_and_pane_id_in_env() {
        let out = out_path("stdin");
        let settings = OpenSettings {
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                format!(
                    "{{ cat; printf '\\n%s\\n%s\\n' \"$HERDR_PLUCK_PANE_ID\" \"$HERDR_PLUCK_TAB_ID\"; }} > {}",
                    out.display()
                ),
            ],
            cwd: None,
        };

        run_open_command(&settings, "src/main.rs", "w1:p3", "w1:t2").unwrap();

        let contents = std::fs::read_to_string(&out).unwrap();
        assert_eq!(contents, "src/main.rs\nw1:p3\nw1:t2\n");
    }

    #[test]
    fn runs_in_the_configured_cwd() {
        let out = out_path("cwd");
        let dir = std::env::temp_dir().join(format!("pluck-open-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("marker.txt"), "found").unwrap();
        let settings = OpenSettings {
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("cat \"$(cat)\" > {}", out.display()),
            ],
            cwd: Some(dir.clone()),
        };

        run_open_command(&settings, "marker.txt", "w1:p1", "w1:t1").unwrap();

        assert_eq!(std::fs::read_to_string(&out).unwrap(), "found");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hanging_command_is_killed_and_not_reported() {
        let mut child = Command::new("sh").args(["-c", "sleep 30"]).spawn().unwrap();
        let started = Instant::now();

        let status = wait_with_timeout(&mut child, Duration::from_millis(200)).unwrap();

        assert!(status.is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn failing_command_is_reported() {
        let settings = OpenSettings {
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                "cat > /dev/null; exit 3".to_string(),
            ],
            cwd: None,
        };

        let error = run_open_command(&settings, "x", "w1:p1", "w1:t1").unwrap_err();

        assert!(error.to_string().contains("open command"));
    }
}
