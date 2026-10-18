//! What the footer says you can press right now, and the help overlay. Both depend on where the
//! user is: a page's own keys come first, then the ones that work everywhere.

use crate::app::{App, Focus, Page, Pane, RunPane, TagsPane};

pub type Hint = (&'static str, &'static str);

/// The keys for the footer, most important first: a narrow window drops the end of the list
/// (see `theme::fit_hints`), so what must never go (the way out) comes early. `? help` is added by the footer.
pub fn footer_hints(app: &App) -> Vec<Hint> {
    if app.watch.is_some() {
        return vec![("esc", "cancel"), ("enter", "watch"), ("paste", "the task ID")];
    }
    if let Some(v) = &app.viewer {
        if v.searching {
            return vec![("esc", "cancel"), ("enter", "find"), ("type", "search")];
        }
        let mut h = vec![("esc", "close"), ("↑↓", "scroll"), ("PgUp/PgDn", "page")];
        if v.task_ids.len() > 1 {
            h.push(("←→", "device"));
        }
        h.extend([("/", "search"), ("n/N", "next/prev"), ("w", "wrap"), ("y", "copy"), ("e", "save")]);
        return h;
    }
    let mut hints: Vec<Hint> = match (app.focus, app.page) {
        (Focus::Sidebar, _) => vec![("↑↓", "page"), ("→", "open"), ("q", "quit")],
        (Focus::Content, Page::Tags) if app.tag_form.as_ref().is_some_and(|f| f.editing) => vec![("enter", "done"), ("esc", "done"), ("tab", "next field"), ("type", "edit")],
        (Focus::Content, Page::Tags) if app.picker.is_some() => vec![("esc", "done"), ("enter", "add/remove"), ("↑↓", "move"), ("type", "search")],
        (Focus::Content, Page::Tags) if app.tag_form.is_some() => vec![("↑↓", "field"), ("enter", "choose/edit"), ("esc", "leave"), ("←→", "part/switch"), ("+", "add row"), ("x", "remove row")],
        (Focus::Content, Page::Tags) if app.tags.searching => vec![("enter", "done"), ("esc", "clear"), ("type", "filter")],
        (Focus::Content, Page::Tags) if app.tags.pane == TagsPane::Devices => vec![("↑↓", "device"), ("space", "tick"), ("x", "remove"), ("+", "add"), ("a", "all"), ("R", "run script"), ("←", "back")],
        (Focus::Content, Page::Tags) => vec![("↑↓", "tag"), ("n", "new"), ("e", "edit"), ("/", "search"), ("→", "devices"), ("+", "add devices"), ("x", "delete"), ("R", "run script"), ("←", "back")],
        (Focus::Content, Page::Run) if app.picker.is_some() => vec![("esc", "close"), ("enter", "choose"), ("↑↓", "move"), ("type", "search")],
        (Focus::Content, Page::Run) if app.run.editing => vec![("enter", "done"), ("esc", "done"), ("tab", "next field"), ("type", "edit")],
        (Focus::Content, Page::Run) if app.run.pane == RunPane::Devices => vec![("↑↓", "device"), ("space", "include"), ("a", "all/none"), ("←", "back to form")],
        (Focus::Content, Page::Run) => vec![("↑↓", "field"), ("enter", "choose/edit"), ("→", "devices"), ("←", "back")],
        (Focus::Content, Page::Activity) if app.pane == Pane::Results => vec![("enter", "read output"), ("↑↓", "group"), ("T", "tag devices"), ("PgUp/PgDn", "scroll"), ("←", "back")],
        (Focus::Content, Page::Activity) => vec![("↑↓", "run"), ("→", "results"), ("R", "run again"), ("T", "tag devices"), ("W", "watch by ID"), ("F", "re-run failed"), ("s", "stop"), ("w", "resume"), ("x", "remove"), ("←", "back")],
        (Focus::Content, Page::Audit) => vec![("↑↓", "move"), ("PgUp/PgDn", "jump"), ("r", "refresh"), ("←", "back")],
        (Focus::Content, Page::Settings) => vec![("enter", "edit"), ("c", "clear saved data"), ("←", "back")],
    };
    if app.is_dry_run() {
        // After the first few, so a narrow window keeps the page's own keys and drops this first.
        let at = hints.len().min(3);
        hints.insert(at, ("d", "held-back changes"));
    }
    hints
}

/// A group of keys in the help overlay.
pub struct Section {
    pub title: &'static str,
    pub keys: Vec<Hint>,
}

/// The help for where the user is: this page's keys first, then what works everywhere.
pub fn help_sections(app: &App) -> Vec<Section> {
    let mut sections = vec![];
    let section = |title, keys: &[Hint]| Section { title, keys: keys.to_vec() };

    if app.tag_form.is_some() && app.page == Page::Tags {
        sections.push(section(
            "Tag form",
            &[
                ("↑ ↓", "move between fields"),
                ("enter", "choose, edit, or switch the field"),
                ("← →", "switch Static/Dynamic · move along a rule row"),
                ("+  x", "add / remove a rule row"),
                ("c", "clear the chosen devices"),
                ("enter, enter", "review, then save"),
                ("esc", "leave (asks first if you have changes)"),
            ],
        ));
    } else {
        match app.page {
            Page::Run => sections.push(section(
                "Run a script",
                &[
                    ("↑ ↓", "move between fields"),
                    ("enter", "choose (organization, script, devices) or edit a field"),
                    ("c", "clear the chosen devices and tags"),
                    ("→", "the devices list: space leaves one out, a all/none"),
                    ("enter, enter", "review, then run (d lists a dry run's held-back request)"),
                    ("In a picker", "type to search · enter chooses · tab switches devices/tags · esc closes"),
                ],
            )),
            Page::Activity => {
                sections.push(section(
                    "Activity",
                    &[
                        ("→  ←", "runs ⇄ results ⇄ menu"),
                        ("enter", "on a result: read its output in full"),
                        ("R  F", "run again · re-run only the devices that failed"),
                        ("T", "put these devices in a tag (or make a new one)"),
                        ("W", "watch a run started elsewhere, by its task ID"),
                        ("s  w  x", "stop watching · resume watching · remove from history"),
                    ],
                ));
                sections.push(section(
                    "Reading output",
                    &[
                        ("↑ ↓  PgUp PgDn", "scroll · home/end for the ends"),
                        ("← →", "previous / next device in the group"),
                        ("/  n  N", "search · next · previous match"),
                        ("w  < >", "wrap lines · scroll sideways when not wrapped"),
                        ("y  e", "copy to the clipboard · save to a file"),
                    ],
                ));
            }
            Page::Tags => sections.push(section(
                "Tags",
                &[
                    ("↑ ↓  /", "choose a tag · search by name"),
                    ("→", "its devices: space ticks, a all, x removes the ticked"),
                    ("n  e", "new tag · edit this tag"),
                    ("+", "add devices to a static tag"),
                    ("x", "delete the tag (on the list) · remove devices (on the devices)"),
                    ("R", "run a script on the tag's devices"),
                ],
            )),
            Page::Audit => sections.push(section("Audit log", &[("↑ ↓  PgUp PgDn", "move through the entries"), ("g  G", "first · last"), ("r", "read the log again")])),
            Page::Settings => sections.push(section("Settings", &[("enter", "change the endpoint, API key or how long history is kept"), ("c", "clear saved run history and cached data")])),
        }
    }

    sections.push(section(
        "Everywhere",
        &[
            ("1-5  tab", "go to a page"),
            ("↑ ↓  → ←", "move through the menu, step into a page, step back out"),
            ("?", "this help"),
            ("q  ctrl-c", "quit (q is off while a form is open: esc leaves it)"),
        ],
    ));
    if app.is_dry_run() {
        sections.push(section("Dry run", &[("d", "see the changes that were held back, and exactly what each would have sent")]));
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_are_listed_most_important_first_so_a_narrow_window_loses_the_least_useful() {
        let (_rt, mut app) = crate::app::run::tests::run_app();
        app.go_to_for_tests(Page::Activity);
        let hints = footer_hints(&app);
        let position = |k: &str| hints.iter().position(|(key, _)| *key == k).unwrap();
        assert!(position("↑↓") < position("R") && position("R") < position("x"), "navigation, then the common actions, then the rare ones: {hints:?}");
        assert_eq!(hints.last().map(|h| h.0), Some("←"), "the way back is last on the page's own list, with `? help` after it");
    }

    #[test]
    fn every_context_has_hints_and_help() {
        let (_rt, mut app) = crate::app::run::tests::run_app();
        for page in Page::ALL {
            app.go_to_for_tests(page);
            assert!(!footer_hints(&app).is_empty(), "{page:?}");
            let sections = help_sections(&app);
            assert!(sections.len() >= 2 && sections.last().unwrap().title == "Everywhere", "{page:?}");
            assert!(sections.iter().all(|s| !s.keys.is_empty()));
        }
    }

    #[test]
    fn a_page_gets_its_own_keys_first_and_the_dry_run_key_only_in_a_dry_run() {
        let (_rt, mut app) = crate::app::run::tests::run_app();
        app.go_to_for_tests(Page::Tags);
        assert_eq!(help_sections(&app)[0].title, "Tags");
        assert!(!footer_hints(&app).iter().any(|h| h.0 == "d") && !help_sections(&app).iter().any(|s| s.title == "Dry run"));
        app.core().set_dry_run(true);
        assert!(footer_hints(&app).iter().any(|h| h.0 == "d") && help_sections(&app).iter().any(|s| s.title == "Dry run"));
    }

    #[test]
    fn nothing_is_documented_that_does_not_exist() {
        // The help for every page mentions only keys that the page's handler really has: spot-check the ones
        // that were renamed during the review (w is "resume", W is "watch by ID").
        let (_rt, mut app) = crate::app::run::tests::run_app();
        app.go_to_for_tests(Page::Activity);
        let text: String = help_sections(&app).iter().flat_map(|s| s.keys.iter().map(|(k, d)| format!("{k} {d}"))).collect::<Vec<_>>().join("\n");
        assert!(text.contains("resume watching") && text.contains("watch a run started elsewhere"), "{text}");
    }
}
