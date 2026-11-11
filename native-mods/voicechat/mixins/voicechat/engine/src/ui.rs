//! The voice chat menu (V): the connection, the controls, the hotkeys and the
//! groups, on the mod's own screen. It lays itself out from a [`View`] of the
//! voice chat each frame and keeps only what is typed and focused.
use crate::Settings;
use crate::net::Mode;

/// What a hook draws, in GUI pixels: a filled `[x, y, w, h]` in ARGB, or a
/// line of text at its top left in RGB, with or without a shadow.
#[derive(Clone, Debug, PartialEq)]
pub enum Draw {
    Fill([f32; 4], u32),
    Text(String, [f32; 2], u32, bool),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Mute,
    Disable,
    Mode,
    Labels,
    Volume(i32),
    Gain(i32),
    Threshold(i32),
    Join(u128),
    JoinWithPassword,
    CancelJoin,
    Leave,
    Focus(Field),
    Kind,
    Create,
    Scroll(i32),
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Password,
    JoinPassword,
}

/// A group as the menu lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupRow {
    pub id: u128,
    pub name: String,
    pub has_password: bool,
    pub members: Vec<String>,
    pub ours: bool,
}

/// The voice chat as the menu shows it this frame.
pub struct View {
    pub status: (String, u32),
    pub settings: Settings,
    pub level_db: f32,
    pub connected: bool,
    pub groups_enabled: bool,
    pub groups: Vec<GroupRow>,
    /// The hotkeys: what each does and the key bound to it, already named.
    pub hotkeys: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub rect: [f32; 4],
    pub action: Action,
}

const PANEL: u32 = 0xE010_1018;
const PANEL_EDGE: u32 = 0xFF50_5060;
const BUTTON: u32 = 0xFF30_3038;
const BUTTON_HOVER: u32 = 0xFF40_4058;
const BUTTON_ON: u32 = 0xFF2E_5E2E;
const BUTTON_OFF: u32 = 0xFF6A_2A2A;
const FIELD: u32 = 0xFF00_0000;
const FOCUS: u32 = 0xFFFF_FFFF;
const INK: u32 = 0xFF_FFFF;
const GREY: u32 = 0xAA_AAAA;
const DARK: u32 = 0x77_7777;
const GOLD: u32 = 0xFF_CC55;
const GREEN: u32 = 0x55_FF55;
const RED: u32 = 0xFF_5555;

pub const NAME_MAX: usize = 24;
const PASSWORD_MAX: usize = 24;
const ROW: f32 = 24.0;

#[derive(Default)]
pub struct Menu {
    pub open: bool,
    close_requested: bool,
    pub name: String,
    pub password: String,
    pub kind: i16,
    pub focus: Option<Field>,
    /// The password group being joined, and what is typed for it.
    pub joining: Option<u128>,
    pub join_password: String,
    pub message: Option<(String, u32)>,
    scroll: usize,
    /// The character of the key that opened the menu, typed with it.
    skip_char: Option<char>,
    widgets: Vec<Widget>,
}

struct Out<'a> {
    draws: Vec<Draw>,
    widgets: Vec<Widget>,
    mouse: [f32; 2],
    measure: &'a dyn Fn(&str) -> f32,
}

fn inside(rect: [f32; 4], at: [f32; 2]) -> bool {
    at[0] >= rect[0] && at[0] < rect[0] + rect[2] && at[1] >= rect[1] && at[1] < rect[1] + rect[3]
}

impl Out<'_> {
    fn fill(&mut self, rect: [f32; 4], argb: u32) {
        self.draws.push(Draw::Fill(rect, argb));
    }
    fn text(&mut self, text: &str, at: [f32; 2], rgb: u32) {
        self.draws.push(Draw::Text(text.to_owned(), at, rgb, true));
    }
    fn width(&self, text: &str) -> f32 {
        (self.measure)(text)
    }
    fn boxed(&mut self, rect: [f32; 4], edge: u32, inner: u32) {
        let [x, y, w, h] = rect;
        self.fill(rect, edge);
        self.fill([x + 1.0, y + 1.0, w - 2.0, h - 2.0], inner);
    }
    /// `text` cut to `max` pixels with an ellipsis.
    fn fit(&self, text: &str, max: f32) -> String {
        if self.width(text) <= max {
            return text.to_owned();
        }
        let mut cut = text.to_owned();
        while !cut.is_empty() && self.width(&format!("{cut}...")) > max {
            cut.pop();
        }
        format!("{cut}...")
    }
    /// A button; `state` colours it: Some(true) on, Some(false) off.
    fn button(&mut self, rect: [f32; 4], label: &str, action: Action, state: Option<bool>) {
        let hover = inside(rect, self.mouse);
        let fill = match state {
            _ if hover => BUTTON_HOVER,
            Some(true) => BUTTON_ON,
            Some(false) => BUTTON_OFF,
            None => BUTTON,
        };
        self.boxed(rect, if hover { FOCUS } else { PANEL_EDGE }, fill);
        let label = self.fit(label, rect[2] - 4.0);
        let w = self.width(&label);
        self.text(&label, [rect[0] + (rect[2] - w) / 2.0, rect[1] + (rect[3] - 8.0) / 2.0], INK);
        self.widgets.push(Widget { rect, action });
    }
    /// A text field showing `value` (dots for a password), focused or not.
    fn field(&mut self, rect: [f32; 4], value: &str, hint: &str, secret: bool, focus: bool, action: Action) {
        self.boxed(rect, if focus { FOCUS } else { PANEL_EDGE }, FIELD);
        let shown = if secret { "*".repeat(value.chars().count()) } else { value.to_owned() };
        let (text, rgb) = if shown.is_empty() && !focus { (hint.to_owned(), DARK) } else { (shown, INK) };
        let mut text = text;
        while !text.is_empty() && self.width(&text) > rect[2] - 10.0 {
            text.remove(0);
        }
        let caret = if focus { "_" } else { "" };
        self.text(&format!("{text}{caret}"), [rect[0] + 3.0, rect[1] + 2.0], rgb);
        self.widgets.push(Widget { rect, action });
    }
    /// `label: [-] value [+]` across `w`.
    fn stepper(&mut self, x: f32, y: f32, w: f32, label: &str, value: &str, down: Action, up: Action) {
        self.text(label, [x, y + 2.0], GREY);
        let right = x + w;
        self.button([right - 12.0, y, 12.0, 12.0], "+", up, None);
        let vw = 44.0;
        let vx = right - 14.0 - vw;
        let tw = self.width(value);
        self.text(value, [vx + (vw - tw) / 2.0, y + 2.0], INK);
        self.button([vx - 14.0, y, 12.0, 12.0], "-", down, None);
    }
}

pub const KINDS: [&str; 3] = ["Normal", "Open", "Isolated"];

impl Menu {
    pub fn open(&mut self, opened_by: Option<char>) {
        self.open = true;
        self.skip_char = opened_by;
        self.message = None;
    }

    pub fn reset(&mut self) {
        self.open = false;
        self.focus = None;
        self.joining = None;
        self.join_password.clear();
        self.skip_char = None;
        self.widgets.clear();
    }

    pub fn close(&mut self) {
        self.reset();
        self.close_requested = true;
    }

    pub fn take_close(&mut self) -> bool {
        std::mem::take(&mut self.close_requested)
    }

    /// The game's screen is (`ours`) or isn't the menu: forget a menu the
    /// game closed.
    pub fn sync_open(&mut self, ours: bool) {
        if !ours && self.open {
            self.reset();
        }
    }

    /// The action under the pointer, from the last frame drawn.
    pub fn hit(&self, at: [f32; 2]) -> Option<Action> {
        self.widgets.iter().rev().find(|w| inside(w.rect, at)).map(|w| w.action.clone())
    }

    /// Where the widget doing `action` was drawn last.
    pub fn hit_rect(&self, action: &Action) -> Option<[f32; 4]> {
        self.widgets.iter().find(|w| &w.action == action).map(|w| w.rect)
    }

    fn field_mut(&mut self, f: Field) -> &mut String {
        match f {
            Field::Name => &mut self.name,
            Field::Password => &mut self.password,
            Field::JoinPassword => &mut self.join_password,
        }
    }

    pub fn char_typed(&mut self, c: char) {
        if let Some(skip) = self.skip_char.take()
            && skip.eq_ignore_ascii_case(&c)
        {
            return;
        }
        let Some(f) = self.focus else { return };
        let max = if f == Field::Name { NAME_MAX } else { PASSWORD_MAX };
        let field = self.field_mut(f);
        if !c.is_control() && field.chars().count() < max && !(f != Field::Name && c == ' ') {
            field.push(c);
        }
    }

    /// Editing keys. Returns the action Enter asks for, if any.
    pub fn key(&mut self, key: &str, shift: bool) -> Option<Action> {
        self.skip_char = None;
        match key {
            "key.keyboard.escape" => {
                if self.joining.is_some() {
                    return Some(Action::CancelJoin);
                }
                if self.focus.is_some() {
                    self.focus = None;
                    return None;
                }
                return Some(Action::Close);
            }
            "key.keyboard.backspace" => {
                if let Some(f) = self.focus {
                    self.field_mut(f).pop();
                }
            }
            "key.keyboard.tab" => {
                self.focus = match (self.focus, shift) {
                    (Some(Field::JoinPassword), _) => Some(Field::JoinPassword),
                    (Some(Field::Name), false) | (Some(Field::Password), true) => Some(Field::Password),
                    _ => Some(Field::Name),
                };
                if self.joining.is_some() {
                    self.focus = Some(Field::JoinPassword);
                }
            }
            "key.keyboard.enter" | "key.keyboard.keypad.enter" => match self.focus {
                Some(Field::JoinPassword) => return Some(Action::JoinWithPassword),
                Some(_) => return Some(Action::Create),
                None => {}
            },
            "key.keyboard.up" => self.scroll = self.scroll.saturating_sub(1),
            "key.keyboard.down" => self.scroll += 1,
            _ => {}
        }
        None
    }

    /// Whether typing goes into a field, so other keys (the menu key) type.
    pub fn typing(&self) -> bool {
        self.focus.is_some()
    }

    pub fn scroll_by(&mut self, steps: i32) {
        self.scroll = (self.scroll as i32 - steps).max(0) as usize;
    }

    pub fn draw(&mut self, view: &View, size: [f32; 2], mouse: [f32; 2], measure: &dyn Fn(&str) -> f32) -> Vec<Draw> {
        let mut out = Out { draws: Vec::new(), widgets: Vec::new(), mouse, measure };
        let w = (size[0] - 16.0).min(440.0);
        let h = (size[1] - 16.0).min(260.0);
        let (x0, y0) = ((size[0] - w) / 2.0, (size[1] - h) / 2.0);
        out.fill([0.0, 0.0, size[0], size[1]], 0x8000_0000);
        out.boxed([x0, y0, w, h], PANEL_EDGE, PANEL);
        out.text("Voice Chat", [x0 + 6.0, y0 + 5.0], GOLD);
        out.button([x0 + w - 16.0, y0 + 3.0, 12.0, 12.0], "x", Action::Close, None);
        let (status, rgb) = &view.status;
        let tw = out.width("Voice Chat") + 12.0;
        let st = out.fit(status, w - tw - 24.0);
        out.text(&st, [x0 + tw, y0 + 5.0], *rgb);

        let col = (w - 18.0) / 2.0;
        let (lx, rx, top) = (x0 + 6.0, x0 + 12.0 + col, y0 + 20.0);
        out.fill([x0 + 9.0 + col, top, 1.0, h - 26.0], PANEL_EDGE);
        self.draw_controls(&mut out, view, lx, top, col);
        self.draw_groups(&mut out, view, rx, top, col, y0 + h - 6.0);
        self.widgets = out.widgets;
        out.draws
    }

    fn draw_controls(&self, out: &mut Out<'_>, view: &View, x: f32, top: f32, w: f32) {
        let s = &view.settings;
        let mut y = top;
        let toggle = |out: &mut Out<'_>, y: &mut f32, label: String, action, on| {
            out.button([x, *y, w, 12.0], &label, action, Some(on));
            *y += 14.0;
        };
        toggle(out, &mut y, format!("Microphone: {}", if s.muted { "Muted" } else { "On" }), Action::Mute, !s.muted);
        toggle(out, &mut y, format!("Voice chat: {}", if s.disabled { "Off" } else { "On" }), Action::Disable, !s.disabled);
        let mode = if s.mode == Mode::Voice { "Voice activation" } else { "Push to talk" };
        out.button([x, y, w, 12.0], &format!("Activation: {mode}"), Action::Mode, None);
        y += 14.0;
        toggle(out, &mut y, format!("Group names above players: {}", if s.group_labels { "On" } else { "Off" }), Action::Labels, s.group_labels);
        y += 2.0;
        out.stepper(x, y, w, "Voice volume", &format!("{}%", s.volume), Action::Volume(-25), Action::Volume(25));
        y += 14.0;
        out.stepper(x, y, w, "Microphone gain", &format!("{}%", s.mic_gain), Action::Gain(-25), Action::Gain(25));
        y += 14.0;
        out.stepper(x, y, w, "Activation level", &format!("{} dB", s.threshold), Action::Threshold(-5), Action::Threshold(5));
        y += 14.0;
        // The microphone's level against the activation level, -80 to 0 dB.
        let bar = [x, y, w, 5.0];
        out.fill(bar, 0xFF00_0000);
        let at = |db: f32| ((db + 80.0) / 80.0).clamp(0.0, 1.0) * (w - 2.0);
        let loud = view.level_db > s.threshold as f32;
        out.fill([x + 1.0, y + 1.0, at(view.level_db), 3.0], if loud { 0xFF55_FF55 } else { 0xFF55_7755 });
        out.fill([x + 1.0 + at(s.threshold as f32), y, 1.0, 5.0], 0xFFFF_CC55);
        y += 10.0;

        out.text("Hotkeys", [x, y], GOLD);
        y += 11.0;
        for (what, key) in &view.hotkeys {
            out.text(&out.fit(what, w * 0.6), [x + 2.0, y], GREY);
            let kw = out.width(key);
            out.text(key, [x + w - kw, y], if key == "not bound" { DARK } else { INK });
            y += 10.0;
        }
        out.text(&out.fit("Change keys in Options > Mods > Horizon Voice Chat", w), [x, y + 1.0], DARK);
    }

    fn draw_groups(&self, out: &mut Out<'_>, view: &View, x: f32, top: f32, w: f32, bottom: f32) {
        let mut y = top;
        out.text("Groups", [x, y + 2.0], GOLD);
        if let Some(ours) = view.groups.iter().find(|g| g.ours) {
            let label = out.fit(&format!("In {}", ours.name), w - 60.0);
            let lw = out.width(&label);
            out.text(&label, [x + w - 46.0 - lw, y + 2.0], GREEN);
            out.button([x + w - 42.0, y, 42.0, 12.0], "Leave", Action::Leave, None);
        }
        y += 15.0;
        // The create form at the bottom, the message above it.
        let form_top = bottom - 42.0;
        if !view.connected || !view.groups_enabled {
            let why = if view.connected { "Groups are off on this server" } else { "Voice chat isn't connected" };
            out.text(&out.fit(why, w), [x, y], GREY);
            return;
        }
        let list_bottom = form_top - if self.message.is_some() { 12.0 } else { 2.0 };
        let fits = (((list_bottom - y) / ROW).floor().max(1.0)) as usize;
        let scroll = self.scroll.min(view.groups.len().saturating_sub(fits));
        if view.groups.is_empty() {
            out.text("No groups yet: make one below", [x, y], GREY);
        }
        for g in view.groups.iter().skip(scroll).take(fits) {
            let row = [x, y, w, ROW - 2.0];
            out.fill(row, if g.ours { 0x6030_6030 } else { 0x4030_3040 });
            if self.joining == Some(g.id) {
                out.text(&out.fit(&format!("Password for {}", g.name), w - 8.0), [x + 3.0, y + 2.0], INK);
                let fw = w - 84.0;
                out.field([x + 2.0, y + 11.0, fw, 10.0], &self.join_password, "password", true, self.focus == Some(Field::JoinPassword), Action::Focus(Field::JoinPassword));
                out.button([x + fw + 4.0, y + 10.0, 36.0, 11.0], "Join", Action::JoinWithPassword, None);
                out.button([x + fw + 42.0, y + 10.0, 38.0, 11.0], "Cancel", Action::CancelJoin, None);
            } else {
                let lock = if g.has_password { " (password)" } else { "" };
                out.text(&out.fit(&format!("{}{lock}", g.name), w - 50.0), [x + 3.0, y + 2.0], if g.ours { GREEN } else { INK });
                let members = if g.members.is_empty() { "nobody".to_owned() } else { g.members.join(", ") };
                out.text(&out.fit(&members, w - 50.0), [x + 3.0, y + 12.0], GREY);
                if !g.ours {
                    out.button([x + w - 40.0, y + 5.0, 38.0, 12.0], "Join", Action::Join(g.id), None);
                }
            }
            y += ROW;
        }
        if view.groups.len() > fits {
            let more = format!("{}-{} of {}", scroll + 1, (scroll + fits).min(view.groups.len()), view.groups.len());
            out.button([x + w - 22.0, list_bottom - 26.0, 10.0, 12.0], "^", Action::Scroll(-1), None);
            out.button([x + w - 11.0, list_bottom - 26.0, 10.0, 12.0], "v", Action::Scroll(1), None);
            let mw = out.width(&more);
            out.text(&more, [x + w - 26.0 - mw, list_bottom - 24.0], DARK);
        }
        if let Some((m, rgb)) = &self.message {
            out.text(&out.fit(m, w), [x, form_top - 10.0], *rgb);
        }
        out.text("New group", [x, form_top + 1.0], GOLD);
        let fy = form_top + 11.0;
        let half = (w - 4.0) / 2.0;
        out.field([x, fy, half, 12.0], &self.name, "name", false, self.focus == Some(Field::Name), Action::Focus(Field::Name));
        out.field([x + half + 4.0, fy, half, 12.0], &self.password, "password (optional)", true, self.focus == Some(Field::Password), Action::Focus(Field::Password));
        let by = fy + 15.0;
        out.button([x, by, half, 12.0], &format!("Type: {}", KINDS[self.kind as usize % 3]), Action::Kind, None);
        out.button([x + half + 4.0, by, half, 12.0], "Create", Action::Create, None);
    }

    /// What a click on the menu's own parts does; the rest is the voice
    /// chat's to do (returned).
    pub fn act(&mut self, action: Action) -> Option<Action> {
        match action {
            Action::Focus(f) => self.focus = Some(f),
            Action::Kind => self.kind = (self.kind + 1) % 3,
            Action::Scroll(d) => self.scroll_by(-d),
            Action::CancelJoin => {
                self.joining = None;
                self.join_password.clear();
                self.focus = None;
            }
            Action::Close => self.close(),
            other => {
                if !matches!(other, Action::JoinWithPassword | Action::Create) {
                    self.focus = None;
                }
                return Some(other);
            }
        }
        None
    }

    pub fn say(&mut self, text: impl Into<String>, error: bool) {
        self.message = Some((text.into(), if error { RED } else { GREEN }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            status: ("Connected".into(), 0x55FF55),
            settings: Settings::default(),
            level_db: -30.0,
            connected: true,
            groups_enabled: true,
            groups: vec![
                GroupRow { id: 1, name: "Builders".into(), has_password: false, members: vec!["Alex".into()], ours: false },
                GroupRow { id: 2, name: "Secret".into(), has_password: true, members: vec![], ours: false },
            ],
            hotkeys: vec![("Push to talk".into(), "Caps Lock".into()), ("Whisper".into(), "not bound".into())],
        }
    }

    fn texts(draws: &[Draw]) -> Vec<String> {
        draws.iter().filter_map(|d| if let Draw::Text(t, ..) = d { Some(t.clone()) } else { None }).collect()
    }

    fn centre(m: &Menu, action: &Action) -> [f32; 2] {
        let w = m.widgets.iter().find(|w| &w.action == action).unwrap();
        [w.rect[0] + w.rect[2] / 2.0, w.rect[1] + w.rect[3] / 2.0]
    }

    #[test]
    fn shows_hotkeys_groups_and_members() {
        let mut m = Menu::default();
        m.open(Some('v'));
        let measure = |t: &str| t.len() as f32 * 6.0;
        let t = texts(&m.draw(&view(), [480.0, 270.0], [0.0, 0.0], &measure));
        for want in ["Hotkeys", "Push to talk", "Caps Lock", "not bound", "Builders", "Alex", "Secret (password)"] {
            assert!(t.iter().any(|x| x == want), "{want} missing from {t:?}");
        }
        // Every control is on screen.
        assert!(m.widgets.iter().all(|w| w.rect[0] >= 0.0 && w.rect[1] + w.rect[3] <= 270.0));
        let join = centre(&m, &Action::Join(1));
        assert_eq!(m.hit(join), Some(Action::Join(1)));
    }

    #[test]
    fn typing_a_new_group_and_creating_it() {
        let mut m = Menu::default();
        m.open(Some('v'));
        m.char_typed('v');
        assert_eq!(m.act(Action::Focus(Field::Name)), None);
        for c in "Team A".chars() {
            m.char_typed(c);
        }
        m.key("key.keyboard.tab", false);
        for c in "p w".chars() {
            m.char_typed(c);
        }
        assert_eq!((m.name.as_str(), m.password.as_str()), ("Team A", "pw"));
        m.act(Action::Kind);
        assert_eq!(m.kind, 1);
        assert_eq!(m.key("key.keyboard.enter", false), Some(Action::Create));
        assert_eq!(m.key("key.keyboard.escape", false), None);
        assert_eq!(m.key("key.keyboard.escape", false), Some(Action::Close));
    }
}
