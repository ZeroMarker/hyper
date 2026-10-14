//! Pickers and the provider form shown over the chat, so models and providers
//! are chosen from lists instead of typed as commands.
use crossterm::event::KeyCode;

use crate::{
    deepseek::{DEFAULT_BASE_URL, DEFAULT_MODEL},
    i18n,
};

/// The profile picker's last row, which opens the provider setup.
pub const ADD_PROVIDER: &str = "\u{0}add";

pub struct PickerItem {
    pub label: String,
    pub detail: String,
    pub value: String,
    pub current: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKind {
    Model,
    Profile,
    Preset,
    Session,
    /// A yes/no question; the chosen value is `yes` or `no`.
    ConfirmUndo,
    Permissions,
}

pub struct Picker {
    pub kind: PickerKind,
    pub title: &'static str,
    pub items: Vec<PickerItem>,
    pub filter: String,
    pub selected: usize,
    /// Set while the items are still being fetched.
    pub loading: bool,
    pub error: Option<String>,
}

impl Picker {
    pub fn new(kind: PickerKind, title: &'static str, items: Vec<PickerItem>) -> Self {
        let selected = items.iter().position(|item| item.current).unwrap_or(0);
        Self {
            kind,
            title,
            items,
            filter: String::new(),
            selected,
            loading: false,
            error: None,
        }
    }

    pub fn loading(kind: PickerKind, title: &'static str) -> Self {
        Self {
            loading: true,
            ..Self::new(kind, title, Vec::new())
        }
    }

    pub fn set_items(&mut self, items: Vec<PickerItem>) {
        *self = Self::new(self.kind, self.title, items);
    }

    /// Items matching the filter, case-insensitively, on label or detail.
    pub fn visible(&self) -> Vec<&PickerItem> {
        let filter = self.filter.to_lowercase();
        self.items
            .iter()
            .filter(|item| {
                item.label.to_lowercase().contains(&filter)
                    || item.detail.to_lowercase().contains(&filter)
            })
            .collect()
    }

    fn key(&mut self, code: KeyCode) -> Option<Action> {
        let count = self.visible().len();
        match code {
            KeyCode::Esc => return Some(Action::Close),
            KeyCode::Up => {
                self.selected = self
                    .selected
                    .checked_sub(1)
                    .unwrap_or(count.saturating_sub(1))
            }
            KeyCode::Down => {
                self.selected = if self.selected + 1 >= count {
                    0
                } else {
                    self.selected + 1
                }
            }
            KeyCode::Enter => {
                let item = self
                    .visible()
                    .get(self.selected)
                    .map(|item| item.value.clone());
                return item.map(|value| Action::Choose(self.kind, value));
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.selected = 0;
            }
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.selected = 0;
            }
            _ => {}
        }
        None
    }
}

pub struct Field {
    pub label: &'static str,
    pub value: String,
    /// Shown masked, for the API key.
    pub secret: bool,
}

/// Name, base URL, API key and model of a provider profile.
pub struct ProviderForm {
    pub fields: Vec<Field>,
    pub focus: usize,
    pub error: Option<String>,
}

impl ProviderForm {
    pub fn for_preset(preset: &str) -> Self {
        let (name, base_url, model) = match preset {
            "deepseek" => ("deepseek", DEFAULT_BASE_URL, DEFAULT_MODEL),
            "opencode-go" => (
                "opencode-go",
                "https://opencode.ai/zen/go/v1",
                DEFAULT_MODEL,
            ),
            _ => ("", "", ""),
        };
        let field = |label, value: &str, secret| Field {
            label,
            value: value.into(),
            secret,
        };
        let fields = vec![
            field(i18n::text("Name", "名称"), name, false),
            field("Base URL", base_url, false),
            field("API Key", "", true),
            field(i18n::text("Model", "模型"), model, false),
        ];
        // Start on the first field the preset left empty.
        let focus = fields
            .iter()
            .position(|field| field.value.is_empty())
            .unwrap_or(0);
        Self {
            fields,
            focus,
            error: None,
        }
    }

    pub fn value(&self, index: usize) -> &str {
        self.fields[index].value.trim()
    }

    fn key(&mut self, code: KeyCode) -> Option<Action> {
        let count = self.fields.len();
        match code {
            KeyCode::Esc => return Some(Action::Close),
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % count,
            KeyCode::BackTab | KeyCode::Up => self.focus = (self.focus + count - 1) % count,
            KeyCode::Enter if self.focus + 1 < count => self.focus += 1,
            KeyCode::Enter => return Some(Action::Submit),
            KeyCode::Backspace => {
                self.fields[self.focus].value.pop();
            }
            KeyCode::Char(c) => self.fields[self.focus].value.push(c),
            _ => {}
        }
        None
    }
}

pub enum Overlay {
    Picker(Picker),
    Form(ProviderForm),
}

pub enum Action {
    Close,
    Choose(PickerKind, String),
    Submit,
}

impl Overlay {
    /// A paste goes into the filter or the focused field on one line; a key
    /// copied with a trailing newline must not carry it.
    pub fn paste(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        match self {
            Overlay::Picker(picker) => {
                picker.filter.push_str(&text);
                picker.selected = 0;
            }
            Overlay::Form(form) => form.fields[form.focus].value.push_str(&text),
        }
    }

    pub fn key(&mut self, code: KeyCode) -> Option<Action> {
        match self {
            Overlay::Picker(picker) => picker.key(code),
            Overlay::Form(form) => form.key(code),
        }
    }
}

pub fn presets() -> Vec<PickerItem> {
    let item = |label: &str, detail: &str, value: &str| PickerItem {
        label: label.into(),
        detail: detail.into(),
        value: value.into(),
        current: false,
    };
    vec![
        item("DeepSeek", DEFAULT_BASE_URL, "deepseek"),
        item(
            "OpenCode Go",
            "https://opencode.ai/zen/go/v1",
            "opencode-go",
        ),
        item(
            i18n::text("OpenAI-compatible (custom)", "OpenAI 兼容（自定义）"),
            i18n::text("any base URL", "任意 Base URL"),
            "custom",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, current: bool) -> PickerItem {
        PickerItem {
            label: label.into(),
            detail: String::new(),
            value: label.into(),
            current,
        }
    }

    #[test]
    fn picker_starts_on_current_filters_and_chooses() {
        let mut picker = Picker::new(
            PickerKind::Model,
            "Models",
            vec![
                item("glm-5.3", false),
                item("deepseek-v4-pro", true),
                item("GLM-5.1", false),
            ],
        );
        assert_eq!(picker.selected, 1);
        for c in "glm".chars() {
            assert!(picker.key(KeyCode::Char(c)).is_none());
        }
        assert_eq!(picker.visible().len(), 2, "filter ignores case");
        picker.key(KeyCode::Down);
        match picker.key(KeyCode::Enter) {
            Some(Action::Choose(PickerKind::Model, value)) => assert_eq!(value, "GLM-5.1"),
            _ => panic!("expected a choice"),
        }
        picker.key(KeyCode::Down);
        assert_eq!(picker.selected, 0, "moving past the end wraps");
        assert!(matches!(picker.key(KeyCode::Esc), Some(Action::Close)));
    }

    #[test]
    fn provider_form_walks_fields_and_submits_on_the_last() {
        let mut form = ProviderForm::for_preset("opencode-go");
        assert_eq!(form.focus, 2, "starts on the empty API key");
        for c in "sk-1".chars() {
            form.key(KeyCode::Char(c));
        }
        assert_eq!(form.value(2), "sk-1");
        assert!(form.key(KeyCode::Enter).is_none());
        assert_eq!(form.focus, 3);
        assert!(matches!(form.key(KeyCode::Enter), Some(Action::Submit)));
        let custom = ProviderForm::for_preset("custom");
        assert_eq!(custom.focus, 0);
    }
}
