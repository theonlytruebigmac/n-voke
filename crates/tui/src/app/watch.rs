//! "Watch a task": follow a run that was started elsewhere by pasting its task ID.

use nvoke_core::{jobs::Job, watch::watch_task};
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, Focus, Msg, Page, Pane, run::NoticeKind};
use crate::input::TextInput;

#[derive(Default)]
pub struct WatchState {
    pub input: TextInput,
    pub busy: bool,
    pub error: Option<String>,
}

impl App {
    pub(super) fn open_watch(&mut self) {
        self.watch = Some(WatchState::default());
    }

    pub(super) fn on_watch_key(&mut self, key: KeyEvent) {
        let Some(w) = self.watch.as_mut() else { return };
        if w.busy {
            return; // the lookup is in flight
        }
        match key.code {
            KeyCode::Esc => self.watch = None,
            KeyCode::Enter => {
                let raw = w.input.value.clone();
                w.busy = true;
                w.error = None;
                self.spawn(move |core| async move { Msg::Watched(watch_task(&core, &raw).await.map(Box::new)) });
            }
            _ => {
                if w.input.handle_key(key) {
                    w.error = None;
                }
            }
        }
    }

    pub(super) fn on_watched(&mut self, result: Result<Box<Job>, String>) {
        match result {
            Ok(job) => {
                let job = *job;
                let (id, script, device) = (job.id.clone(), job.script_name.clone(), job.target_label.clone());
                let store = self.jobs.clone();
                self.runtime.spawn(async move { store.add(job).await });
                self.watch = None;
                self.selected_job = Some(id);
                self.jobs_version = u64::MAX; // refresh as soon as the store has it
                self.page = Page::Activity;
                self.pane = Pane::Jobs;
                self.focus = Focus::Content;
                self.flash(NoticeKind::Info, format!("Watching {script} on {device}."));
            }
            Err(e) => {
                self.note_error(&e);
                if let Some(w) = self.watch.as_mut() {
                    w.busy = false;
                    w.error = Some(e);
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use base64::{Engine, engine::general_purpose::STANDARD as B64};
    use ratatui::crossterm::event::KeyCode;

    use super::*;
    use crate::app::run::tests::{press, pump, run_app, type_text};

    fn watch_app() -> (tokio::runtime::Runtime, App) {
        let (rt, mut app) = run_app();
        app.go(Page::Activity);
        app.focus = Focus::Content;
        (rt, app)
    }

    #[test]
    fn w_opens_the_watch_box_and_it_owns_the_keyboard_while_open() {
        let (_rt, mut app) = watch_app();
        press(&mut app, KeyCode::Char('W'));
        assert!(app.watch.is_some());
        type_text(&mut app, "q?1dRsx");
        assert!(!app.quit && !app.show_help && app.page == Page::Activity && app.confirm.is_none());
        assert_eq!(app.watch.as_ref().unwrap().input.value, "q?1dRsx", "every key is text here");
        press(&mut app, KeyCode::Esc);
        assert!(app.watch.is_none());
    }

    #[test]
    fn a_blank_id_is_refused_with_a_reason_and_the_box_stays_open() {
        let (_rt, mut app) = watch_app();
        press(&mut app, KeyCode::Char('W'));
        type_text(&mut app, "  ");
        press(&mut app, KeyCode::Enter);
        assert!(app.watch.as_ref().unwrap().busy);
        pump(&mut app);
        let w = app.watch.as_ref().unwrap();
        assert!(!w.busy && w.error.as_deref() == Some("Paste a task ID first."));
    }

    #[test]
    fn editing_the_id_clears_the_old_error() {
        let (_rt, mut app) = watch_app();
        press(&mut app, KeyCode::Char('W'));
        press(&mut app, KeyCode::Enter);
        pump(&mut app);
        assert!(app.watch.as_ref().unwrap().error.is_some());
        type_text(&mut app, "a");
        assert!(app.watch.as_ref().unwrap().error.is_none());
    }

    #[test]
    fn a_failed_lookup_keeps_the_box_open_and_says_why() {
        let (_rt, mut app) = watch_app();
        press(&mut app, KeyCode::Char('W'));
        type_text(&mut app, "something");
        app.on_watched(Err("No task found with that ID.".into()));
        assert_eq!(app.watch.as_ref().unwrap().error.as_deref(), Some("No task found with that ID."));
        app.on_watched(Err("Access denied (HTTP 403): your API key is no longer active.".into()));
        assert!(app.key_rejected);
    }

    #[test]
    fn a_found_task_becomes_a_run_in_activity_and_is_selected() {
        let (rt, mut app) = watch_app();
        press(&mut app, KeyCode::Char('W'));
        let id = B64.encode("task:asset");
        let snapshot = nvoke_core::model::TaskSnapshot {
            id: id.clone(),
            name: Some("Weekly".into()),
            status: "SUCCEEDED".into(),
            created_at: Some("2026-10-17T12:00:00Z".into()),
            script: Some(nvoke_core::model::ScriptRef { id: "s".into(), name: "Cleanup".into() }),
            executions: None,
        };
        let job = nvoke_core::watch::watched_job(&id, &snapshot, "HP Laptop", chrono::Utc::now());
        app.on_watched(Ok(Box::new(job)));
        assert!(app.watch.is_none() && app.page == Page::Activity && app.pane == Pane::Jobs && app.focus == Focus::Content);
        assert_eq!(app.selected_job.as_deref(), Some(id.as_str()));
        assert_eq!(app.flash.as_ref().unwrap().0.text, "Watching Cleanup on HP Laptop.");
        rt.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(100)).await });
        app.sync_jobs();
        assert_eq!(app.job_list.len(), 1);
    }
}
