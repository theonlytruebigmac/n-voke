//! The full-screen output viewer's behaviour: opening it on a group of devices, its keys, and
//! getting the text out (clipboard and file). The logic is in `output_view.rs`; drawing is in
//! `viewer_ui.rs`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use nvoke_core::{task_state::JobTask, vault};
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::App;
use crate::{
    input::TextInput,
    output_view::{self, MAX_COPY_BYTES, OutLine, Viewer, Visual},
};

/// How long a message stays up, in ticks.
pub const TOAST_TICKS: u64 = 40;

impl App {
    /// Opens the viewer on the devices of the selected result group.
    pub(super) fn open_viewer(&mut self) {
        let Some(job) = self.current_job() else { return };
        let Some(group) = self.current_group() else { return };
        let ids: Vec<String> = group.tasks.iter().filter_map(|&i| job.tasks.get(i)).map(|t| t.task_id.clone()).collect();
        if !ids.is_empty() {
            self.viewer = Some(Viewer::new(ids));
        }
    }

    /// The device being read, from the live list (so polling keeps it current).
    pub fn viewer_task(&self) -> Option<&JobTask> {
        let id = self.viewer.as_ref()?.current_task_id()?;
        self.job_list.iter().flat_map(|j| &j.tasks).find(|t| t.task_id == id)
    }

    fn viewer_content(&self) -> Option<(Vec<OutLine>, Vec<Visual>)> {
        let v = self.viewer.as_ref()?;
        let lines = output_view::build(self.viewer_task()?);
        let visuals = output_view::layout(&lines, v.width, v.wrap, v.hscroll);
        Some((lines, visuals))
    }

    fn toast(&mut self, text: String) {
        let tick = self.tick;
        if let Some(v) = self.viewer.as_mut() {
            v.toast = Some((text, tick));
        }
    }

    pub(super) fn on_viewer_key(&mut self, key: KeyEvent) {
        let Some(v) = self.viewer.as_mut() else { return };

        if v.searching {
            match key.code {
                KeyCode::Esc => {
                    v.searching = false;
                    v.search.clear();
                }
                KeyCode::Enter => {
                    v.searching = false;
                    v.match_at = 0;
                    // No matches is shown in the status line, which names what was searched for.
                    if let Some((lines, visuals)) = self.viewer_content()
                        && let Some(v) = self.viewer.as_mut()
                    {
                        v.go_to_match(&lines, &visuals, 0);
                    }
                }
                _ => {
                    let mut input = TextInput::new(&v.search, false);
                    input.handle_key(key);
                    v.search = input.value;
                }
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.viewer = None,
            KeyCode::Char('?') | KeyCode::F(1) => self.show_help = true,
            // Esc backs out one step: first the search, then the viewer.
            KeyCode::Esc => {
                if v.search.is_empty() {
                    self.viewer = None;
                } else {
                    v.search.clear();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => v.scroll_by(1),
            KeyCode::Up | KeyCode::Char('k') => v.scroll_by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => v.page(true),
            KeyCode::PageUp | KeyCode::Char('b') => v.page(false),
            KeyCode::Home | KeyCode::Char('g') => v.go_top(),
            KeyCode::End | KeyCode::Char('G') => v.go_bottom(),
            KeyCode::Right | KeyCode::Char(']') => {
                v.switch_device(1);
            }
            KeyCode::Left | KeyCode::Char('[') => {
                v.switch_device(-1);
            }
            KeyCode::Char(c @ ('<' | '>')) => {
                if v.wrap {
                    // Wrapped lines have nothing off to the side, so say how to get that view.
                    self.toast("Lines are wrapped, so there is nothing to the side. Press w to show them whole.".into());
                } else {
                    v.scroll_across(if c == '>' { 8 } else { -8 });
                }
            }
            KeyCode::Char('w') => v.toggle_wrap(),
            KeyCode::Char('/') => {
                v.searching = true;
                v.search.clear();
            }
            KeyCode::Char(c @ ('n' | 'N')) => {
                if v.search.is_empty() {
                    return;
                }
                let step = if c == 'n' { 1 } else { -1 };
                if let Some((lines, visuals)) = self.viewer_content() {
                    // The first press after a search lands on the match already selected.
                    if let Some(v) = self.viewer.as_mut() {
                        v.go_to_match(&lines, &visuals, step);
                    }
                }
            }
            KeyCode::Char('y') => self.copy_output(),
            KeyCode::Char('e') => self.export_output(),
            _ => {}
        }
    }

    fn output_text(&self) -> Option<(String, String, String)> {
        let task = self.viewer_task()?;
        let script = self.current_job().map(|j| j.script_name.clone()).unwrap_or_default();
        Some((script, task.device_name.clone(), output_view::plain_text(&output_view::build(task))))
    }

    /// Asks the terminal to put the output on the clipboard (OSC 52). Needs a terminal that allows it.
    fn copy_output(&mut self) {
        let Some((_, device, text)) = self.output_text() else { return };
        if text.len() > MAX_COPY_BYTES {
            return self.toast(format!("Too large to copy ({} KB): press e to save it to a file instead.", text.len() / 1024));
        }
        self.outbox.push(output_view::osc52(&text));
        self.toast(format!("Sent {} line{} from {device} to the clipboard (if your terminal allows it).", text.lines().count(), if text.lines().count() == 1 { "" } else { "s" }));
    }

    fn export_output(&mut self) {
        let Some((script, device, text)) = self.output_text() else { return };
        let dir = self.export_dir.clone();
        match export_text_to(&dir, &script, &device, &text, Local::now()) {
            Ok(path) => self.toast(format!("Saved to {}", path.display())),
            Err(e) => self.toast(format!("Could not save: {e}")),
        }
    }
}

/// Where saved output goes: the downloads folder, else the home folder.
pub fn export_dir() -> PathBuf {
    dirs::download_dir().or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir)
}

/// "BitlockerPreChecker" -> "BitlockerPreChecker"; "HP Laptop - Vienna" -> "HP_Laptop_-_Vienna".
fn file_part(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    let mut out = String::new();
    for c in cleaned.chars() {
        if !(c == '_' && out.ends_with('_')) {
            out.push(c);
        }
    }
    out.trim_matches('_').chars().take(40).collect()
}

/// Saves `text` as a file in `dir`, readable only by the current user (output can hold secrets),
/// without overwriting anything. Returns the path.
pub fn export_text_to(dir: &Path, script: &str, device: &str, text: &str, now: DateTime<Local>) -> Result<PathBuf, String> {
    let base = format!("nvoke-{}-{}-{}", file_part(script), file_part(device), now.format("%Y%m%d-%H%M%S"));
    let mut path = dir.join(format!("{base}.txt"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base}-{n}.txt"));
        n += 1;
    }
    vault::write_private(&path, text.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
pub(crate) mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 17, 13, 5, 9).unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nvoke-viewer-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn file_names_are_safe_and_readable() {
        assert_eq!(file_part("HP Laptop - Vienna"), "HP_Laptop_-_Vienna");
        assert_eq!(file_part("../../etc/passwd"), ".._.._etc_passwd");
        assert!(!file_part("a/b\\c:d").contains(['/', '\\', ':']));
        assert_eq!(file_part("  "), "");
        assert_eq!(file_part("a   b"), "a_b", "runs of separators collapse");
        assert!(file_part(&"x".repeat(200)).len() <= 40);
    }

    #[test]
    fn exporting_writes_the_text_to_a_named_file_only_the_user_can_read() {
        let dir = scratch("export");
        let path = export_text_to(&dir, "BitlockerPreChecker", "HP Laptop - Vienna", "line one\nline two\n", at()).unwrap();
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "nvoke-BitlockerPreChecker-HP_Laptop_-_Vienna-20261017-130509.txt");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "line one\nline two\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "output can hold secrets");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn exporting_twice_in_the_same_second_never_overwrites() {
        let dir = scratch("twice");
        let first = export_text_to(&dir, "S", "D", "one", at()).unwrap();
        let second = export_text_to(&dir, "S", "D", "two", at()).unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "one");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "two");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn exporting_into_a_folder_that_does_not_exist_yet_creates_it() {
        let dir = scratch("missing").join("a").join("b");
        assert!(export_text_to(&dir, "S", "D", "x", at()).is_ok());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    }

    // ---- the viewer in use ----------------------------------------------------------

    use nvoke_core::{
        jobs::Job,
        model::{Execution, ExecutionOutput},
        task_state::JobTask,
    };
    use ratatui::{Terminal, backend::TestBackend};

    use super::super::{Focus, Page, Pane, run::tests::press};

    fn finished(device: &str, out: &str, err: Option<&str>) -> JobTask {
        let mut t = JobTask::new(&format!("t-{device}"), Some(device), device, 0);
        t.watching = false;
        t.task_status = Some("FAILED".into());
        t.execution = Some(Execution {
            id: "e".into(),
            status: "FAILED".into(),
            exit_code: None,
            error_message: err.map(String::from),
            started_at: None,
            updated_at: None,
            duration_milliseconds: Some(900.0),
            output: Some(ExecutionOutput { run_as: None, std_output: Some(out.into()), std_error: None, outputs: None }),
        });
        t
    }

    fn lines(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}: {}", if i % 10 == 0 { "TPM not ready" } else { "ok" })).collect::<Vec<_>>().join("\n")
    }

    /// Three devices with identical output (one group) and a long report, with the results pane focused.
    pub(crate) fn viewer_app() -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = super::super::run::tests::run_app();
        app.export_dir = scratch("app-export");
        let tasks = vec![finished("PC-1", &lines(100), Some("exit status 1")), finished("PC-2", &lines(100), Some("exit status 1")), finished("PC-3", &lines(100), Some("exit status 1"))];
        app.jobs.seed(vec![Job {
            id: "j".into(),
            name: "run".into(),
            script_id: None,
            script: None,
            script_name: "BitlockerPreChecker".into(),
            org_id: None,
            org_name: None,
            target_label: "PatchGroupTwo".into(),
            created_at: "2026-10-17T12:00:00.000Z".into(),
            timeout_seconds: 0,
            warnings: None,
            migration_project: None,
            tasks,
        }]);
        app.go(Page::Activity);
        app.focus = Focus::Content;
        app.sync_jobs();
        app.pane = Pane::Results;
        (rt, app)
    }

    /// Gives every device the same new output (so they stay one result group).
    fn set_all_output(app: &mut App, text: &str) {
        let mut job = app.job_list[0].clone();
        for t in &mut job.tasks {
            t.execution.as_mut().unwrap().output.as_mut().unwrap().std_output = Some(text.into());
        }
        app.jobs.seed(vec![job]);
        app.sync_jobs();
    }

    /// Draws once, as the real loop does before every key, so the viewer knows its sizes.
    pub(crate) fn draw(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let b = terminal.backend().buffer();
        (0..b.area.height).map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    pub(crate) fn open(app: &mut App) {
        press(app, KeyCode::Enter);
        draw(app, 100, 30);
    }

    #[test]
    fn enter_on_a_result_group_opens_the_viewer_on_its_devices_but_enter_on_the_run_list_does_not() {
        let (_rt, mut app) = viewer_app();
        app.pane = Pane::Jobs;
        press(&mut app, KeyCode::Enter);
        assert!(app.viewer.is_none() && app.pane == Pane::Results, "enter on the run list steps into its results");
        press(&mut app, KeyCode::Enter);
        let viewer = app.viewer.as_ref().expect("opened");
        assert_eq!(viewer.task_ids, ["t-PC-1", "t-PC-2", "t-PC-3"], "all the devices in the group, in order");
    }

    #[test]
    fn nothing_opens_when_there_is_nothing_to_read() {
        let (_rt, mut app) = viewer_app();
        app.jobs.seed(vec![]);
        app.sync_jobs();
        press(&mut app, KeyCode::Enter);
        assert!(app.viewer.is_none());
    }

    #[test]
    fn the_viewer_owns_the_keyboard_so_nothing_leaks_to_the_app_behind_it() {
        let (_rt, mut app) = viewer_app();
        open(&mut app);
        for c in ['1', '2', 'd', 'x', 'R', 's'] {
            press(&mut app, KeyCode::Char(c));
        }
        assert!(app.page == Page::Activity && !app.show_help && app.confirm.is_none() && !app.quit);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.viewer.is_none() && !app.quit, "q closes the viewer; it must not quit the app");
        assert!(app.page == Page::Activity && app.pane == Pane::Results, "and you are back where you were");
    }

    #[test]
    fn arrows_and_pages_scroll_and_stop_at_the_ends() {
        let (_rt, mut app) = viewer_app();
        open(&mut app);
        let height = app.viewer.as_ref().unwrap().height;
        let total = app.viewer.as_ref().unwrap().total;
        assert!(total > height, "the report is longer than the window");
        press(&mut app, KeyCode::Up);
        assert_eq!(app.viewer.as_ref().unwrap().top, 0);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.viewer.as_ref().unwrap().top, 1);
        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.viewer.as_ref().unwrap().top, 1 + height - 2);
        press(&mut app, KeyCode::End);
        assert_eq!(app.viewer.as_ref().unwrap().top, total - height);
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("line 100:"), "the last line is on screen at the end:\n{screen}");
        assert!(screen.contains("100%"), "{screen}");
        press(&mut app, KeyCode::Home);
        assert_eq!(app.viewer.as_ref().unwrap().top, 0);
    }

    #[test]
    fn left_and_right_move_between_the_devices_of_the_group() {
        let (_rt, mut app) = viewer_app();
        open(&mut app);
        assert!(draw(&mut app, 100, 30).contains("device 1 of 3") && draw(&mut app, 100, 30).contains("PC-1"));
        press(&mut app, KeyCode::PageDown);
        press(&mut app, KeyCode::Right);
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("device 2 of 3") && screen.contains("PC-2"), "{screen}");
        assert_eq!(app.viewer.as_ref().unwrap().top, 0, "a new device starts at its top");
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        assert!(draw(&mut app, 100, 30).contains("device 3 of 3"), "stops at the last");
        press(&mut app, KeyCode::Left);
        assert!(draw(&mut app, 100, 30).contains("device 2 of 3"));
    }

    #[test]
    fn a_single_device_has_no_device_switching() {
        let (_rt, mut app) = viewer_app();
        let mut only = app.job_list[0].clone();
        only.tasks.truncate(1);
        app.jobs.seed(vec![only]);
        app.sync_jobs();
        open(&mut app);
        let screen = draw(&mut app, 100, 30);
        assert!(!screen.contains("device 1 of") && !screen.contains("←→ device"), "{screen}");
    }

    #[test]
    fn searching_finds_matches_jumps_between_them_and_esc_backs_out_in_steps() {
        let (_rt, mut app) = viewer_app();
        open(&mut app);
        press(&mut app, KeyCode::Char('/'));
        assert!(app.viewer.as_ref().unwrap().searching);
        for c in "tpm".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert!(draw(&mut app, 100, 30).contains("/tpm"), "the search box shows what is typed");
        press(&mut app, KeyCode::Enter);
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("Match 1 of 10 for “tpm”"), "{screen}");
        press(&mut app, KeyCode::Char('n'));
        assert!(draw(&mut app, 100, 30).contains("Match 2 of 10"));
        press(&mut app, KeyCode::Char('N'));
        press(&mut app, KeyCode::Char('N'));
        assert!(draw(&mut app, 100, 30).contains("Match 10 of 10"), "wraps backwards past the first");
        // The match is on screen after jumping.
        let top = app.viewer.as_ref().unwrap().top;
        assert!(top > 0 && draw(&mut app, 100, 30).contains("line 100: TPM not ready"));
        // Esc clears the search first; a second Esc closes.
        press(&mut app, KeyCode::Esc);
        assert!(app.viewer.is_some() && app.viewer.as_ref().unwrap().search.is_empty());
        press(&mut app, KeyCode::Esc);
        assert!(app.viewer.is_none());
    }

    #[test]
    fn a_search_with_no_matches_says_so_and_typing_keys_do_not_trigger_shortcuts() {
        let (_rt, mut app) = viewer_app();
        open(&mut app);
        press(&mut app, KeyCode::Char('/'));
        for c in "wyqne".chars() {
            press(&mut app, KeyCode::Char(c)); // w, y, q, n, e are all viewer commands when not typing
        }
        assert!(app.viewer.is_some() && app.viewer.as_ref().unwrap().wrap && app.outbox.is_empty() && !app.quit);
        press(&mut app, KeyCode::Enter);
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("No matches for “wyqne”"), "{screen}");
    }

    #[test]
    fn cancelling_a_search_while_typing_leaves_the_view_as_it_was() {
        let (_rt, mut app) = viewer_app();
        open(&mut app);
        press(&mut app, KeyCode::PageDown);
        let top = app.viewer.as_ref().unwrap().top;
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Esc);
        let v = app.viewer.as_ref().unwrap();
        assert!(!v.searching && v.search.is_empty() && v.top == top);
    }

    #[test]
    fn wrap_can_be_switched_off_and_then_the_view_scrolls_sideways() {
        let (_rt, mut app) = viewer_app();
        set_all_output(&mut app, &"x".repeat(300));
        open(&mut app);
        press(&mut app, KeyCode::Char('>'));
        assert_eq!(app.viewer.as_ref().unwrap().hscroll, 0, "nothing to scroll sideways while lines wrap");
        assert!(draw(&mut app, 100, 30).contains("Press w to show them whole"), "and it says how to get that view");
        press(&mut app, KeyCode::Char('w'));
        assert!(!app.viewer.as_ref().unwrap().wrap);
        assert!(draw(&mut app, 100, 30).contains("wrap off"));
        press(&mut app, KeyCode::Char('>'));
        press(&mut app, KeyCode::Char('>'));
        assert_eq!(app.viewer.as_ref().unwrap().hscroll, 16);
        press(&mut app, KeyCode::Char('<'));
        assert_eq!(app.viewer.as_ref().unwrap().hscroll, 8);
        press(&mut app, KeyCode::Char('w'));
        assert!(app.viewer.as_ref().unwrap().wrap && app.viewer.as_ref().unwrap().hscroll == 0);
    }

    #[test]
    fn copying_sends_the_whole_result_to_the_clipboard_through_the_terminal() {
        use base64::{Engine, engine::general_purpose::STANDARD as B64};
        let (_rt, mut app) = viewer_app();
        open(&mut app);
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.outbox.len(), 1);
        let sequence = &app.outbox[0];
        assert!(sequence.starts_with("\x1b]52;c;") && sequence.ends_with('\x07'));
        let payload = &sequence["\x1b]52;c;".len()..sequence.len() - 1];
        let copied = String::from_utf8(B64.decode(payload).unwrap()).unwrap();
        assert!(copied.contains("line 1: ok") && copied.contains("line 100: TPM not ready") && copied.contains("== N-able says ==\nexit status 1"), "{copied}");
        assert!(draw(&mut app, 100, 30).contains("to the clipboard"), "it tells you what it did");
    }

    #[test]
    fn output_too_large_to_copy_says_so_and_points_to_saving_it() {
        let (_rt, mut app) = viewer_app();
        set_all_output(&mut app, &"y".repeat(80_000));
        open(&mut app);
        press(&mut app, KeyCode::Char('y'));
        assert!(app.outbox.is_empty(), "nothing is sent that a terminal would likely drop");
        assert!(draw(&mut app, 100, 30).contains("Too large to copy"));
    }

    #[test]
    fn saving_writes_the_result_to_a_file_and_shows_where() {
        let (_rt, mut app) = viewer_app();
        open(&mut app);
        press(&mut app, KeyCode::Char('e'));
        let files: Vec<_> = std::fs::read_dir(&app.export_dir).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);
        let name = files[0].file_name().to_string_lossy().to_string();
        assert!(name.starts_with("nvoke-BitlockerPreChecker-PC-1-") && name.ends_with(".txt"), "{name}");
        let text = std::fs::read_to_string(files[0].path()).unwrap();
        assert!(text.contains("line 49: ok") && text.contains("line 50: TPM not ready") && text.contains("== Output =="));
        assert!(draw(&mut app, 120, 30).contains("Saved to"), "the path is shown");
        let _ = std::fs::remove_dir_all(&app.export_dir);
    }

    #[test]
    fn a_message_goes_away_after_a_few_seconds() {
        let (_rt, mut app) = viewer_app();
        open(&mut app);
        press(&mut app, KeyCode::Char('y'));
        assert!(draw(&mut app, 100, 30).contains("clipboard"));
        app.tick += TOAST_TICKS + 1;
        assert!(!draw(&mut app, 100, 30).contains("clipboard"));
    }

    #[test]
    fn the_viewer_follows_a_run_that_is_still_updating() {
        let (_rt, mut app) = viewer_app();
        open(&mut app);
        assert!(!draw(&mut app, 100, 30).contains("brand new line"));
        set_all_output(&mut app, "brand new line");
        assert!(draw(&mut app, 100, 30).contains("brand new line"), "polling updates what is being read");
    }

    #[test]
    fn if_the_run_disappears_the_viewer_says_so_and_can_still_be_closed() {
        let (_rt, mut app) = viewer_app();
        open(&mut app);
        app.jobs.seed(vec![]);
        app.sync_jobs();
        assert!(draw(&mut app, 100, 30).contains("no longer in the list"));
        press(&mut app, KeyCode::Esc);
        assert!(app.viewer.is_none());
    }

    #[test]
    fn the_viewer_survives_tiny_windows() {
        let (_rt, mut app) = viewer_app();
        open(&mut app);
        for (w, h) in [(100, 30), (60, 12), (30, 8), (20, 5), (10, 3), (4, 2)] {
            draw(&mut app, w, h);
            press(&mut app, KeyCode::End);
            press(&mut app, KeyCode::PageDown);
            press(&mut app, KeyCode::Char('>'));
        }
    }
}
