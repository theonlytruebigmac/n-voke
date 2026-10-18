//! A single-line text field: the value, an optional mask (for the API token),
//! and the keys that edit it.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Default)]
pub struct TextInput {
    pub value: String,
    pub masked: bool,
}

impl TextInput {
    pub fn new(value: &str, masked: bool) -> Self {
        Self { value: value.to_string(), masked }
    }

    /// What to draw: the value, or bullets when masked.
    pub fn shown(&self) -> String {
        if self.masked { "•".repeat(self.value.chars().count()) } else { self.value.clone() }
    }

    /// Applies an editing key. Returns true if the key was one it handles.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => self.value.push(c),
            KeyCode::Backspace => {
                self.value.pop();
            }
            // Ctrl-U clears the line, as in a shell.
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => self.value.clear(),
            _ => return false,
        }
        true
    }
}
