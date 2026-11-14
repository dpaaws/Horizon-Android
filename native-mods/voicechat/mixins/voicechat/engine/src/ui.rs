//! The voice chat menu (V): the connection, the controls, the hotkeys and the
//! groups, on the mod's own screen. It lays itself out from a [`View`] of the
//! voice chat each frame and keeps only what is typed and focused.
use crate::Settings;
use crate::look::{self, Rect};
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
    Hud,
    Deafen,
    Clean,
    Fx,
    CopyVoice(u128),
    Record,
    PlayNote,
    ClearNote,
    Loopback,
    /// Step one player's volume by this many percent.
    PlayerStep(u128, i32),
    PlayerMute(u128),
    Tab(Tab),
    /// A slider bar clicked at a fraction (0..1) of its width.
    Slide(Slider, f32),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Voice,
    Players,
    Groups,
    More,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slider {
    Volume,
    Gain,
    Threshold,
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

/// A player with voice chat as the menu lists them.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerRow {
    pub id: u128,
    pub name: String,
    /// The face, as [`look::head_rects`] gives it.
    pub head: Vec<Rect>,
    pub talking: bool,
    pub whisper: bool,
    pub mic_muted: bool,
    pub voice_off: bool,
    pub in_group: bool,
    /// Blocks away, when the player is in this world's reach.
    pub distance: Option<f32>,
    /// What this player's voice is set to, in percent, and whether it is muted.
    pub volume: u32,
    pub muted: bool,
    /// Our voice is set to match this player's pitch.
    pub copied: bool,
}

/// The voice chat as the menu shows it this frame.
pub struct View {
    pub status: (String, u32),
    pub settings: Settings,
    pub level_db: f32,
    pub connected: bool,
    pub groups_enabled: bool,
    pub groups: Vec<GroupRow>,
    pub players: Vec<PlayerRow>,
    pub loopback: bool,
    /// The voice note: recording, playing, and its picture.
    pub rec: crate::rec::RecView,
    /// The hotkeys: what each does and the key bound to it, already named.
    pub hotkeys: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub rect: [f32; 4],
    pub action: Action,
}

// Minecraft's own look: the inventory's light grey panel with its bevel,
// grey buttons, black fields and lists.
const BLACK: u32 = 0xFF00_0000;
const WHITE: u32 = 0xFFFF_FFFF;
const PANEL: u32 = 0xFFC6_C6C6;
const PANEL_LIGHT: u32 = 0xFFFF_FFFF;
const PANEL_SHADE: u32 = 0xFF55_5555;
const BUTTON_FACE: u32 = 0xFF6F_6F6F;
const BUTTON_LIGHT: u32 = 0xFFA5_A5A5;
const BUTTON_SHADE: u32 = 0xFF3A_3A3A;
const TAB_OFF: u32 = 0xFF8B_8B8B;
const FIELD_EDGE: u32 = 0xFFA0_A0A0;
const LIST_BG: u32 = 0xFF10_1010;
const INK: u32 = 0x40_4040;
const LABEL: u32 = 0x55_5555;
const HINT: u32 = 0x80_8080;
const LIST_GREY: u32 = 0xAA_AAAA;
const ON: u32 = 0x55_FF55;
const OFF: u32 = 0xFF_5555;
const HOVER_TEXT: u32 = 0xFF_FFA0;

// The colours the engine gives its status lines (for the HUD's dark ground).
const GREEN: u32 = 0x55FF55;
const RED: u32 = 0xFF5555;
const YELLOW: u32 = 0xFFFF55;

pub const NAME_MAX: usize = 24;
const PASSWORD_MAX: usize = 24;
const ROW: f32 = 26.0;
/// A control's height: Minecraft's own button is 20, a fingertip's minimum.
const TOUCH: f32 = 20.0;
const GAP: f32 = 4.0;

type Row<'a> = (f32, Box<dyn FnOnce(&mut Out<'_>, f32, f32) + 'a>);

#[derive(Default)]
pub struct Menu {
    pub open: bool,
    close_requested: bool,
    pub name: String,
    pub password: String,
    pub kind: i16,
    pub tab: Tab,
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
    /// Text with the game's shadow (on dark parts).
    fn text(&mut self, text: &str, at: [f32; 2], rgb: u32) {
        self.draws.push(Draw::Text(text.to_owned(), at, rgb, true));
    }
    /// Text without a shadow (on the light panel).
    fn ink(&mut self, text: &str, at: [f32; 2], rgb: u32) {
        self.draws.push(Draw::Text(text.to_owned(), at, rgb, false));
    }
    fn width(&self, text: &str) -> f32 {
        (self.measure)(text)
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
    /// Pixel art: each `#` of `rows` is one GUI pixel.
    fn icon(&mut self, x: f32, y: f32, rows: &[&str], rgb: u32) {
        for (r, argb) in look::icon_rects(rows, rgb) {
            self.fill([x + r[0], y + r[1], r[2], r[3]], argb);
        }
    }
    /// The inventory's panel: black edge, white light, dark shade.
    fn panel(&mut self, [x, y, w, h]: [f32; 4]) {
        self.fill([x, y, w, h], BLACK);
        self.fill([x + 1.0, y + 1.0, w - 2.0, h - 2.0], PANEL_SHADE);
        self.fill([x + 1.0, y + 1.0, w - 3.0, h - 3.0], PANEL_LIGHT);
        self.fill([x + 2.0, y + 2.0, w - 4.0, h - 4.0], PANEL);
    }
    /// A sunken part: the shade up and left, the light down and right.
    fn sunken(&mut self, [x, y, w, h]: [f32; 4], inner: u32) {
        self.fill([x, y, w, h], PANEL_LIGHT);
        self.fill([x, y, w - 1.0, h - 1.0], PANEL_SHADE);
        self.fill([x + 1.0, y + 1.0, w - 2.0, h - 2.0], inner);
    }
    /// A grey Minecraft button face, white-edged while the pointer is on it.
    fn face(&mut self, [x, y, w, h]: [f32; 4], hover: bool, pressed: bool) {
        self.fill([x, y, w, h], if hover { WHITE } else { BLACK });
        let (light, shade) = if pressed { (BUTTON_SHADE, BUTTON_LIGHT) } else { (BUTTON_LIGHT, BUTTON_SHADE) };
        self.fill([x + 1.0, y + 1.0, w - 2.0, h - 2.0], shade);
        self.fill([x + 1.0, y + 1.0, w - 3.0, h - 3.0], light);
        self.fill([x + 2.0, y + 2.0, w - 4.0, h - 4.0], BUTTON_FACE);
    }
    /// A button; `state` colours its words: Some(true) green, Some(false) red.
    fn button(&mut self, rect: [f32; 4], label: &str, action: Action, state: Option<bool>) {
        let hover = inside(rect, self.mouse);
        self.face(rect, hover, false);
        let label = self.fit(label, rect[2] - 6.0);
        let w = self.width(&label);
        let rgb = match state {
            _ if hover => HOVER_TEXT,
            Some(true) => ON,
            Some(false) => OFF,
            None => WHITE & 0xFF_FFFF,
        };
        self.text(&label, [rect[0] + (rect[2] - w) / 2.0, rect[1] + (rect[3] - 8.0) / 2.0], rgb);
        self.widgets.push(Widget { rect, action });
    }
    /// A text field showing `value` (stars for a password), focused or not.
    fn field(&mut self, rect: [f32; 4], value: &str, hint: &str, secret: bool, focus: bool, action: Action) {
        self.fill(rect, if focus { WHITE } else { FIELD_EDGE });
        self.fill([rect[0] + 1.0, rect[1] + 1.0, rect[2] - 2.0, rect[3] - 2.0], BLACK);
        let shown = if secret { "*".repeat(value.chars().count()) } else { value.to_owned() };
        let (mut text, rgb) = if shown.is_empty() && !focus { (hint.to_owned(), HINT) } else { (shown, WHITE & 0xFF_FFFF) };
        while !text.is_empty() && self.width(&text) > rect[2] - 10.0 {
            text.remove(0);
        }
        let caret = if focus { "_" } else { "" };
        self.text(&format!("{text}{caret}"), [rect[0] + 3.0, rect[1] + (rect[3] - 8.0) / 2.0], rgb);
        self.widgets.push(Widget { rect, action });
    }
    /// `label  [-] [ track with a handle, the value on it ] [+]`: the track
    /// sets the value where it is clicked, the buttons step it.
    fn slider(&mut self, x: f32, y: f32, w: f32, label: &str, slider: Slider, fraction: f32, value: &str, down: Action, up: Action) {
        let lw = (w * 0.34).min(100.0);
        let label = self.fit(label, lw - 4.0);
        self.ink(&label, [x, y + 6.0], INK);
        self.button([x + lw, y, TOUCH, TOUCH], "-", down, None);
        self.button([x + w - TOUCH, y, TOUCH, TOUCH], "+", up, None);
        let bar = [x + lw + TOUCH + 2.0, y, w - lw - 2.0 * TOUCH - 4.0, TOUCH];
        let hover = inside(bar, self.mouse);
        self.fill(bar, if hover { WHITE } else { BLACK });
        self.fill([bar[0] + 1.0, bar[1] + 1.0, bar[2] - 2.0, bar[3] - 2.0], 0xFF20_2020);
        let handle = 10.0;
        let at = bar[0] + 1.0 + fraction.clamp(0.0, 1.0) * (bar[2] - 2.0 - handle);
        self.face([at, bar[1] + 1.0, handle, bar[3] - 2.0], hover, false);
        let vw = self.width(value);
        self.text(value, [bar[0] + (bar[2] - vw) / 2.0, bar[1] + 6.0], WHITE & 0xFF_FFFF);
        self.widgets.push(Widget { rect: bar, action: Action::Slide(slider, 0.0) });
    }
}

const PLAYER_ROW: f32 = 30.0;

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
        self.widgets.iter().rev().find(|w| inside(w.rect, at)).map(|w| match &w.action {
            Action::Slide(slider, _) => Action::Slide(*slider, ((at[0] - w.rect[0]) / w.rect[2]).clamp(0.0, 1.0)),
            other => other.clone(),
        })
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
        let w = (size[0] - 8.0).min(420.0);
        let h = (size[1] - 8.0).min(270.0);
        let (x0, y0) = ((size[0] - w) / 2.0, (size[1] - h) / 2.0);
        out.fill([0.0, 0.0, size[0], size[1]], 0xA000_0000);
        out.panel([x0, y0, w, h]);
        // Title bar: the name, the connection's state, and a big close button.
        out.icon(x0 + 8.0, y0 + 9.0, &look::SPEAKER, INK);
        out.ink("Voice Chat", [x0 + 21.0, y0 + 9.0], INK);
        let (status, rgb) = &view.status;
        let rgb = match *rgb {
            GREEN => 0x00_8A00,
            RED => 0xAA_0000,
            YELLOW => 0x9A_6A00,
            _ => LABEL,
        };
        let tw = out.width("Voice Chat") + 29.0;
        let st = out.fit(status, w - tw - 50.0);
        let sw = out.width(&st);
        out.ink(&st, [x0 + w - 32.0 - sw, y0 + 9.0], rgb);
        out.fill([x0 + w - 42.0 - sw, y0 + 10.0, 6.0, 6.0], BLACK);
        out.fill([x0 + w - 41.0 - sw, y0 + 11.0, 4.0, 4.0], 0xFF00_0000 | rgb);
        out.button([x0 + w - 26.0, y0 + 5.0, TOUCH, TOUCH], "x", Action::Close, None);
        // Tabs, the open one joined to the panel.
        let tabs = [(Tab::Voice, "Voice"), (Tab::Players, "Players"), (Tab::Groups, "Groups"), (Tab::More, "More")];
        let tab_w = (w - 16.0) / 4.0;
        let ty = y0 + 28.0;
        out.fill([x0 + 8.0, ty + TOUCH - 1.0, w - 16.0, 1.0], PANEL_SHADE);
        for (i, (tab, label)) in tabs.iter().enumerate() {
            let rect = [x0 + 8.0 + tab_w * i as f32, ty, tab_w - 1.0, TOUCH];
            let on = self.tab == *tab;
            let hover = inside(rect, mouse);
            out.fill(rect, if hover && !on { WHITE } else { BLACK });
            out.fill([rect[0] + 1.0, rect[1] + 1.0, rect[2] - 2.0, rect[3] - 1.0], if on { PANEL } else { TAB_OFF });
            if on {
                out.fill([rect[0] + 1.0, rect[1] + 1.0, rect[2] - 2.0, 1.0], PANEL_LIGHT);
                out.fill([rect[0] + 1.0, rect[1] + rect[3] - 1.0, rect[2] - 2.0, 2.0], PANEL);
            }
            let lw = out.width(label);
            out.ink(label, [rect[0] + (rect[2] - lw) / 2.0, rect[1] + 7.0], if on { INK } else { LABEL });
            out.widgets.push(Widget { rect, action: Action::Tab(*tab) });
        }
        let (x, top, cw) = (x0 + 8.0, y0 + 54.0, w - 16.0);
        let bottom = y0 + h - 17.0;
        match self.tab {
            Tab::Voice => self.draw_voice(&mut out, view, x, top, cw, bottom),
            Tab::Players => self.draw_players(&mut out, view, x, top, cw, bottom),
            Tab::Groups => self.draw_groups(&mut out, view, x, top, cw, bottom),
            Tab::More => self.draw_more(&mut out, view, x, top, cw, bottom),
        }
        // Footer: a thin rule, then the last thing that happened, or a hint.
        out.fill([x0 + 8.0, y0 + h - 15.0, w - 16.0, 1.0], PANEL_SHADE);
        if let Some((m, rgb)) = &self.message {
            let rgb = if *rgb == RED { 0xAA_0000 } else { 0x00_8A00 };
            out.ink(&out.fit(m, w - 16.0), [x, y0 + h - 12.0], rgb);
        } else {
            let hint = match self.tab {
                Tab::Voice if view.settings.mode == Mode::Voice => "The mic opens by itself when you talk".to_owned(),
                Tab::Voice => {
                    let key = view.hotkeys.iter().find(|(what, _)| what == "Push to talk").map_or("the talk key", |(_, k)| k.as_str());
                    format!("Hold {key} to talk")
                }
                _ => "Press Esc or V to close".to_owned(),
            };
            out.ink(&out.fit(&hint, w - 16.0), [x, y0 + h - 12.0], HINT);
        }
        self.widgets = out.widgets;
        out.draws
    }

    /// Stacks `rows` (height, drawing) from `top`, `GAP` apart. What does not
    /// fit scrolls, by whole rows, with two big arrows at the right.
    fn rows(&self, out: &mut Out<'_>, rows: Vec<Row<'_>>, x: f32, top: f32, w: f32, bottom: f32) {
        let avail = bottom - top;
        let span = |r: &[Row<'_>]| r.iter().map(|(h, _)| h + GAP).sum::<f32>() - GAP;
        let overflow = span(&rows) > avail + 0.01;
        let w = if overflow { w - TOUCH - 4.0 } else { w };
        // The first row that still lets every later one fit.
        let last = (0..rows.len()).find(|i| span(&rows[*i..]) <= avail + 0.01).unwrap_or(rows.len().saturating_sub(1));
        let first = self.scroll.min(last);
        if overflow {
            let ax = x + w + 4.0;
            out.button([ax, top, TOUCH, TOUCH], "^", Action::Scroll(-1), None);
            out.button([ax, bottom - TOUCH, TOUCH, TOUCH], "v", Action::Scroll(1), None);
        }
        let mut y = top;
        for (h, draw) in rows.into_iter().skip(first) {
            if y + h > bottom + 0.01 {
                break;
            }
            draw(out, y, w);
            y += h + GAP;
        }
    }

    fn draw_voice(&self, out: &mut Out<'_>, view: &View, x: f32, top: f32, w: f32, bottom: f32) {
        let s = &view.settings;
        let auto = s.mode == Mode::Voice;
        let mut rows: Vec<Row<'_>> = Vec::new();
        rows.push((
            TOUCH,
            Box::new(move |out, y, w| {
                let third = (w - 8.0) / 3.0;
                out.button([x, y, third, TOUCH], &format!("Mic: {}", if s.muted { "Off" } else { "On" }), Action::Mute, Some(!s.muted));
                out.button([x + third + 4.0, y, third, TOUCH], &format!("Hear: {}", if s.deafened { "Off" } else { "On" }), Action::Deafen, Some(!s.deafened));
                out.button([x + 2.0 * (third + 4.0), y, third, TOUCH], &format!("Voice: {}", if s.disabled { "Off" } else { "On" }), Action::Disable, Some(!s.disabled));
            }),
        ));
        rows.push((
            TOUCH,
            Box::new(move |out, y, w| {
                out.button([x, y, w, TOUCH], &format!("Mode: {}", if auto { "Voice activation" } else { "Push to talk" }), Action::Mode, None);
            }),
        ));
        rows.push((
            TOUCH,
            Box::new(move |out, y, w| {
                out.slider(x, y, w, "Voice volume", Slider::Volume, s.volume as f32 / 200.0, &format!("{}%", s.volume), Action::Volume(-10), Action::Volume(10));
            }),
        ));
        rows.push((
            TOUCH,
            Box::new(move |out, y, w| {
                out.slider(x, y, w, "Mic gain", Slider::Gain, (s.mic_gain as f32 - 25.0) / 475.0, &format!("{}%", s.mic_gain), Action::Gain(-10), Action::Gain(10));
            }),
        ));
        if auto {
            rows.push((
                TOUCH,
                Box::new(move |out, y, w| {
                    out.slider(x, y, w, "Mic threshold", Slider::Threshold, (s.threshold as f32 + 80.0) / 80.0, &format!("{} dB", s.threshold), Action::Threshold(-2), Action::Threshold(2));
                }),
            ));
        }
        let (loopback, level_db) = (view.loopback, view.level_db);
        rows.push((
            TOUCH,
            Box::new(move |out, y, w| {
                // The microphone's level, with the activation level marked on it.
                let lw = (w * 0.34).min(100.0);
                out.button([x, y, lw, TOUCH], "Mic test", Action::Loopback, Some(loopback).filter(|on| *on));
                let bar = [x + lw + 8.0, y + 4.0, w - lw - 8.0, TOUCH - 8.0];
                out.sunken(bar, BLACK);
                let inner = bar[2] - 2.0;
                let at = |db: f32| ((db + 80.0) / 80.0).clamp(0.0, 1.0);
                let lit = at(level_db);
                let segments = ((inner / 4.0) as usize).max(10);
                let seg_w = inner / segments as f32;
                for i in 0..segments {
                    let f = (i as f32 + 0.5) / segments as f32;
                    if f > lit {
                        break;
                    }
                    let rgb = if f < 0.6 { 0xFF55_FF55 } else if f < 0.85 { 0xFFFF_FF55 } else { 0xFFFF_5555 };
                    let quiet = auto && level_db <= s.threshold as f32;
                    out.fill([bar[0] + 1.0 + seg_w * i as f32, bar[1] + 1.0, (seg_w - 1.0).max(1.0), bar[3] - 2.0], if quiet { 0xFF6A_7A6A } else { rgb });
                }
                if auto {
                    out.fill([bar[0] + 1.0 + at(s.threshold as f32) * inner, bar[1], 1.0, bar[3]], WHITE);
                }
            }),
        ));
        let note = view.rec.clone();
        rows.push((
            TOUCH,
            Box::new(move |out, y, w| {
                // A voice note, like in a chat app: record, play, delete.
                use crate::rec::RecState::{Idle, Playing, Recording};
                let third = (w - 8.0) / 3.0;
                let secs = format!("{}:{:02}", note.seconds as u32 / 60, note.seconds as u32 % 60);
                let rec_label = if note.state == Recording { format!("Stop {secs}") } else { "Record".to_owned() };
                out.button([x, y, third, TOUCH], &rec_label, Action::Record, Some(false).filter(|_| note.state == Recording));
                let has = !note.bars.is_empty();
                out.button([x + third + 4.0, y, third, TOUCH], if note.state == Playing { "Stop" } else { "Play" }, Action::PlayNote, Some(true).filter(|_| note.state == Playing || (has && note.state == Idle)));
                out.button([x + 2.0 * (third + 4.0), y, third, TOUCH], "Delete", Action::ClearNote, None);
            }),
        ));
        let note = view.rec.clone();
        rows.push((
            TOUCH,
            Box::new(move |out, y, w| {
                // The note as bars, the played part lit.
                use crate::rec::RecState::Recording;
                out.sunken([x, y, w, TOUCH], BLACK);
                if note.bars.is_empty() {
                    let hint = "Record, then Play to hear what others hear";
                    out.text(&out.fit(hint, w - 8.0), [x + 4.0, y + 6.0], LIST_GREY);
                    return;
                }
                let time = format!("{}:{:02}", note.seconds as u32 / 60, note.seconds as u32 % 60);
                let tw = out.width(&time) + 6.0;
                let area = w - 4.0 - tw;
                let n = ((area / 4.0).floor() as usize).clamp(1, note.bars.len().max(1));
                let n = if note.bars.len() >= 24 { n } else { note.bars.len() };
                for i in 0..n {
                    let v = note.bars[i * note.bars.len() / n];
                    let bh = (v * (TOUCH - 6.0)).max(1.0).round();
                    let lit = note.state != Recording && (i as f32 + 0.5) / n as f32 <= note.progress;
                    let rgb = if note.state == Recording { 0xFFFF_5555 } else if lit { 0xFF55_FF55 } else { 0xFF9A_9A9A };
                    out.fill([x + 3.0 + i as f32 * area / n as f32, y + (TOUCH - bh) / 2.0, 2.0, bh], rgb);
                }
                out.text(&time, [x + w - tw + 1.0, y + 6.0], 0xFF_FFFF);
            }),
        ));
        self.rows(out, rows, x, top, w, bottom);
    }

    fn draw_more(&self, out: &mut Out<'_>, view: &View, x: f32, top: f32, w: f32, bottom: f32) {
        let s = &view.settings;
        let mut rows: Vec<Row<'_>> = Vec::new();
        rows.push((
            TOUCH,
            Box::new(move |out, y, w| {
                out.button([x, y, w, TOUCH], &format!("Tags above players: {}", if s.group_labels { "Shown" } else { "Hidden" }), Action::Labels, Some(s.group_labels));
            }),
        ));
        rows.push((
            TOUCH,
            Box::new(move |out, y, w| {
                out.button([x, y, w, TOUCH], &format!("Corner display: {}", if s.hud { "Shown" } else { "Hidden" }), Action::Hud, Some(s.hud));
            }),
        ));
        rows.push((
            TOUCH,
            Box::new(move |out, y, w| {
                out.button([x, y, w, TOUCH], &format!("Clean mic: {}", if s.clean { "On" } else { "Off" }), Action::Clean, Some(s.clean));
            }),
        ));
        rows.push((
            TOUCH,
            Box::new(move |out, y, w| {
                let label = if s.fx == crate::dsp::Fx::Copy && !s.copy_name.is_empty() {
                    let waiting = if s.copy_hz > 0.0 { "" } else { " (waiting)" };
                    format!("Voice changer: {}'s voice{waiting}", s.copy_name)
                } else {
                    format!("Voice changer: {}", s.fx.name())
                };
                out.button([x, y, w, TOUCH], &label, Action::Fx, Some(true).filter(|_| s.fx != crate::dsp::Fx::Off));
            }),
        ));
        rows.push((
            12.0,
            Box::new(move |out, y, _| {
                out.ink("Hotkeys", [x, y + 2.0], INK);
            }),
        ));
        for (what, key) in &view.hotkeys {
            rows.push((
                16.0,
                Box::new(move |out, y, w| {
                    out.sunken([x, y, w, 16.0], LIST_BG);
                    out.text(&out.fit(what, w * 0.62), [x + 4.0, y + 4.0], LIST_GREY);
                    let kw = out.width(key);
                    out.text(key, [x + w - kw - 4.0, y + 4.0], if key == "not set" { HINT } else { 0xFF_FFFF });
                }),
            ));
        }
        self.rows(out, rows, x, top, w, bottom);
    }

    fn draw_players(&self, out: &mut Out<'_>, view: &View, x: f32, top: f32, w: f32, bottom: f32) {
        let mut y = top;
        if !view.connected {
            out.ink("Voice chat is not connected", [x, y + 2.0], LABEL);
            return;
        }
        out.ink(&format!("Players with voice chat: {}", view.players.len()), [x, y + 2.0], INK);
        y += 14.0;
        let list_h = (bottom - y).max(PLAYER_ROW + 2.0);
        let fits = (((list_h - 2.0) / PLAYER_ROW).floor() as usize).max(1);
        let scroll = self.scroll.min(view.players.len().saturating_sub(fits));
        let more = view.players.len() > fits;
        let list_w = if more { w - TOUCH - 4.0 } else { w };
        out.sunken([x, y, list_w, list_h], LIST_BG);
        if view.players.is_empty() {
            out.text("No other players with voice chat", [x + 6.0, y + 8.0], LIST_GREY);
        }
        let list_top = y;
        y += 1.0;
        for p in view.players.iter().skip(scroll).take(fits) {
            let right = x + list_w - 4.0;
            // The face: green framed while the player talks.
            let (hx, hy) = (x + 6.0, y + 6.0);
            out.fill([hx - 1.0, hy - 1.0, 18.0, 18.0], if p.talking { 0xFF55_FF55 } else { BLACK });
            for (r, argb) in &p.head {
                out.fill([hx + r[0] * 2.0, hy + r[1] * 2.0, r[2] * 2.0, r[3] * 2.0], *argb);
            }
            // Right to left: [Copy] [-] 100% [+] [Mute].
            let mute = [right - 38.0, y + 5.0, 38.0, TOUCH];
            let plus = [mute[0] - 22.0, y + 5.0, TOUCH, TOUCH];
            let vol_x = plus[0] - 32.0;
            let minus = [vol_x - 20.0, y + 5.0, TOUCH, TOUCH];
            let copy = [minus[0] - 42.0, y + 5.0, 38.0, TOUCH];
            let name_w = (copy[0] - (x + 29.0) - 4.0).max(20.0);
            out.text(&out.fit(&p.name, name_w), [x + 29.0, y + 6.0], 0xFF_FFFF);
            let (sub, rgb) = if p.talking {
                (if p.whisper { "Whispering".to_owned() } else { "Talking".to_owned() }, 0x55_FF55)
            } else if p.voice_off {
                ("Voice chat off".to_owned(), 0xFF_5555)
            } else if p.mic_muted {
                ("Mic muted".to_owned(), 0xFF_5555)
            } else if let Some(d) = p.distance {
                (format!("{} m{}", d.round() as i64, if p.in_group { " - your group" } else { "" }), LIST_GREY)
            } else {
                (if p.in_group { "Your group".to_owned() } else { "Far away".to_owned() }, LIST_GREY)
            };
            out.text(&out.fit(&sub, name_w), [x + 29.0, y + 17.0], rgb);
            out.button(copy, "Copy", Action::CopyVoice(p.id), p.copied.then_some(true));
            out.button(minus, "-", Action::PlayerStep(p.id, -10), None);
            let vol = format!("{}%", if p.muted { 0 } else { p.volume });
            let vw = out.width(&vol);
            out.text(&vol, [vol_x + (30.0 - vw) / 2.0, y + 11.0], if p.muted { 0xFF_5555 } else { 0xFF_FFFF });
            out.button(plus, "+", Action::PlayerStep(p.id, 10), None);
            out.button(mute, "Mute", Action::PlayerMute(p.id), p.muted.then_some(false));
            y += PLAYER_ROW;
        }
        if more {
            out.button([x + w - TOUCH, list_top, TOUCH, TOUCH], "^", Action::Scroll(-1), None);
            out.button([x + w - TOUCH, list_top + list_h - TOUCH, TOUCH, TOUCH], "v", Action::Scroll(1), None);
        }
    }

    fn draw_groups(&self, out: &mut Out<'_>, view: &View, x: f32, top: f32, w: f32, bottom: f32) {
        let mut y = top;
        if !view.connected || !view.groups_enabled {
            let why = if view.connected { "Groups are off on this server" } else { "Voice chat is not connected" };
            out.ink(&out.fit(why, w), [x, y + 2.0], LABEL);
            return;
        }
        y += TOUCH + GAP;
        // The create form at the bottom: a label, two fields, two buttons.
        let form_top = bottom - (12.0 + 2.0 + TOUCH + GAP + TOUCH);
        let list_h = (form_top - 4.0 - y).max(ROW + 2.0);
        let fits = ((list_h - 2.0) / ROW).floor().max(1.0) as usize;
        let scroll = self.scroll.min(view.groups.len().saturating_sub(fits));
        let more = view.groups.len() > fits;
        // The header row: the group, the two scroll arrows when the list is
        // longer than its box, and Leave.
        let hy = top;
        let mut right = x + w;
        if view.groups.iter().any(|g| g.ours) {
            right -= 58.0;
            out.button([right, hy, 58.0, TOUCH], "Leave", Action::Leave, None);
            right -= 4.0;
        }
        if more {
            right -= TOUCH;
            out.button([right, hy, TOUCH, TOUCH], "v", Action::Scroll(1), None);
            right -= TOUCH + 2.0;
            out.button([right, hy, TOUCH, TOUCH], "^", Action::Scroll(-1), None);
            right -= 4.0;
        }
        let head = match view.groups.iter().find(|g| g.ours) {
            Some(ours) => (format!("Your group: {}", ours.name), 0x00_8A00),
            None => ("Pick a group to join".to_owned(), LABEL),
        };
        out.ink(&out.fit(&head.0, right - x), [x, hy + 6.0], head.1);
        let list_w = w;
        out.sunken([x, y, list_w, list_h], LIST_BG);
        if view.groups.is_empty() {
            out.text("No groups yet. Create one below", [x + 6.0, y + 8.0], LIST_GREY);
        }
        y += 1.0;
        for g in view.groups.iter().skip(scroll).take(fits) {
            let row = [x + 1.0, y, list_w - 2.0, ROW];
            if g.ours {
                out.fill(row, WHITE);
                out.fill([row[0] + 1.0, row[1] + 1.0, row[2] - 2.0, row[3] - 2.0], BLACK);
            }
            if self.joining == Some(g.id) {
                let fw = list_w - 128.0;
                out.field([x + 3.0, y + 3.0, fw, TOUCH], &self.join_password, &format!("password for {}", g.name), true, self.focus == Some(Field::JoinPassword), Action::Focus(Field::JoinPassword));
                out.button([x + fw + 6.0, y + 3.0, 40.0, TOUCH], "Join", Action::JoinWithPassword, None);
                out.button([x + fw + 48.0, y + 3.0, 52.0, TOUCH], "Cancel", Action::CancelJoin, None);
            } else {
                let lock = if g.has_password { " (password)" } else { "" };
                let room = if g.ours { list_w - 12.0 } else { list_w - 64.0 };
                out.text(&out.fit(&format!("{}{lock}", g.name), room), [x + 6.0, y + 3.0], 0xFF_FFFF);
                let members = if g.members.is_empty() { "no members yet".to_owned() } else { g.members.join(", ") };
                out.text(&out.fit(&members, room), [x + 6.0, y + 14.0], LIST_GREY);
                if !g.ours {
                    out.button([x + list_w - 56.0, y + 3.0, 52.0, TOUCH], "Join", Action::Join(g.id), None);
                }
            }
            y += ROW;
        }
        out.ink("Create a group", [x, form_top + 2.0], INK);
        let fy = form_top + 14.0;
        let half = (w - 4.0) / 2.0;
        out.field([x, fy, half, TOUCH], &self.name, "group name", false, self.focus == Some(Field::Name), Action::Focus(Field::Name));
        out.field([x + half + 4.0, fy, half, TOUCH], &self.password, "password (optional)", true, self.focus == Some(Field::Password), Action::Focus(Field::Password));
        let by = fy + TOUCH + GAP;
        out.button([x, by, half, TOUCH], &format!("Type: {}", KINDS[self.kind as usize % 3]), Action::Kind, None);
        out.button([x + half + 4.0, by, half, TOUCH], "Create", Action::Create, None);
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
            Action::Tab(tab) => {
                self.tab = tab;
                self.focus = None;
                self.scroll = 0;
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
            settings: Settings { mode: Mode::Voice, ..Settings::default() },
            level_db: -30.0,
            connected: true,
            groups_enabled: true,
            players: vec![
                PlayerRow { id: 7, name: "Alex".into(), head: look::head_rects(None, "Alex"), talking: true, whisper: false, mic_muted: false, voice_off: false, in_group: false, distance: Some(12.0), volume: 100, muted: false, copied: false },
                PlayerRow { id: 8, name: "Bea".into(), head: look::head_rects(None, "Bea"), talking: false, whisper: false, mic_muted: true, voice_off: false, in_group: true, distance: None, volume: 150, muted: false, copied: false },
            ],
            loopback: false,
            rec: Default::default(),
            groups: vec![
                GroupRow { id: 1, name: "Builders".into(), has_password: false, members: vec!["Alex".into()], ours: false },
                GroupRow { id: 2, name: "Secret".into(), has_password: true, members: vec![], ours: false },
            ],
            hotkeys: vec![("Open this menu".into(), "V".into()), ("Push to talk".into(), "Caps Lock".into()), ("Whisper".into(), "not set".into())],
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
    fn every_tab_fits_and_shows_its_parts() {
        let measure = |t: &str| t.len() as f32 * 6.0;
        for size in [[480.0, 270.0], [360.0, 190.0], [320.0, 180.0]] {
            let mut m = Menu::default();
            m.open(Some('v'));
            for (tab, wants) in [
                (Tab::Voice, vec!["Voice volume", "Mic gain", "Mic threshold", "Mic test", "Record", "Play", "Delete"]),
                (Tab::Players, vec!["Players with voice chat: 2", "Alex", "Talking", "Mic muted", "150%"]),
                (Tab::Groups, vec!["Builders", "Alex", "Secret (password)", "Create a group"]),
                (Tab::More, vec!["Hotkeys", "Push to talk", "Caps Lock", "not set"]),
            ] {
                m.act(Action::Tab(tab));
                let t = texts(&m.draw(&view(), size, [0.0, 0.0], &measure));
                // A short screen scrolls, so it shows only part of each page.
                for want in wants.into_iter().filter(|_| size[1] > 260.0) {
                    assert!(t.iter().any(|x| x == want), "{want} missing from {t:?}");
                }
                assert!(
                    m.widgets.iter().all(|w| w.rect[0] >= 0.0 && w.rect[1] >= 0.0 && w.rect[0] + w.rect[2] <= size[0] && w.rect[1] + w.rect[3] <= size[1]),
                    "{tab:?} off screen at {size:?}"
                );
                // Nothing may touch the footer line.
                let foot = (size[1] + (size[1] - 8.0).min(270.0)) / 2.0 - 17.0;
                assert!(
                    m.widgets.iter().all(|w| w.rect[1] + w.rect[3] <= foot + 0.01),
                    "{tab:?} runs into the footer at {size:?}"
                );
            }
        }
        let mut m = Menu::default();
        m.act(Action::Tab(Tab::Groups));
        m.draw(&view(), [480.0, 270.0], [0.0, 0.0], &measure);
        let join = centre(&m, &Action::Join(1));
        assert_eq!(m.hit(join), Some(Action::Join(1)));
    }

    #[test]
    fn a_short_screen_scrolls_to_the_rest_of_a_page() {
        let measure = |t: &str| t.len() as f32 * 6.0;
        let mut m = Menu::default();
        m.open(Some('v'));
        let size = [360.0, 190.0];
        let shown = |m: &mut Menu| texts(&m.draw(&view(), size, [0.0, 0.0], &measure));
        let t = shown(&mut m);
        assert!(t.contains(&"Voice volume".to_owned()) && !t.contains(&"Mic test".to_owned()), "{t:?}");
        for _ in 0..6 {
            let down = centre(&m, &Action::Scroll(1));
            let act = m.hit(down).unwrap();
            m.act(act);
            m.draw(&view(), size, [0.0, 0.0], &measure);
        }
        let t = shown(&mut m);
        assert!(t.contains(&"Mic test".to_owned()), "{t:?}");
        // Every control is a fingertip big.
        assert!(m.widgets.iter().all(|w| w.rect[3] >= 20.0 || matches!(w.action, Action::Slide(..))), "{:?}", m.widgets);
    }

    #[test]
    fn a_slider_click_gives_its_place_on_the_bar() {
        let mut m = Menu::default();
        let measure = |t: &str| t.len() as f32 * 6.0;
        m.draw(&view(), [480.0, 270.0], [0.0, 0.0], &measure);
        let bar = m.widgets.iter().find(|w| matches!(w.action, Action::Slide(Slider::Volume, _))).unwrap().rect;
        let Some(Action::Slide(Slider::Volume, f)) = m.hit([bar[0] + bar[2] * 0.75, bar[1] + 2.0]) else { panic!() };
        assert!((f - 0.75).abs() < 0.01, "{f}");
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
