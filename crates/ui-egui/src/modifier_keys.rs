//! On-screen modifier keys for touchscreens and pens ([`crate::touch`]): a dropdown on the
//! application bar, beside Undo and Redo, for the keys a tablet has no keyboard to hold.
//!
//! - **Shift, Ctrl / Cmd, Alt** latch: a tap turns one on, and it stays on until it is tapped
//!   again. While on, every click, drag, wheel turn and key press the app sees carries it, so
//!   Shift-click adds to a selection and Ctrl-click toggles one.
//! - **Space** latches the same way, as a held key.
//! - **Escape** is a single press: it cancels the running tool or closes what is open.
//!
//! The button and the keys exist only while touch or a pen is in use. When the mouse takes over
//! (or touch mode ends) every latch is let go, so a hand back on the mouse is never stuck holding
//! Ctrl.

use egui::{Event, Key, Modifiers, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};

use crate::SolveApp;
use crate::theme::Tokens;
use crate::ui_kit::{Container, MenuItem, Surface};

/// A key of the dropdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Shift,
    /// Ctrl, or Cmd on a Mac.
    Command,
    Alt,
    Space,
    Escape,
}

/// The dropdown's rows, top to bottom, and what each is called.
const ENTRIES: [(Entry, &str); 5] =
    [(Entry::Shift, "Shift"), (Entry::Command, "Ctrl / Cmd"), (Entry::Alt, "Alt"), (Entry::Space, "Space"), (Entry::Escape, "Escape")];

/// Which keys are latched, whether the dropdown is open, and the key events waiting for the
/// next frame.
#[derive(Debug, Default)]
pub(crate) struct State {
    shift: bool,
    command: bool,
    alt: bool,
    space: bool,
    /// The dropdown is open.
    pub(crate) open: bool,
    /// Where the dropdown goes (under the bar button), and where that button is.
    anchor: Pos2,
    button: Option<Rect>,
    /// The latched modifiers egui was last told about (through `ModifiersChanged`).
    reported: Modifiers,
    /// The button opened the dropdown this frame: the same press must not close it again.
    just_opened: bool,
    /// Key events for the next frame (Space going down or up, an Escape press).
    pending: Vec<Event>,
}

impl State {
    /// Is `entry` latched? (Escape never is.)
    pub(crate) fn latched(&self, entry: Entry) -> bool {
        match entry {
            Entry::Shift => self.shift,
            Entry::Command => self.command,
            Entry::Alt => self.alt,
            Entry::Space => self.space,
            Entry::Escape => false,
        }
    }

    /// Is any key latched?
    pub(crate) fn any(&self) -> bool {
        self.shift || self.command || self.alt || self.space
    }

    /// The modifiers the latched keys add. Ctrl / Cmd sets both `ctrl` and `command`, so
    /// shortcuts written for either see it.
    pub(crate) fn modifiers(&self) -> Modifiers {
        Modifiers { alt: self.alt, ctrl: self.command, shift: self.shift, mac_cmd: false, command: self.command }
    }

    /// A tap on `entry`: latch or unlatch it (Escape is pressed once).
    pub(crate) fn press(&mut self, entry: Entry) {
        match entry {
            Entry::Shift => self.shift = !self.shift,
            Entry::Command => self.command = !self.command,
            Entry::Alt => self.alt = !self.alt,
            Entry::Space => {
                self.space = !self.space;
                self.pending.push(key_event(Key::Space, self.space));
            }
            Entry::Escape => {
                self.pending.push(key_event(Key::Escape, true));
                self.pending.push(key_event(Key::Escape, false));
            }
        }
        crate::debug_log::push(format!("modifier keys: {entry:?} (now {:?})", self.modifiers()));
    }

    /// Let every key go (touch mode ended).
    pub(crate) fn release_all(&mut self) {
        if self.space {
            self.pending.push(key_event(Key::Space, false));
        }
        *self =
            Self { pending: std::mem::take(&mut self.pending), anchor: self.anchor, button: self.button, reported: self.reported, ..Self::default() };
        crate::debug_log::push("modifier keys released");
    }

    /// Rewrite one frame of raw input: add the latched modifiers to the frame and to every
    /// event that carries them, and deliver the waiting key events. `in_use`: touch or a pen is
    /// in use (else every latch is let go).
    pub(crate) fn apply(&mut self, raw: &mut egui::RawInput, in_use: bool) {
        if !in_use && (self.any() || self.open) {
            self.release_all();
        }
        let m = self.modifiers();
        if m != Modifiers::NONE {
            for event in &mut raw.events {
                match event {
                    Event::PointerButton { modifiers, .. }
                    | Event::Key { modifiers, .. }
                    | Event::MouseWheel { modifiers, .. }
                    | Event::ModifiersChanged(modifiers) => *modifiers = modifiers.plus(m),
                    _ => {}
                }
            }
        }
        // egui learns the held modifiers from `ModifiersChanged`: tell it when the latches change
        // (a platform's own report, patched above, already says it).
        let told = raw.events.iter().any(|e| matches!(e, Event::ModifiersChanged(_)));
        if m != self.reported {
            if !told {
                raw.events.insert(0, Event::ModifiersChanged(m));
            }
            self.reported = m;
        }
        for event in self.pending.drain(..).take(16) {
            // A key press carries the modifiers held with it.
            let event = match event {
                Event::Key { key, physical_key, pressed, repeat, modifiers } => {
                    Event::Key { key, physical_key, pressed, repeat, modifiers: modifiers.plus(m) }
                }
                other => other,
            };
            raw.events.push(event);
        }
    }
}

/// A key going down or up, with no modifiers of its own.
fn key_event(key: Key, pressed: bool) -> Event {
    Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }
}

/// The dropdown's button on the application bar, centred on `at`, `width` points wide. It shows
/// only while touch or a pen is in use. Tapping it opens or closes the dropdown.
pub(crate) fn bar_button(app: &mut SolveApp, ui: &mut egui::Ui, bar: Rect, at: f32, width: f32) {
    let t = Tokens::get();
    let br = Rect::from_center_size(pos2(at, bar.center().y), vec2(width, 32.0));
    let resp = ui.interact(br, ui.id().with(("ab", "modifiers")), egui::Sense::click());
    let label = crate::i18n::tr_label("Modifier keys").to_string();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
    crate::scenario::publish_target("appbar:modifiers", br);
    let keys = &mut app.touch.keys;
    keys.button = Some(br);
    if resp.hovered() || resp.has_focus() || keys.open {
        ui.painter().rect_filled(br.shrink(3.0), t.radius_sm, t.app_bar_hover_button);
    }
    let wide = width >= 30.0;
    let cap = Rect::from_center_size(pos2(br.center().x - if wide { 4.0 } else { 0.0 }, br.center().y), vec2(16.0, 16.0));
    let active = keys.any();
    let ink = if active { t.on_accent } else { t.app_bar_text };
    if active {
        ui.painter().rect_filled(cap, t.radius_sm, t.accent);
    } else {
        ui.painter().rect_stroke(cap, t.radius_sm, Stroke::new(1.4, t.app_bar_text), StrokeKind::Inside);
    }
    // An up arrow on the key cap: the shift sign.
    let c = cap.center();
    let stroke = Stroke::new(1.5, ink);
    ui.painter().line_segment([c + vec2(0.0, 4.5), c + vec2(0.0, -4.0)], stroke);
    ui.painter().add(Shape::line(vec![c + vec2(-3.5, -0.5), c + vec2(0.0, -4.5), c + vec2(3.5, -0.5)], stroke));
    if wide {
        // The drop-down caret.
        let k = pos2(br.center().x + 10.0, br.center().y);
        ui.painter().add(Shape::line(vec![k + vec2(-3.0, -1.5), k + vec2(0.0, 1.5), k + vec2(3.0, -1.5)], Stroke::new(1.4, t.app_bar_text)));
    }
    if resp.on_hover_text(label).clicked() {
        keys.open = !keys.open;
        keys.just_opened = keys.open;
        keys.anchor = pos2(br.left(), br.bottom());
    }
}

/// The dropdown under the bar button, while it is open.
pub(crate) fn show_menu(app: &mut SolveApp, ctx: &egui::Context) {
    if !app.touch.keys.open {
        return;
    }
    if !app.touch.in_use {
        app.touch.keys.open = false;
        return;
    }
    let mut pick: Option<Entry> = None;
    let latched: Vec<bool> = ENTRIES.iter().map(|(e, _)| app.touch.keys.latched(*e)).collect();
    let resp = Container::new(ctx, Surface::Popup)
        .area(egui::Id::new("sc_modifier_menu"))
        .fixed_pos(app.touch.keys.anchor)
        .constrain(true)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            crate::context_menu::menu_frame(ui, |ui| {
                for ((entry, label), on) in ENTRIES.iter().zip(&latched) {
                    let mut row = MenuItem::new(label, label, "");
                    // Latching keys show a tick while on; Escape is a plain action.
                    row.checked = (*entry != Entry::Escape).then_some(*on);
                    if row.show(ui).clicked() {
                        pick = Some(*entry);
                    }
                }
            })
        });
    let keys = &mut app.touch.keys;
    if let Some(entry) = pick {
        keys.press(entry);
        // The latching keys stay open for the next one; Escape has done its job.
        if entry == Entry::Escape {
            keys.open = false;
        }
    } else {
        let (pressed, at) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos()));
        let on_button = at.is_some_and(|p| keys.button.is_some_and(|b| b.contains(p)));
        // Pressed elsewhere closes it (the bar button toggles it itself).
        if pressed && !resp.response.contains_pointer() && !on_button && !keys.just_opened {
            keys.open = false;
        }
    }
    keys.just_opened = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn press_event(modifiers: Modifiers) -> Event {
        Event::PointerButton { pos: pos2(10.0, 10.0), button: egui::PointerButton::Primary, pressed: true, modifiers }
    }

    fn frame(state: &mut State, events: Vec<Event>, in_use: bool) -> egui::RawInput {
        let mut raw = egui::RawInput { events, ..Default::default() };
        state.apply(&mut raw, in_use);
        raw
    }

    #[test]
    fn a_latched_shift_rides_on_every_click_until_it_is_tapped_again() {
        let mut s = State::default();
        let raw = frame(&mut s, vec![press_event(Modifiers::NONE)], true);
        assert!(!raw.events.iter().any(|e| matches!(e, Event::ModifiersChanged(_))), "nothing latched: nothing added");
        s.press(Entry::Shift);
        for _ in 0..3 {
            let raw = frame(&mut s, vec![press_event(Modifiers::NONE)], true);
            let Some(Event::PointerButton { modifiers, .. }) = raw.events.last() else { panic!("no click: {:?}", raw.events) };
            assert!(modifiers.shift && !modifiers.ctrl && !modifiers.alt && !modifiers.command, "{modifiers:?}");
        }
        s.press(Entry::Shift);
        let raw = frame(&mut s, vec![press_event(Modifiers::NONE)], true);
        let Some(Event::PointerButton { modifiers, .. }) = raw.events.last() else { panic!("no click: {:?}", raw.events) };
        assert_eq!(*modifiers, Modifiers::NONE);
        assert!(
            matches!(raw.events.first(), Some(Event::ModifiersChanged(m)) if *m == Modifiers::NONE),
            "egui is told Shift went up: {:?}",
            raw.events
        );
    }

    #[test]
    fn egui_is_told_when_the_latches_change_and_not_otherwise() {
        let mut s = State::default();
        s.press(Entry::Shift);
        let raw = frame(&mut s, vec![], true);
        assert!(matches!(raw.events.as_slice(), [Event::ModifiersChanged(m)] if m.shift));
        assert!(frame(&mut s, vec![], true).events.is_empty(), "unchanged: nothing sent");
        // A platform report in the same frame is patched, not doubled.
        s.press(Entry::Alt);
        let raw = frame(&mut s, vec![Event::ModifiersChanged(Modifiers { ctrl: true, ..Modifiers::NONE })], true);
        let [Event::ModifiersChanged(m)] = raw.events.as_slice() else { panic!("{:?}", raw.events) };
        assert!(m.shift && m.alt && m.ctrl, "{m:?}");
    }

    #[test]
    fn ctrl_sets_both_ctrl_and_command_and_keeps_what_the_event_had() {
        let mut s = State::default();
        s.press(Entry::Command);
        s.press(Entry::Alt);
        let held = Modifiers { shift: true, ..Modifiers::NONE };
        let raw = frame(&mut s, vec![Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: held }], true);
        let Some(Event::Key { modifiers, .. }) = raw.events.iter().find(|e| matches!(e, Event::Key { .. })) else {
            panic!("no key event: {:?}", raw.events)
        };
        assert!(modifiers.ctrl && modifiers.command && modifiers.alt && modifiers.shift, "{modifiers:?}");
        assert!(!modifiers.mac_cmd);
    }

    #[test]
    fn the_wheel_and_text_are_handled_without_touching_other_events() {
        let mut s = State::default();
        s.press(Entry::Command);
        let wheel =
            Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, 1.0), phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE };
        let raw = frame(&mut s, vec![wheel, Event::Text("7".into())], true);
        assert!(raw.events.iter().any(|e| matches!(e, Event::MouseWheel { modifiers, .. } if modifiers.command)));
        assert!(raw.events.contains(&Event::Text("7".into())), "text passes untouched: {:?}", raw.events);
    }

    #[test]
    fn space_is_held_down_until_it_is_tapped_again() {
        let mut s = State::default();
        s.press(Entry::Space);
        let raw = frame(&mut s, vec![], true);
        assert!(matches!(raw.events.as_slice(), [Event::Key { key: Key::Space, pressed: true, .. }]));
        assert!(frame(&mut s, vec![], true).events.is_empty(), "held, not repeated");
        s.press(Entry::Space);
        let raw = frame(&mut s, vec![], true);
        assert!(matches!(raw.events.as_slice(), [Event::Key { key: Key::Space, pressed: false, .. }]));
    }

    #[test]
    fn escape_is_one_press_and_never_latches() {
        let mut s = State::default();
        s.press(Entry::Escape);
        assert!(!s.latched(Entry::Escape) && !s.any());
        let raw = frame(&mut s, vec![], true);
        assert!(matches!(
            raw.events.as_slice(),
            [Event::Key { key: Key::Escape, pressed: true, .. }, Event::Key { key: Key::Escape, pressed: false, .. }]
        ));
        assert!(frame(&mut s, vec![], true).events.is_empty());
    }

    #[test]
    fn the_mouse_taking_over_lets_every_key_go() {
        let mut s = State::default();
        for e in [Entry::Shift, Entry::Command, Entry::Alt, Entry::Space] {
            s.press(e);
        }
        s.open = true;
        frame(&mut s, vec![], true);
        let raw = frame(&mut s, vec![press_event(Modifiers::NONE)], false);
        assert!(!s.any() && !s.open);
        assert!(
            raw.events.iter().any(|e| matches!(e, Event::Key { key: Key::Space, pressed: false, .. })),
            "a held Space is released: {:?}",
            raw.events
        );
        assert!(
            matches!(raw.events.first(), Some(Event::ModifiersChanged(m)) if *m == Modifiers::NONE),
            "egui is told every modifier went up: {:?}",
            raw.events
        );
        let click = raw.events.iter().find_map(|e| if let Event::PointerButton { modifiers, .. } = e { Some(*modifiers) } else { None });
        assert_eq!(click, Some(Modifiers::NONE), "the click carries nothing");
    }

    #[test]
    fn a_hostile_pile_of_pending_events_is_bounded() {
        let mut s = State::default();
        for _ in 0..1000 {
            s.press(Entry::Escape);
        }
        assert!(frame(&mut s, vec![], true).events.len() <= 16);
    }

    /// The button shows only with touch or a pen; its dropdown latches Shift, which then
    /// reaches the app as a modifier on the next click, and the mouse taking over lets it go.
    #[test]
    fn the_dropdown_latches_shift_for_the_app_and_the_mouse_releases_it() {
        let mut h = crate::scenario::Harness::new();
        h.frames(3);
        assert!(crate::scenario::handle_at("appbar:modifiers").is_none(), "no button with a mouse");
        h.app.touch.in_use = true;
        h.frames(3);
        let Some(button) = crate::scenario::handle_at("appbar:modifiers") else { panic!("no modifier keys button for touch") };
        assert!(!h.app.touch.keys.open);
        h.step(&json!({"click": [button.x, button.y]})).unwrap();
        h.frames(3);
        assert!(h.app.touch.keys.open, "the button opens the dropdown");
        h.step(&json!({"widget": "Shift"})).unwrap();
        h.frames(2);
        assert!(h.app.touch.keys.latched(Entry::Shift), "a row latches its key");
        assert!(h.app.touch.keys.open, "the dropdown stays open for the next key");
        let seen = h.ctx.input(|i| i.modifiers.shift);
        assert!(seen, "egui sees Shift held");
        h.step(&json!({"widget": "Shift"})).unwrap();
        h.frames(2);
        assert!(!h.app.touch.keys.latched(Entry::Shift) && !h.ctx.input(|i| i.modifiers.shift), "tapping again lets it go");
        h.step(&json!({"widget": "Alt"})).unwrap();
        h.frames(2);
        assert!(h.app.touch.keys.latched(Entry::Alt));
        h.app.touch.in_use = false;
        h.frames(3);
        assert!(!h.app.touch.keys.any() && !h.app.touch.keys.open, "the mouse took over");
        assert!(crate::scenario::handle_at("appbar:modifiers").is_none(), "the button is gone with the mouse");
    }
}
