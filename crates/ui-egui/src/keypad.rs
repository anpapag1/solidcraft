//! The on-screen number pad for dimension and value fields, while a touchscreen or pen is in
//! use ([`crate::touch`]): no keyboard needed to type a length, an angle or an expression.
//!
//! Every expression field (a dimension's value, a tool's value in the view, a dialog's distance)
//! offers itself through [`offer`] while it has the keyboard; [`show`] then draws the pad under
//! it, with a copy of the field's text on its handle. Dragging the handle moves the pad (and that
//! copy) anywhere, out of the way of the drawing, and it stays there for later fields.
//!
//! The pad's presses never reach egui: [`filter`] takes them out of the frame's input and types
//! the key into the field instead, so the field keeps the keyboard (a press anywhere else would
//! take it away, which ends a dimension's editing).

use std::cell::RefCell;

use egui::{Event, Pos2, Rect, Vec2, pos2, vec2};

use crate::SolveApp;
use crate::theme::Tokens;
use crate::ui_kit::{Container, Surface};

/// A key of the pad: what it shows and what it types.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Key {
    Text(&'static str, &'static str),
    Backspace,
    Enter,
}

/// The keys, row by row, with how many columns each spans.
const ROWS: [&[(Key, usize)]; 4] = [
    &[(Key::Text("7", "7"), 1), (Key::Text("8", "8"), 1), (Key::Text("9", "9"), 1), (Key::Text("(", "("), 1), (Key::Text(")", ")"), 1)],
    &[(Key::Text("4", "4"), 1), (Key::Text("5", "5"), 1), (Key::Text("6", "6"), 1), (Key::Text("×", "*"), 1), (Key::Text("÷", "/"), 1)],
    &[(Key::Text("1", "1"), 1), (Key::Text("2", "2"), 1), (Key::Text("3", "3"), 1), (Key::Text("+", "+"), 1), (Key::Text("−", "-"), 1)],
    &[(Key::Text("0", "0"), 1), (Key::Text(".", "."), 1), (Key::Backspace, 1), (Key::Enter, 2)],
];
const COLUMNS: usize = 5;
/// A key's size and the gap between keys (points).
const KEY: Vec2 = vec2(46.0, 40.0);
const GAP: f32 = 4.0;
/// The handle row on top: it holds the copy of the field's text and drags the pad.
const HANDLE: f32 = 30.0;
const PAD: f32 = 6.0;

/// The field that has the keyboard this frame.
#[derive(Clone, Debug)]
struct Offer {
    id: egui::Id,
    rect: Rect,
    text: String,
}

/// The pad as drawn last frame: where its handle and keys are.
#[derive(Clone, Debug)]
struct Layout {
    whole: Rect,
    handle: Rect,
    keys: Vec<(Rect, Key)>,
}

impl Layout {
    fn key_at(&self, p: Pos2) -> Option<Key> {
        self.keys.iter().find(|(r, _)| r.contains(p)).map(|(_, k)| *k)
    }
}

/// A press on the pad, until its release.
#[derive(Clone, Copy, Debug)]
enum Press {
    Key(Option<Key>),
    /// The handle, held this far from the pad's corner.
    Handle(Vec2),
}

#[derive(Debug, Default)]
struct State {
    offer: Option<Offer>,
    layout: Option<Layout>,
    press: Option<Press>,
    /// Where the pad was dragged (its top-left corner); `None`: under the field.
    moved: Option<Pos2>,
    /// Under which field, and where, the pad was put when that field took the keyboard: a
    /// tool's value box follows the pointer, and the pad must not chase it.
    anchor: Option<(egui::Id, Pos2)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// An expression field has the keyboard: the pad (if shown) types into it.
pub(crate) fn offer(resp: &egui::Response, text: &str) {
    STATE.with(|s| s.borrow_mut().offer = Some(Offer { id: resp.id, rect: resp.rect, text: text.to_owned() }));
}

/// The pad covers this point (a finger there is the mouse, not the camera).
pub(crate) fn covers(p: Pos2) -> bool {
    STATE.with(|s| s.borrow().layout.as_ref().is_some_and(|l| l.whole.contains(p)))
}

/// Is the pad on screen?
#[cfg(test)]
pub(crate) fn shown() -> bool {
    STATE.with(|s| s.borrow().layout.is_some())
}

/// Take this frame's presses on the pad out of `raw`, and type their keys instead.
pub(crate) fn filter(raw: &mut egui::RawInput) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(layout) = s.layout.clone() else {
            s.press = None;
            return;
        };
        let events = std::mem::take(&mut raw.events);
        let mut out = Vec::with_capacity(events.len());
        // A finger's release on the pad comes with the pointer going away: that goes too.
        let mut released = false;
        for event in events {
            match event {
                Event::PointerButton { pos, button, pressed: true, .. } if layout.whole.contains(pos) => {
                    if button == egui::PointerButton::Primary {
                        s.press =
                            Some(if layout.handle.contains(pos) { Press::Handle(pos - layout.whole.min) } else { Press::Key(layout.key_at(pos)) });
                    }
                }
                Event::PointerMoved(pos) if pos.is_finite() => match s.press {
                    Some(Press::Handle(grab)) => s.moved = Some(pos - grab),
                    Some(Press::Key(_)) => {}
                    None if layout.whole.contains(pos) => {}
                    None => out.push(event),
                },
                Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, .. } if s.press.is_some() => {
                    if let Some(Press::Key(Some(key))) = s.press
                        && layout.key_at(pos) == Some(key)
                    {
                        crate::debug_log::push(format!("number pad: {key:?}"));
                        out.extend(typed(key));
                    }
                    s.press = None;
                    released = true;
                }
                Event::PointerButton { pos, pressed: false, .. } if layout.whole.contains(pos) => {}
                Event::PointerGone if released => {}
                other => out.push(other),
            }
        }
        raw.events = out;
    });
}

/// The events that type `key` into the field with the keyboard.
fn typed(key: Key) -> Vec<Event> {
    let press = |key: egui::Key| {
        [true, false].map(|pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE })
    };
    match key {
        Key::Text(_, text) => vec![Event::Text(text.to_owned())],
        Key::Backspace => press(egui::Key::Backspace).to_vec(),
        Key::Enter => press(egui::Key::Enter).to_vec(),
    }
}

/// Draw the pad under the field that has the keyboard, while touch or a pen is in use. Call it
/// once a frame, after everything else is drawn.
pub(crate) fn show(app: &SolveApp, ctx: &egui::Context) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let offer = s.offer.take().filter(|o| app.touch.in_use && ctx.memory(|m| m.has_focus(o.id)));
        let Some(offer) = offer else {
            if s.layout.is_some() {
                crate::debug_log::push("number pad hidden (no value field has the keyboard, or the mouse is in use)");
            }
            s.layout = None;
            s.anchor = None;
            return;
        };
        if s.layout.is_none() {
            crate::debug_log::push(format!("number pad shown (field text \"{}\")", offer.text));
        }
        let t = Tokens::get();
        let size =
            vec2(PAD * 2.0 + KEY.x * COLUMNS as f32 + GAP * (COLUMNS - 1) as f32, PAD * 2.0 + HANDLE + GAP + (KEY.y + GAP) * ROWS.len() as f32 - GAP);
        let screen = ctx.content_rect();
        let under = match s.anchor {
            Some((id, at)) if id == offer.id => at,
            _ => offer.rect.left_bottom() + vec2(0.0, 6.0),
        };
        s.anchor = Some((offer.id, under));
        let wanted = s.moved.unwrap_or(under);
        let at = pos2(
            wanted.x.clamp(screen.left(), (screen.right() - size.x).max(screen.left())),
            wanted.y.clamp(screen.top(), (screen.bottom() - size.y).max(screen.top())),
        );
        if s.moved.is_some() {
            s.moved = Some(at);
        }
        let whole = Rect::from_min_size(at, size);
        let handle = Rect::from_min_size(at + vec2(PAD, PAD), vec2(size.x - PAD * 2.0, HANDLE));
        let mut keys = Vec::new();
        let mut y = handle.bottom() + GAP;
        for row in ROWS {
            let mut x = at.x + PAD;
            for (key, span) in row {
                let w = KEY.x * *span as f32 + GAP * (*span as f32 - 1.0);
                keys.push((Rect::from_min_size(pos2(x, y), vec2(w, KEY.y)), *key));
                x += w + GAP;
            }
            y += KEY.y + GAP;
        }
        let held = match s.press {
            Some(Press::Key(k)) => k,
            _ => None,
        };
        let dragging = matches!(s.press, Some(Press::Handle(_)));
        Container::new(ctx, Surface::Canvas).area(egui::Id::new("sc_keypad")).fixed_pos(at).order(egui::Order::Tooltip).show(ctx, |ui| {
            let (_, painter) = ui.allocate_painter(size, egui::Sense::hover());
            painter.rect_filled(whole, t.radius_md, t.dialog_body);
            painter.rect_stroke(whole, t.radius_md, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            // The handle: grip dots, then the field's text with a cursor at its end.
            painter.rect_filled(handle, t.radius_sm, if dragging { t.hover } else { t.control_fill });
            for i in 0..3 {
                for j in 0..2 {
                    painter.circle_filled(handle.left_center() + vec2(10.0 + 5.0 * j as f32, -5.0 + 5.0 * i as f32), 1.5, t.text_dim);
                }
            }
            let text = format!("{}▏", offer.text);
            let galley = painter.layout_no_wrap(text, egui::FontId::monospace(t.font_heading), t.text);
            let room = handle.width() - 30.0;
            // A long expression shows its end, where typing goes.
            let left = handle.left() + 24.0 - (galley.size().x - room).max(0.0);
            painter.with_clip_rect(handle.shrink(2.0)).galley(pos2(left, handle.center().y - galley.size().y / 2.0), galley, t.text);
            for (rect, key) in &keys {
                let down = held == Some(*key);
                let fill = match (key, down) {
                    (Key::Enter, false) => t.accent,
                    (Key::Enter, true) => t.accent_soft,
                    (_, true) => t.hover,
                    (_, false) => t.control_fill,
                };
                painter.rect_filled(*rect, t.radius_sm, fill);
                match key {
                    Key::Text(label, _) => {
                        painter.text(rect.center(), egui::Align2::CENTER_CENTER, *label, egui::FontId::proportional(t.font_article_heading), t.text);
                    }
                    Key::Backspace => backspace_icon(&painter, rect.center(), t.text),
                    Key::Enter => enter_icon(&painter, rect.center(), t.on_accent),
                }
            }
        });
        s.layout = Some(Layout { whole, handle, keys });
        // The pad is the keyboard: the browser must not slide its own over the view.
        ctx.output_mut(|o| o.ime = None);
    });
}

/// A tag pointing left with a cross in it (the fonts have no backspace sign).
fn backspace_icon(painter: &egui::Painter, c: Pos2, colour: egui::Color32) {
    let stroke = egui::Stroke::new(1.6, colour);
    let outline = [c + vec2(-13.0, 0.0), c + vec2(-6.0, -8.0), c + vec2(12.0, -8.0), c + vec2(12.0, 8.0), c + vec2(-6.0, 8.0), c + vec2(-13.0, 0.0)];
    painter.add(egui::Shape::line(outline.to_vec(), stroke));
    let x = c + vec2(3.0, 0.0);
    painter.line_segment([x + vec2(-4.0, -4.0), x + vec2(4.0, 4.0)], stroke);
    painter.line_segment([x + vec2(-4.0, 4.0), x + vec2(4.0, -4.0)], stroke);
}

/// A return arrow: down from the top right, then left to an arrowhead.
fn enter_icon(painter: &egui::Painter, c: Pos2, colour: egui::Color32) {
    let stroke = egui::Stroke::new(2.0, colour);
    let corner = c + vec2(10.0, 5.0);
    let tip = c + vec2(-11.0, 5.0);
    painter.add(egui::Shape::line(vec![c + vec2(10.0, -8.0), corner, tip], stroke));
    painter.add(egui::Shape::line(vec![tip + vec2(5.0, -5.0), tip, tip + vec2(5.0, 5.0)], stroke));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_layout() -> Layout {
        let whole = Rect::from_min_size(pos2(100.0, 100.0), vec2(250.0, 220.0));
        let handle = Rect::from_min_size(pos2(106.0, 106.0), vec2(238.0, 30.0));
        let keys = vec![
            (Rect::from_min_size(pos2(106.0, 140.0), vec2(46.0, 40.0)), Key::Text("7", "7")),
            (Rect::from_min_size(pos2(156.0, 140.0), vec2(46.0, 40.0)), Key::Text("×", "*")),
            (Rect::from_min_size(pos2(206.0, 140.0), vec2(46.0, 40.0)), Key::Backspace),
            (Rect::from_min_size(pos2(256.0, 140.0), vec2(90.0, 40.0)), Key::Enter),
        ];
        Layout { whole, handle, keys }
    }

    fn frame(events: Vec<Event>) -> Vec<Event> {
        let mut raw = egui::RawInput { events, ..Default::default() };
        filter(&mut raw);
        raw.events
    }

    fn button(pressed: bool, at: Pos2) -> Event {
        Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
    }

    fn reset(layout: Option<Layout>) {
        STATE.with(|s| *s.borrow_mut() = State { layout, ..Default::default() });
    }

    #[test]
    fn a_tap_on_a_key_types_it_and_egui_never_sees_the_press() {
        reset(Some(with_layout()));
        let seven = pos2(120.0, 160.0);
        let out = frame(vec![Event::PointerMoved(seven), button(true, seven)]);
        assert!(out.is_empty(), "{out:?}");
        let out = frame(vec![Event::PointerMoved(seven), button(false, seven), Event::PointerGone]);
        assert_eq!(out, vec![Event::Text("7".into())]);
        let times = pos2(170.0, 160.0);
        frame(vec![button(true, times)]);
        assert_eq!(frame(vec![button(false, times)]), vec![Event::Text("*".into())]);
        let back = pos2(220.0, 160.0);
        frame(vec![button(true, back)]);
        let out = frame(vec![button(false, back)]);
        assert!(matches!(out.as_slice(), [Event::Key { key: egui::Key::Backspace, pressed: true, .. }, Event::Key { pressed: false, .. }]));
        let enter = pos2(300.0, 160.0);
        frame(vec![button(true, enter)]);
        let out = frame(vec![button(false, enter)]);
        assert!(matches!(out.first(), Some(Event::Key { key: egui::Key::Enter, pressed: true, .. })));
    }

    #[test]
    fn sliding_off_a_key_types_nothing() {
        reset(Some(with_layout()));
        frame(vec![button(true, pos2(120.0, 160.0))]);
        assert!(frame(vec![Event::PointerMoved(pos2(220.0, 160.0)), button(false, pos2(220.0, 160.0))]).is_empty());
    }

    #[test]
    fn the_handle_drags_the_pad() {
        reset(Some(with_layout()));
        frame(vec![button(true, pos2(110.0, 110.0))]);
        // Moves outside the pad drag it and are kept from the view under them.
        assert!(frame(vec![Event::PointerMoved(pos2(410.0, 510.0))]).is_empty());
        frame(vec![button(false, pos2(410.0, 510.0))]);
        assert_eq!(STATE.with(|s| s.borrow().moved), Some(pos2(400.0, 500.0)));
    }

    #[test]
    fn input_off_the_pad_or_without_it_passes() {
        reset(Some(with_layout()));
        let elsewhere =
            vec![Event::PointerMoved(pos2(20.0, 20.0)), button(true, pos2(20.0, 20.0)), button(false, pos2(20.0, 20.0)), Event::PointerGone];
        assert_eq!(frame(elsewhere.clone()), elsewhere);
        reset(None);
        let on = vec![button(true, pos2(120.0, 160.0)), button(false, pos2(120.0, 160.0))];
        assert_eq!(frame(on.clone()), on);
    }

    /// The pad shows under a dimension's value only while touch or a pen is in use, and a tap
    /// on its keys types into the value without taking its keyboard away.
    #[test]
    fn the_pad_types_into_a_dimension() {
        use serde_json::json;
        reset(None);
        let mut h = crate::scenario::Harness::new();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.line", json!({"points":[[0,0],[40,0]],"infer":false})).unwrap();
        let param =
            h.app.run("sketch.dimension", json!({"entities":["l1"],"value":40,"text_at":[20,8]})).unwrap()["param"].as_str().unwrap().to_string();
        h.settle();
        crate::dim_view::edit_param(&h.app, &param);
        h.frames(5);
        assert!(crate::dim_view::editing());
        assert!(!shown(), "no pad with a mouse");
        h.app.touch.in_use = true;
        h.frames(2);
        assert!(shown(), "the pad shows for touch");
        let keys = STATE.with(|s| s.borrow().layout.clone()).map(|l| l.keys).unwrap_or_default();
        let at = |k: &str| keys.iter().find(|(_, key)| matches!(key, Key::Text(l, _) if *l == k)).map(|(r, _)| r.center());
        let enter = keys.iter().find(|(_, k)| *k == Key::Enter).map(|(r, _)| r.center());
        for k in ["2", "5", "×", "2"] {
            let Some(p) = at(k) else { panic!("no key {k}") };
            h.step(&json!({"click": [p.x, p.y]})).unwrap();
            assert!(crate::dim_view::editing(), "the value keeps the keyboard after {k}");
        }
        let Some(p) = enter else { panic!("no Enter") };
        h.step(&json!({"click": [p.x, p.y]})).unwrap();
        h.settle();
        assert!(!crate::dim_view::editing(), "Enter applies the value");
        assert_eq!(h.app.session.doc.param(&param).map(|p| p.expr.replace(' ', "")), Some("25*2".to_string()));
    }
}
