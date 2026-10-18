//! Reading one device's result at length: what to show (status, output, errors, N-able's
//! message), how to lay it out for a given width, searching it, and where the view is scrolled to.
//! Pure logic; `viewer_ui.rs` draws it and the app wires the keys.

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use nvoke_core::task_state::{JobTask, TaskPhase, task_outcome, task_phase, task_status_word};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Heading,
    Meta,
    Stdout,
    Stderr,
    Message,
    Variable,
    Note,
}

/// One logical line of the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutLine {
    pub kind: Kind,
    /// For metadata and variables: the label shown before the text.
    pub label: Option<String>,
    pub text: String,
    /// Line number within its section (output and errors only).
    pub number: Option<usize>,
}

impl OutLine {
    fn new(kind: Kind, text: &str) -> Self {
        Self { kind, label: None, text: text.to_string(), number: None }
    }

    fn labelled(kind: Kind, label: &str, text: &str) -> Self {
        Self { kind, label: Some(label.to_string()), text: text.to_string(), number: None }
    }
}

/// Normalizes line endings and tabs so lines have a predictable width.
fn clean(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ")
}

/// The lines of a block of text, without the empty line a trailing newline would add.
fn text_lines(text: &str) -> Vec<String> {
    let cleaned = clean(text);
    let mut lines: Vec<String> = cleaned.split('\n').map(String::from).collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) && lines.len() > 1 {
        lines.pop();
    }
    lines
}

fn output_value(v: &nvoke_core::model::InputValue) -> String {
    if let Some(s) = &v.string_value {
        s.clone()
    } else if let Some(b) = v.boolean_value {
        b.to_string()
    } else if let Some(i) = v.int_value {
        i.to_string()
    } else if let Some(f) = v.float_value {
        f.to_string()
    } else {
        v.date_time_value.clone().unwrap_or_default()
    }
}

/// Everything worth reading about one device's run, top to bottom.
pub fn build(task: &JobTask) -> Vec<OutLine> {
    let mut out = vec![];
    let exec = task.execution.as_ref();
    let outcome = task_outcome(task);

    out.push(OutLine::labelled(Kind::Meta, "Status", &task_status_word(task)));
    if let Some(code) = outcome.as_ref().and_then(|o| o.exit_code) {
        out.push(OutLine::labelled(Kind::Meta, "Exit code", &code.to_string()));
    }
    if let Some(ms) = exec.and_then(|e| e.duration_milliseconds) {
        out.push(OutLine::labelled(Kind::Meta, "Duration", &format!("{:.1}s", ms / 1000.0)));
    }
    if let Some(at) = exec.and_then(|e| e.started_at.as_deref()) {
        out.push(OutLine::labelled(Kind::Meta, "Started", at));
    }
    if let Some(run_as) = exec.and_then(|e| e.output.as_ref()).and_then(|o| o.run_as.as_deref()).filter(|r| !r.is_empty()) {
        out.push(OutLine::labelled(Kind::Meta, "Run as", run_as));
    }
    out.push(OutLine::labelled(Kind::Meta, "Task ID", &task.task_id));

    let output = exec.and_then(|e| e.output.as_ref());
    let variables: Vec<_> = output.and_then(|o| o.outputs.as_ref()).map(|v| v.iter().collect()).unwrap_or_default();
    if !variables.is_empty() {
        out.push(OutLine::new(Kind::Heading, "Output variables"));
        for v in variables {
            let label = if v.name.is_empty() { &v.variable } else { &v.name };
            out.push(OutLine::labelled(Kind::Variable, label, &v.value.as_ref().map(output_value).unwrap_or_default()));
        }
    }

    let section = |out: &mut Vec<OutLine>, title: &str, kind: Kind, text: Option<&str>, numbered: bool| {
        let Some(text) = text.filter(|t| !t.trim().is_empty()) else { return };
        out.push(OutLine::new(Kind::Heading, title));
        for (i, line) in text_lines(text).into_iter().enumerate() {
            out.push(OutLine { kind, label: None, text: line, number: numbered.then_some(i + 1) });
        }
    };
    section(&mut out, "Output", Kind::Stdout, output.and_then(|o| o.std_output.as_deref()), true);
    section(&mut out, "Errors", Kind::Stderr, output.and_then(|o| o.std_error.as_deref()), true);
    section(&mut out, "N-able says", Kind::Message, exec.and_then(|e| e.error_message.as_deref()), false);

    let has_text = out.iter().any(|l| matches!(l.kind, Kind::Stdout | Kind::Stderr | Kind::Message));
    match task_phase(task) {
        TaskPhase::Queued | TaskPhase::Running => out.push(OutLine::new(Kind::Note, "Waiting for results…")),
        _ if !has_text => out.push(OutLine::new(Kind::Note, "No output.")),
        _ => {}
    }
    out
}

/// The whole result as plain text, for copying or saving.
pub fn plain_text(lines: &[OutLine]) -> String {
    let mut text = String::new();
    for line in lines {
        match (&line.kind, &line.label) {
            (Kind::Heading, _) => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&format!("== {} ==", line.text));
            }
            (_, Some(label)) => text.push_str(&format!("{label}: {}", line.text)),
            _ => text.push_str(&line.text),
        }
        text.push('\n');
    }
    text
}

/// One row on screen: a logical line, or a piece of one that has been wrapped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visual {
    /// Index of the logical line this belongs to.
    pub line: usize,
    /// First piece of its line (the one that carries the line number).
    pub first: bool,
    pub text: String,
}

/// Splits text into pieces no wider than `width` display columns.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if text.width() <= width {
        return vec![text.to_string()];
    }
    let (mut pieces, mut current, mut used) = (vec![], String::new(), 0);
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width && !current.is_empty() {
            pieces.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(c);
        used += w;
    }
    pieces.push(current);
    pieces
}

/// The part of `text` between display columns `from` and `from + width`.
pub fn slice_columns(text: &str, from: usize, width: usize) -> String {
    let (mut out, mut col) = (String::new(), 0);
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if col >= from && col + w <= from + width {
            out.push(c);
        }
        col += w;
        if col >= from + width {
            break;
        }
    }
    out
}

/// Lays the lines out for a text area `width` columns wide. Without wrapping, each line is one row
/// cut to the window that starts at column `hscroll`.
pub fn layout(lines: &[OutLine], width: usize, wrap: bool, hscroll: usize) -> Vec<Visual> {
    let mut out = vec![];
    for (i, line) in lines.iter().enumerate() {
        let text = match &line.label {
            Some(label) => format!("{label:<10} {}", line.text),
            None => line.text.clone(),
        };
        if wrap {
            for (k, piece) in wrap_text(&text, width).into_iter().enumerate() {
                out.push(Visual { line: i, first: k == 0, text: piece });
            }
        } else {
            out.push(Visual { line: i, first: true, text: slice_columns(&text, hscroll, width) });
        }
    }
    out
}

/// The widest logical line, in display columns (for the horizontal scroll limit).
pub fn widest(lines: &[OutLine]) -> usize {
    lines.iter().map(|l| l.text.width() + l.label.as_ref().map_or(0, |_| 11)).max().unwrap_or(0)
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Character ranges in `text` that match `term`, ignoring case.
pub fn match_ranges(text: &str, term: &str) -> Vec<(usize, usize)> {
    let needle: Vec<char> = term.chars().map(fold).collect();
    if needle.is_empty() {
        return vec![];
    }
    let hay: Vec<char> = text.chars().map(fold).collect();
    let mut out = vec![];
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == needle[..] {
            out.push((i, i + needle.len()));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// Indexes of the logical lines that contain `term`.
pub fn find(lines: &[OutLine], term: &str) -> Vec<usize> {
    lines.iter().enumerate().filter(|(_, l)| !match_ranges(&l.text, term).is_empty() || l.label.as_deref().is_some_and(|b| !match_ranges(b, term).is_empty())).map(|(i, _)| i).collect()
}

/// The OSC 52 escape sequence that asks the terminal to put `text` on the clipboard.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", B64.encode(text.as_bytes()))
}

/// Most text copied at once: some terminals drop longer clipboard requests.
pub const MAX_COPY_BYTES: usize = 70_000;

/// Where the view is scrolled to, and the reader's settings.
#[derive(Clone, Debug)]
pub struct Viewer {
    /// The devices in the group being read, by task id.
    pub task_ids: Vec<String>,
    pub device: usize,
    /// First visible row.
    pub top: usize,
    pub wrap: bool,
    pub hscroll: usize,
    pub search: String,
    /// The search box is being typed into.
    pub searching: bool,
    pub match_at: usize,
    pub toast: Option<(String, u64)>,
    /// Set by the drawing code each frame, so the keys know the sizes.
    pub total: usize,
    pub height: usize,
    pub widest: usize,
    pub width: usize,
}

impl Viewer {
    pub fn new(task_ids: Vec<String>) -> Self {
        Self { task_ids, device: 0, top: 0, wrap: true, hscroll: 0, search: String::new(), searching: false, match_at: 0, toast: None, total: 0, height: 1, widest: 0, width: 1 }
    }

    pub fn max_top(&self) -> usize {
        self.total.saturating_sub(self.height)
    }

    pub fn clamp(&mut self) {
        self.top = self.top.min(self.max_top());
        self.hscroll = self.hscroll.min(self.widest.saturating_sub(self.width));
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.top = (self.top as isize + delta).clamp(0, self.max_top() as isize) as usize;
    }

    /// A page is the visible rows less two, so the last lines of the old page stay in view.
    pub fn page(&mut self, down: bool) {
        let step = self.height.saturating_sub(2).max(1) as isize;
        self.scroll_by(if down { step } else { -step });
    }

    pub fn go_top(&mut self) {
        self.top = 0;
    }

    pub fn go_bottom(&mut self) {
        self.top = self.max_top();
    }

    pub fn scroll_across(&mut self, delta: isize) {
        self.hscroll = (self.hscroll as isize + delta).clamp(0, self.widest.saturating_sub(self.width) as isize) as usize;
    }

    /// Switches to the previous or next device in the group. Returns whether it moved.
    pub fn switch_device(&mut self, delta: isize) -> bool {
        let next = (self.device as isize + delta).clamp(0, self.task_ids.len() as isize - 1) as usize;
        let moved = next != self.device;
        if moved {
            self.device = next;
            self.top = 0;
            self.hscroll = 0;
            self.match_at = 0;
        }
        moved
    }

    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        self.hscroll = 0;
        self.top = 0;
    }

    pub fn current_task_id(&self) -> Option<&str> {
        self.task_ids.get(self.device).map(String::as_str)
    }

    /// Scrolls so the `n`th match sits a couple of rows below the top. `visuals` is the current layout.
    pub fn go_to_match(&mut self, lines: &[OutLine], visuals: &[Visual], n: isize) -> Option<(usize, usize)> {
        let hits = find(lines, &self.search);
        if hits.is_empty() {
            return None;
        }
        self.match_at = (self.match_at as isize + n).rem_euclid(hits.len() as isize) as usize;
        let row = visuals.iter().position(|v| v.line == hits[self.match_at]).unwrap_or(0);
        self.top = row.saturating_sub(2).min(self.max_top());
        Some((self.match_at + 1, hits.len()))
    }
}

#[cfg(test)]
mod tests {
    use nvoke_core::model::{Execution, ExecutionOutput, InputValue, OutputValue};

    use super::*;

    fn task(stdout: &str, stderr: Option<&str>, error: Option<&str>) -> JobTask {
        let mut t = JobTask::new("task-1", Some("a1"), "PC-1", 0);
        t.watching = false;
        t.task_status = Some("FAILED".into());
        t.execution = Some(Execution {
            id: "e".into(),
            status: "FAILED".into(),
            exit_code: None,
            error_message: error.map(String::from),
            started_at: Some("2026-10-17T12:00:05Z".into()),
            updated_at: None,
            duration_milliseconds: Some(4200.0),
            output: Some(ExecutionOutput {
                run_as: Some("SYSTEM".into()),
                std_output: Some(stdout.into()),
                std_error: stderr.map(String::from),
                outputs: Some(vec![OutputValue { name: "Count".into(), variable: "n".into(), kind: "INT".into(), value: Some(InputValue { int_value: Some(3), ..Default::default() }) }]),
            }),
        });
        t
    }

    fn kinds(lines: &[OutLine]) -> Vec<Kind> {
        lines.iter().map(|l| l.kind).collect()
    }

    #[test]
    fn a_result_is_shown_as_details_variables_output_errors_and_the_message() {
        let lines = build(&task("one\ntwo\n", Some("oops"), Some("exit status 7")));
        let labels: Vec<_> = lines.iter().filter(|l| l.kind == Kind::Meta).map(|l| l.label.clone().unwrap()).collect();
        assert_eq!(labels, ["Status", "Exit code", "Duration", "Started", "Run as", "Task ID"]);
        assert_eq!(lines.iter().find(|l| l.label.as_deref() == Some("Exit code")).unwrap().text, "7");
        assert_eq!(lines.iter().find(|l| l.label.as_deref() == Some("Duration")).unwrap().text, "4.2s");
        let headings: Vec<_> = lines.iter().filter(|l| l.kind == Kind::Heading).map(|l| l.text.as_str()).collect();
        assert_eq!(headings, ["Output variables", "Output", "Errors", "N-able says"]);
        assert_eq!(lines.iter().find(|l| l.kind == Kind::Variable).unwrap().text, "3");
    }

    #[test]
    fn output_lines_are_numbered_from_one_and_a_trailing_newline_adds_no_empty_line() {
        let lines = build(&task("a\r\nb\r\nc\r\n", None, None));
        let numbered: Vec<_> = lines.iter().filter(|l| l.kind == Kind::Stdout).map(|l| (l.number.unwrap(), l.text.as_str())).collect();
        assert_eq!(numbered, [(1, "a"), (2, "b"), (3, "c")]);
    }

    #[test]
    fn tabs_become_spaces_so_columns_line_up() {
        let lines = build(&task("a\tb", None, None));
        assert_eq!(lines.iter().find(|l| l.kind == Kind::Stdout).unwrap().text, "a    b");
    }

    #[test]
    fn a_finished_run_with_nothing_to_show_says_so_and_a_running_one_says_it_is_waiting() {
        let mut quiet = task("", None, None);
        quiet.execution.as_mut().unwrap().output = None;
        assert_eq!(kinds(&build(&quiet)).last(), Some(&Kind::Note));
        assert_eq!(build(&quiet).last().unwrap().text, "No output.");
        let waiting = JobTask::new("t", None, "PC", 0);
        assert_eq!(build(&waiting).last().unwrap().text, "Waiting for results…");
    }

    #[test]
    fn plain_text_keeps_the_structure_for_copying_and_saving() {
        let text = plain_text(&build(&task("hello\nworld", None, Some("boom"))));
        assert!(text.contains("Task ID: task-1") && text.contains("== Output ==\nhello\nworld\n") && text.contains("== N-able says ==\nboom"), "{text}");
    }

    #[test]
    fn wrapping_splits_by_display_width_and_never_loses_text() {
        assert_eq!(wrap_text("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(wrap_text("short", 10), ["short"]);
        assert_eq!(wrap_text("", 5), [""]);
        // Wide characters take two columns and are never split in half.
        assert_eq!(wrap_text("日本語テキスト", 5), ["日本", "語テ", "キス", "ト"]);
        let text = "The quick brown fox jumps over the lazy dog";
        assert_eq!(wrap_text(text, 7).concat(), text);
        assert_eq!(wrap_text("abc", 0).concat(), "abc", "a zero width must not loop forever or drop text");
    }

    #[test]
    fn layout_wraps_long_lines_and_only_the_first_piece_is_marked_first() {
        let lines = vec![OutLine::new(Kind::Stdout, "abcdefghij"), OutLine::new(Kind::Stdout, "xy")];
        let rows = layout(&lines, 4, true, 0);
        assert_eq!(rows.iter().map(|r| (r.line, r.first, r.text.as_str())).collect::<Vec<_>>(), [(0, true, "abcd"), (0, false, "efgh"), (0, false, "ij"), (1, true, "xy")]);
    }

    #[test]
    fn without_wrapping_each_line_is_one_row_and_horizontal_scroll_slides_the_window() {
        let lines = vec![OutLine::new(Kind::Stdout, "0123456789")];
        assert_eq!(layout(&lines, 4, false, 0)[0].text, "0123");
        assert_eq!(layout(&lines, 4, false, 3)[0].text, "3456");
        assert_eq!(layout(&lines, 4, false, 8)[0].text, "89");
        assert_eq!(slice_columns("日本語", 2, 2), "本");
    }

    #[test]
    fn labels_are_laid_out_in_a_column() {
        let rows = layout(&[OutLine::labelled(Kind::Meta, "Status", "Passed")], 40, true, 0);
        assert_eq!(rows[0].text, "Status     Passed");
    }

    #[test]
    fn searching_ignores_case_and_finds_every_occurrence() {
        assert_eq!(match_ranges("TPM Ready - tpm present", "tpm"), [(0, 3), (12, 15)]);
        assert_eq!(match_ranges("aaaa", "aa"), [(0, 2), (2, 4)], "matches do not overlap");
        assert!(match_ranges("abc", "").is_empty());
        assert!(match_ranges("abc", "zzz").is_empty());
        assert_eq!(match_ranges("ÄÖ straße", "äö"), [(0, 2)], "non-ASCII case folds too");
    }

    #[test]
    fn find_returns_the_lines_that_match() {
        let lines = build(&task("alpha\nbeta TPM\ngamma\ntpm again", None, None));
        let hits = find(&lines, "tpm");
        assert_eq!(hits.iter().map(|i| lines[*i].text.as_str()).collect::<Vec<_>>(), ["beta TPM", "tpm again"]);
        assert!(find(&lines, "").is_empty());
    }

    #[test]
    fn the_clipboard_sequence_is_osc_52_with_base64() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }

    fn viewer(total: usize, height: usize) -> Viewer {
        let mut v = Viewer::new(vec!["a".into(), "b".into(), "c".into()]);
        v.total = total;
        v.height = height;
        v
    }

    #[test]
    fn scrolling_stops_at_both_ends_and_a_page_keeps_two_lines_of_context() {
        let mut v = viewer(100, 20);
        v.scroll_by(-5);
        assert_eq!(v.top, 0);
        v.page(true);
        assert_eq!(v.top, 18);
        v.page(true);
        assert_eq!(v.top, 36);
        v.go_bottom();
        assert_eq!(v.top, 80, "the last page ends exactly at the last line");
        v.scroll_by(10);
        assert_eq!(v.top, 80);
        v.page(false);
        assert_eq!(v.top, 62);
        v.go_top();
        assert_eq!(v.top, 0);
    }

    #[test]
    fn content_shorter_than_the_window_cannot_scroll() {
        let mut v = viewer(5, 20);
        v.scroll_by(3);
        v.go_bottom();
        v.page(true);
        assert_eq!(v.top, 0);
    }

    #[test]
    fn a_shrinking_window_or_shorter_content_pulls_the_view_back_into_range() {
        let mut v = viewer(100, 20);
        v.top = 80;
        v.total = 40;
        v.clamp();
        assert_eq!(v.top, 20);
    }

    #[test]
    fn horizontal_scroll_is_limited_to_the_widest_line() {
        let mut v = viewer(10, 5);
        v.widest = 50;
        v.width = 20;
        v.scroll_across(100);
        assert_eq!(v.hscroll, 30);
        v.scroll_across(-100);
        assert_eq!(v.hscroll, 0);
    }

    #[test]
    fn switching_devices_resets_the_position_and_stops_at_the_ends() {
        let mut v = viewer(100, 20);
        v.top = 40;
        assert!(v.switch_device(1));
        assert_eq!((v.device, v.top, v.current_task_id()), (1, 0, Some("b")));
        assert!(v.switch_device(1));
        assert!(!v.switch_device(1), "already on the last device");
        assert!(!{
            v.device = 0;
            v.switch_device(-1)
        });
    }

    #[test]
    fn next_and_previous_match_wrap_around_and_scroll_to_the_line() {
        let lines: Vec<OutLine> = (0..50).map(|i| OutLine::new(Kind::Stdout, if i % 20 == 5 { "needle here" } else { "hay" })).collect();
        let rows = layout(&lines, 80, true, 0);
        let mut v = viewer(rows.len(), 10);
        v.search = "needle".into();
        assert_eq!(v.go_to_match(&lines, &rows, 0), Some((1, 3)));
        assert_eq!(v.top, 3, "two lines of context above line 5");
        assert_eq!(v.go_to_match(&lines, &rows, 1), Some((2, 3)));
        assert_eq!(v.top, 23);
        assert_eq!(v.go_to_match(&lines, &rows, 1), Some((3, 3)));
        assert_eq!(v.go_to_match(&lines, &rows, 1), Some((1, 3)), "wraps to the first");
        assert_eq!(v.go_to_match(&lines, &rows, -1), Some((3, 3)), "and back");
        v.search = "absent".into();
        assert_eq!(v.go_to_match(&lines, &rows, 1), None);
    }

    #[test]
    fn toggling_wrap_resets_the_horizontal_position() {
        let mut v = viewer(10, 5);
        v.hscroll = 7;
        v.top = 3;
        v.toggle_wrap();
        assert!(!v.wrap && v.hscroll == 0 && v.top == 0);
    }
}
