//! Touchscreens and pens.
//!
//! Where the platform tells pens and fingers apart (the browser: [`DeviceEvent`] through
//! [`crate::SolveApp::device_input`]), the pen is the mouse and fingers are for the camera:
//!
//! - **Pen:** hovers, clicks and drags like a mouse. Its barrel button, or holding it still on
//!   the screen, is a right click. While the pen is down or near the screen, fingers (the palm)
//!   are ignored.
//! - **Fingers on the 3D view:** one finger orbits (pans a flat page), two fingers pan and pinch
//!   to zoom, a two-finger tap undoes and a three-finger tap redoes. A one-finger tap selects
//!   until a pen has been used; after that fingers only move the camera.
//! - **Fingers anywhere else** (toolbar, panels, menus, the axis view): a finger is the mouse,
//!   so buttons and fields work without a pen, and holding it on a control opens its menu.
//!
//! Where the platform reports a finger as the mouse as well as a touch (the desktop app), a
//! [`TouchGate`] keeps that emulated mouse out of two-finger gestures.
//!
//! The camera reads what the fingers did through [`camera_input`], which
//! [`crate::navigation::handle`] calls, so every page with a camera gets the same gestures.

use egui::{Event, Pos2, Rect, TouchDeviceId, TouchId, TouchPhase, Vec2};

/// The touch device id of the pen's own touch (it lets a pen held still open the context
/// menu, as egui does for touches). Fingers use [`FINGER_DEVICE`].
pub const PEN_DEVICE: TouchDeviceId = TouchDeviceId(0x0070_656e);
/// The touch device id of fingers reported through [`DeviceEvent::Finger`].
pub const FINGER_DEVICE: TouchDeviceId = TouchDeviceId(0);

/// How long a first finger's press is held back, waiting to see if a second finger joins it.
const TOUCH_GESTURE_WAIT: f64 = 0.15;
/// How far (points) a finger moves before it counts as a drag rather than a tap.
const TOUCH_SLOP: f32 = 8.0;
/// A tap of one, two or three fingers is shorter than this (seconds)...
const TAP_TIME: f64 = 0.35;
/// ...and no finger of it moves further than this (points).
const TAP_SLOP: f32 = 14.0;
/// Fingers that land this soon (seconds) after the pen was last seen are a resting palm.
const PALM_WINDOW: f64 = 0.6;
/// At most this many fingers are tracked; more are ignored (hostile input stays bounded).
const MAX_TOUCHES: usize = 10;

/// One pen or finger event, from a platform that tells them apart. Positions are in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeviceEvent {
    /// The pen touched the screen; `button` is its tip (primary) or barrel (secondary) button.
    PenDown {
        pos: Pos2,
        button: egui::PointerButton,
    },
    /// The pen moved, touching the screen or hovering over it.
    PenMove {
        pos: Pos2,
    },
    PenUp {
        pos: Pos2,
        button: egui::PointerButton,
    },
    /// The pen left hover range.
    PenLeave,
    /// The platform took the pen's input away (a system gesture).
    PenCancel,
    /// A finger, by the platform's id for it.
    Finger {
        id: u64,
        phase: TouchPhase,
        pos: Pos2,
    },
}

/// What the fingers asked of the camera this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Fingers {
    /// Fingers are moving the camera of the page being drawn.
    pub active: bool,
    /// Two-finger movement, in points: the model follows the fingers.
    pub pan: Vec2,
    /// Pinch factor (1 = none); above 1 zooms in.
    pub zoom: f32,
    /// Where the fingers are (the zoom's anchor).
    pub center: Option<Pos2>,
    /// One-finger drag to orbit, in points, and where that finger came down.
    pub orbit: Vec2,
    pub orbit_from: Option<Pos2>,
    /// A command a multi-finger tap asked for (`edit.undo`, `edit.redo`).
    pub command: Option<&'static str>,
}

/// What a touchscreen's two-or-more-finger gesture asks of the camera in one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TouchGesture {
    /// Pinch factor (1 = fingers neither spread nor closed); above 1 zooms in.
    pub zoom: f32,
    /// How far the fingers' centre moved, in points. The model follows the fingers.
    pub pan: egui::Vec2,
    /// The fingers' centre.
    pub center: Option<Pos2>,
}

impl TouchGesture {
    pub(crate) const NONE: Self = Self { zoom: 1.0, pan: egui::Vec2::ZERO, center: None };
}

/// The camera motion of a touchscreen gesture with two or more fingers, from egui's multi-touch
/// tracking. Non-finite or non-positive values count as no motion.
pub(crate) fn touch_gesture(info: Option<egui::MultiTouchInfo>) -> TouchGesture {
    let Some(info) = info.filter(|i| i.num_touches >= 2) else { return TouchGesture::NONE };
    let zoom = if info.zoom_delta.is_finite() && info.zoom_delta > 0.0 { info.zoom_delta } else { 1.0 };
    let pan = if info.translation_delta.is_finite() { info.translation_delta } else { egui::Vec2::ZERO };
    let center = Some(info.center_pos).filter(|c| c.is_finite());
    TouchGesture { zoom, pan, center }
}

/// Touch and pen state of the app ([`crate::SolveApp::touch`]).
#[derive(Debug, Default)]
pub struct Touch {
    /// The platform reports pens and fingers itself ([`crate::SolveApp::device_input`]); fingers
    /// then never arrive as the mouse.
    pub(crate) raw: bool,
    /// Desktop: the emulated mouse of a finger, held out of two-finger gestures.
    pub(crate) gate: TouchGate,
    router: Router,
    /// A pen has been used: finger taps on the 3D view no longer select.
    pub(crate) pen_seen: bool,
    /// Touch or a pen is in use (no mouse since): fields get the on-screen number pad.
    pub(crate) in_use: bool,
    /// The camera page drawn last frame: its rectangle, egui layer, and the parts of it that are
    /// controls (the axis view, the navigation bar), where a finger is the mouse.
    pub(crate) camera_rect: Option<Rect>,
    pub(crate) camera_layer: Option<egui::LayerId>,
    pub(crate) camera_controls: Vec<Rect>,
    /// The keyboard's modifiers, as last reported (a pen click keeps Shift or Ctrl).
    mods: egui::Modifiers,
}

impl Touch {
    /// Fingers are moving the camera.
    #[cfg(test)]
    pub(crate) fn camera_active(&self) -> bool {
        matches!(self.router.route, Route::Camera { .. })
    }

    /// Rewrite one frame of raw input, adding `events` from the platform. `camera_at` says
    /// whether a finger landing at a point is on a camera page (else it is the mouse).
    pub(crate) fn device_input(&mut self, raw: &mut egui::RawInput, events: Vec<DeviceEvent>, camera_at: &dyn Fn(Pos2) -> bool) {
        self.raw = true;
        let now = raw.time.filter(|t| t.is_finite()).unwrap_or(0.0);
        // Anything the platform still sends as the mouse is a mouse or trackpad.
        if self.in_use && raw.events.iter().any(|e| matches!(e, Event::PointerButton { pressed: true, .. } | Event::MouseWheel { .. })) {
            crate::debug_log::push("mouse or trackpad used: touch mode off");
            self.in_use = false;
        }
        if !self.in_use && !events.is_empty() {
            crate::debug_log::push("pen or finger used: touch mode on");
            self.in_use = true;
        }
        if let Some(m) = raw.events.iter().rev().find_map(|e| if let Event::ModifiersChanged(m) = e { Some(*m) } else { None }) {
            self.mods = m;
        }
        let mods = self.mods;
        let mut out = self.router.later.pop_front().unwrap_or_default();
        for event in events.into_iter().take(256) {
            self.router.event(event, now, mods, camera_at, &mut self.pen_seen, &mut out);
        }
        raw.events.extend(out);
    }

    /// Desktop: rewrite one frame of raw input in which fingers are also the mouse.
    pub(crate) fn emulated_input(&mut self, raw: &mut egui::RawInput) {
        if self.raw {
            return;
        }
        if raw.events.iter().any(|e| matches!(e, Event::Touch { .. })) {
            self.in_use = true;
        } else if raw.events.iter().any(|e| matches!(e, Event::PointerButton { pressed: true, .. } | Event::MouseWheel { .. })) {
            self.in_use = false;
        }
        self.gate.filter(raw);
    }

    /// Events are waiting for a later frame (a synthesized click's release).
    pub(crate) fn waiting(&self) -> bool {
        !self.router.later.is_empty() || self.gate.waiting()
    }

    /// How big handles are and how far from one a press still grabs it, now.
    pub(crate) fn sizes(&self) -> Sizes {
        if self.in_use { Sizes::TOUCH } else { Sizes::MOUSE }
    }
}

/// How far (points) from a handle a press still grabs it, and how big the handles are drawn.
/// A mouse keeps the sizes the app always had; while touch or a pen is in use
/// ([`Touch::in_use`]) the handles are bigger, so a fingertip or a pen held a little off still
/// lands on them. The rest of the interface is not scaled.
///
/// Every distance is in points from the handle's centre (a point) or its line (a curve, an edge).
/// A point wins over a line within reach of both, because the picker lists points first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sizes {
    /// A sketch point.
    pub point: f32,
    /// An end point of a model edge.
    pub vertex: f32,
    /// A sketch curve.
    pub curve: f32,
    /// A model edge.
    pub edge: f32,
    /// An origin or construction axis.
    pub axis: f32,
    /// The centre ball, the arrows and the rings of the move triad.
    pub ball: f32,
    pub arrow: f32,
    pub ring: f32,
    /// The corners and the rotation knob of a text's frame.
    pub frame: f32,
    /// Factor on the dots drawn for points (1: as drawn for a mouse).
    pub dot: f32,
    /// Radius of the halo round the point under the pointer, and of its press highlight
    /// (0: none).
    pub halo: f32,
    /// Extra width (points) of the highlight under a hovered curve or edge (0: none).
    pub line_halo: f32,
}

impl Sizes {
    pub(crate) const MOUSE: Self = Self {
        point: 8.0,
        vertex: 7.0,
        curve: 6.0,
        edge: 6.0,
        axis: 5.0,
        ball: 9.0,
        arrow: 6.0,
        ring: 5.0,
        frame: 8.0,
        dot: 1.0,
        halo: 0.0,
        line_halo: 0.0,
    };

    pub(crate) const TOUCH: Self = Self {
        point: 20.0,
        vertex: 18.0,
        curve: 12.0,
        edge: 12.0,
        axis: 10.0,
        ball: 16.0,
        arrow: 14.0,
        ring: 12.0,
        frame: 20.0,
        dot: 1.8,
        halo: 14.0,
        line_halo: 6.0,
    };

    /// The farthest any pick reaches: the picker looks this far round the pointer, then applies
    /// each kind's own distance.
    pub(crate) fn widest(&self) -> f32 {
        self.point.max(self.vertex).max(self.curve).max(self.edge).max(self.axis)
    }
}

/// What one finger touch is doing.
#[derive(Clone, Copy, Debug)]
struct Finger {
    id: u64,
    start: Pos2,
    pos: Pos2,
    /// The furthest it has been from where it landed.
    travel: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Route {
    #[default]
    Idle,
    /// The first finger landed off the camera: it is the mouse, other fingers are ignored.
    Mouse { id: u64 },
    /// The fingers move the camera. `most` is the most fingers down at once.
    Camera { since: f64, most: usize, orbiting: bool },
}

#[derive(Debug, Default)]
struct Router {
    route: Route,
    fingers: Vec<Finger>,
    /// Fingers that land while the pen is in use, or join a mouse finger: never used.
    ignored: Vec<u64>,
    pen_down: bool,
    /// When the pen was last seen (seconds).
    pen_at: Option<f64>,
    orbit: Vec2,
    orbit_from: Option<Pos2>,
    /// Two or more fingers this frame: how far their centre moved, the log of how much they
    /// spread, and where they are.
    pinch_pan: Vec2,
    pinch_ln: f32,
    pinch_center: Option<Pos2>,
    command: Option<&'static str>,
    /// Where the gesture's first finger landed, and the furthest any of its fingers moved.
    tap_at: Pos2,
    tap_travel: f32,
    /// Events held for the following frames (a tap's click: press, then release, then gone).
    later: std::collections::VecDeque<Vec<Event>>,
}

impl Router {
    fn event(
        &mut self,
        event: DeviceEvent,
        now: f64,
        mods: egui::Modifiers,
        camera_at: &dyn Fn(Pos2) -> bool,
        pen_seen: &mut bool,
        out: &mut Vec<Event>,
    ) {
        let finite = |p: Pos2| p.is_finite();
        match event {
            DeviceEvent::PenDown { pos, button } if finite(pos) => {
                if !*pen_seen {
                    crate::debug_log::push("pen used: finger taps on the view no longer select");
                }
                crate::debug_log::push(format!("pen down ({button:?})"));
                *pen_seen = true;
                self.pen_at = Some(now);
                self.drop_fingers(out);
                out.push(Event::PointerMoved(pos));
                out.push(Event::PointerButton { pos, button, pressed: true, modifiers: mods });
                if button == egui::PointerButton::Primary && !self.pen_down {
                    out.push(pen_touch(TouchPhase::Start, pos));
                }
                self.pen_down = true;
            }
            DeviceEvent::PenMove { pos } if finite(pos) => {
                // Only a pen on the screen keeps the palm away: one hovering in the hand that
                // holds it doesn't stop fingers moving the view.
                if self.pen_down {
                    self.pen_at = Some(now);
                }
                out.push(Event::PointerMoved(pos));
                if self.pen_down {
                    out.push(pen_touch(TouchPhase::Move, pos));
                }
            }
            DeviceEvent::PenUp { pos, button } if finite(pos) => {
                self.pen_at = Some(now);
                out.push(Event::PointerButton { pos, button, pressed: false, modifiers: mods });
                // Whatever button it reports, a pen that came up is up (a pen stuck down would
                // keep every finger away).
                if self.pen_down {
                    out.push(pen_touch(TouchPhase::End, pos));
                    self.pen_down = false;
                }
                crate::debug_log::push("pen up");
            }
            DeviceEvent::PenLeave => out.push(Event::PointerGone),
            DeviceEvent::PenCancel => {
                crate::debug_log::push("pen cancelled by the system");
                if self.pen_down {
                    out.push(pen_touch(TouchPhase::Cancel, Pos2::ZERO));
                    self.pen_down = false;
                }
                out.push(Event::PointerGone);
            }
            DeviceEvent::Finger { id, phase, pos } if finite(pos) => self.finger(id, phase, pos, now, mods, camera_at, *pen_seen, out),
            // Non-finite positions are dropped.
            _ => {}
        }
    }

    /// The pen took over: every finger stops, and is ignored until it lifts.
    fn drop_fingers(&mut self, out: &mut Vec<Event>) {
        match self.route {
            Route::Camera { .. } => {
                for f in &self.fingers {
                    out.push(finger_touch(f.id, TouchPhase::Cancel, f.pos));
                }
            }
            Route::Mouse { id } => {
                if let Some(f) = self.fingers.iter().find(|f| f.id == id) {
                    out.push(finger_touch(f.id, TouchPhase::Cancel, f.pos));
                    out.push(Event::PointerButton {
                        pos: f.pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    });
                    out.push(Event::PointerGone);
                }
            }
            Route::Idle => {}
        }
        for f in self.fingers.drain(..) {
            if self.ignored.len() < MAX_TOUCHES {
                self.ignored.push(f.id);
            }
        }
        self.route = Route::Idle;
        self.orbit = Vec2::ZERO;
        self.orbit_from = None;
        self.pinch_pan = Vec2::ZERO;
        self.pinch_ln = 0.0;
    }

    #[allow(clippy::too_many_arguments)]
    fn finger(
        &mut self,
        id: u64,
        phase: TouchPhase,
        pos: Pos2,
        now: f64,
        mods: egui::Modifiers,
        camera_at: &dyn Fn(Pos2) -> bool,
        pen_seen: bool,
        out: &mut Vec<Event>,
    ) {
        let ends = matches!(phase, TouchPhase::End | TouchPhase::Cancel);
        if phase == TouchPhase::Start {
            // A new contact under an id still in use: that one's end was lost (ids are reused).
            // Forget it, or it would keep fingers away for good.
            if self.ignored.contains(&id) {
                crate::debug_log::push(format!("finger {id}: lost end of an ignored touch"));
                self.ignored.retain(|i| *i != id);
            }
            if self.fingers.iter().any(|f| f.id == id) {
                crate::debug_log::push(format!("finger {id}: lost end, gesture reset"));
                self.drop_fingers(out);
                self.ignored.retain(|i| *i != id);
            }
        }
        if self.ignored.contains(&id) {
            if ends {
                self.ignored.retain(|i| *i != id);
            }
            return;
        }
        let known = self.fingers.iter().position(|f| f.id == id);
        if phase == TouchPhase::Start && known.is_none() {
            let palm = self.pen_down || self.pen_at.is_some_and(|t| (now - t).abs() < PALM_WINDOW);
            let mouse_route = matches!(self.route, Route::Mouse { .. });
            if palm || mouse_route || self.fingers.len() >= MAX_TOUCHES {
                let why = if self.pen_down {
                    "the pen is down"
                } else if palm {
                    "the pen was just used (palm)"
                } else if mouse_route {
                    "another finger is the mouse"
                } else {
                    "too many fingers"
                };
                crate::debug_log::push(format!("finger {id} ignored: {why}"));
                if self.ignored.len() >= MAX_TOUCHES {
                    self.ignored.remove(0);
                }
                self.ignored.push(id);
                return;
            }
            self.fingers.push(Finger { id, start: pos, pos, travel: 0.0 });
            match &mut self.route {
                Route::Idle if camera_at(pos) => {
                    crate::debug_log::push(format!("finger {id} on the view: moves the camera"));
                    self.tap_at = pos;
                    self.tap_travel = 0.0;
                    self.route = Route::Camera { since: now, most: 1, orbiting: false };
                    out.push(finger_touch(id, phase, pos));
                }
                Route::Idle => {
                    crate::debug_log::push(format!("finger {id} on a control: acts as the mouse"));
                    self.route = Route::Mouse { id };
                    out.push(Event::PointerMoved(pos));
                    out.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: mods });
                    // Its touch lets a finger held on a control open that control's menu.
                    out.push(finger_touch(id, phase, pos));
                }
                Route::Camera { most, .. } => {
                    *most = (*most).max(self.fingers.len());
                    out.push(finger_touch(id, phase, pos));
                }
                Route::Mouse { .. } => {}
            }
            return;
        }
        // A move or end of a finger never seen to start (it began before the app saw input).
        let Some(index) = known else { return };
        let before = spread(&self.fingers);
        let Some(finger) = self.fingers.get_mut(index) else { return };
        let delta = pos - finger.pos;
        finger.pos = pos;
        finger.travel = finger.travel.max(pos.distance(finger.start));
        let (start, travel) = (finger.start, finger.travel);
        self.tap_travel = self.tap_travel.max(travel);
        let count = self.fingers.len();
        match &mut self.route {
            Route::Mouse { id: mouse } if *mouse == id => {
                out.push(Event::PointerMoved(pos));
                out.push(finger_touch(id, phase, pos));
                if ends {
                    out.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: mods });
                    out.push(Event::PointerGone);
                }
            }
            Route::Mouse { .. } => {}
            Route::Camera { most, orbiting, .. } => {
                out.push(finger_touch(id, phase, pos));
                // Two or more fingers pan with their centre and zoom as they spread. (egui's own
                // gesture tracking needs a mouse position, which fingers here don't give.)
                if phase == TouchPhase::Move
                    && let (Some((c0, d0)), Some((c1, d1))) = (before, spread(&self.fingers))
                {
                    self.pinch_pan += c1 - c0;
                    if d0 > 1.0 && d1 > 1.0 {
                        self.pinch_ln += (d1 / d0).ln();
                    }
                    self.pinch_center = Some(c1);
                }
                // One finger orbits once it clearly moves, and only while it is alone: after a
                // second finger, lifting one doesn't turn the view with the one left.
                if !ends && count == 1 && *most == 1 && (*orbiting || travel > TOUCH_SLOP) {
                    *orbiting = true;
                    self.orbit += delta;
                    self.orbit_from = Some(start);
                }
            }
            Route::Idle => {}
        }
        if ends {
            self.fingers.remove(index);
            if self.fingers.is_empty() {
                self.finish(now, phase == TouchPhase::End, pen_seen, mods);
            }
        }
    }

    /// The last finger lifted: a short, still touch is a tap.
    fn finish(&mut self, now: f64, ended: bool, pen_seen: bool, mods: egui::Modifiers) {
        let route = std::mem::take(&mut self.route);
        let Route::Camera { since, most, orbiting } = route else { return };
        let still = !orbiting && self.tap_travel <= TAP_SLOP;
        let tap = ended && still && (now - since) < TAP_TIME;
        self.tap_travel = 0.0;
        if !tap {
            return;
        }
        match most {
            1 if pen_seen => crate::debug_log::push("finger tap on the view: no selection (a pen has been used)"),
            1 => {
                // A one-finger tap selects: a click over three frames, so egui sees the press
                // and release apart and the hover is right for both.
                let at = self.tap_at;
                let press = Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: mods };
                let release = Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: mods };
                self.later.push_back(vec![Event::PointerMoved(at), press]);
                self.later.push_back(vec![release]);
                self.later.push_back(vec![Event::PointerGone]);
            }
            2 => {
                crate::debug_log::push("two-finger tap: undo");
                self.command = Some("edit.undo");
            }
            3 => {
                crate::debug_log::push("three-finger tap: redo");
                self.command = Some("edit.redo");
            }
            _ => {}
        }
    }
}

/// The centre of two or more fingers and their mean distance from it.
fn spread(fingers: &[Finger]) -> Option<(Pos2, f32)> {
    if fingers.len() < 2 {
        return None;
    }
    let n = fingers.len() as f32;
    let centre = fingers.iter().fold(Vec2::ZERO, |sum, f| sum + f.pos.to_vec2()) / n;
    let centre = centre.to_pos2();
    let distance = fingers.iter().map(|f| f.pos.distance(centre)).sum::<f32>() / n;
    (centre.is_finite() && distance.is_finite()).then_some((centre, distance))
}

fn finger_touch(id: u64, phase: TouchPhase, pos: Pos2) -> Event {
    Event::Touch { device_id: FINGER_DEVICE, id: TouchId(id), phase, pos, force: None }
}

fn pen_touch(phase: TouchPhase, pos: Pos2) -> Event {
    Event::Touch { device_id: PEN_DEVICE, id: TouchId(0), phase, pos, force: None }
}

/// What the fingers did to the camera of the page `rect` (drawn in `ui`) this frame. `inside`:
/// the mouse is over the page (on the desktop, the first finger is the mouse).
pub(crate) fn camera_input(app: &mut crate::SolveApp, ui: &egui::Ui, rect: Rect, inside: bool) -> Fingers {
    let touch = &mut app.touch;
    touch.camera_rect = Some(rect);
    touch.camera_layer = Some(ui.layer_id());
    let router = &mut touch.router;
    let (active, gesture) = if touch.raw {
        let zoom = router.pinch_ln.exp();
        let gesture = TouchGesture {
            zoom: if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 },
            pan: if router.pinch_pan.is_finite() { router.pinch_pan } else { Vec2::ZERO },
            center: router.pinch_center,
        };
        (matches!(router.route, Route::Camera { .. }), gesture)
    } else {
        let gesture = touch_gesture(ui.input(|i| i.multi_touch()));
        let active = inside && gesture != TouchGesture::NONE;
        (active, if active { gesture } else { TouchGesture::NONE })
    };
    router.pinch_pan = Vec2::ZERO;
    router.pinch_ln = 0.0;
    let orbit = std::mem::take(&mut router.orbit);
    let fingers = Fingers {
        active,
        pan: gesture.pan,
        zoom: gesture.zoom,
        center: gesture.center,
        orbit: if orbit.is_finite() { orbit } else { Vec2::ZERO },
        orbit_from: router.orbit_from,
        command: router.command.take(),
    };
    if touch.waiting() {
        // A resting finger's press, or a tap's release, goes through on a later frame.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(TOUCH_GESTURE_WAIT / 3.0));
    }
    fingers
}

/// Touchscreens send the first finger as the primary pointer too, so a two-finger pinch would
/// also start a selection box or a geometry drag under that finger. The gate holds a finger's
/// press back briefly: a lone finger (a tap, or a drag once it moves or rests) goes through
/// unchanged, and a second finger turns the touch into a camera gesture whose pointer events are
/// dropped until every finger is lifted. Mouse and pen input pass straight through.
#[derive(Debug, Default)]
pub struct TouchGate {
    fingers: Vec<(egui::TouchDeviceId, egui::TouchId)>,
    state: GateState,
}

#[derive(Debug, Default)]
enum GateState {
    #[default]
    Idle,
    /// A finger's press, withheld since `since`, with the moves seen after it.
    Pending { press: egui::Event, origin: egui::Pos2, since: f64, last: Option<egui::Pos2> },
    /// A lone finger's press went through: it selects or drags as a mouse would.
    Passed,
    /// Two or more fingers: the camera gesture owns the touch until all fingers lift.
    Gesture,
}

impl TouchGate {
    /// Rewrite one frame of raw input in place.
    pub(crate) fn filter(&mut self, raw: &mut egui::RawInput) {
        let now = raw.time.filter(|t| t.is_finite()).unwrap_or(0.0);
        let touched = raw.events.iter().any(|e| matches!(e, egui::Event::Touch { .. }));
        if !touched && self.fingers.is_empty() && matches!(self.state, GateState::Idle) {
            return;
        }
        let events = std::mem::take(&mut raw.events);
        let mut out = Vec::with_capacity(events.len() + 2);
        for event in events {
            match &event {
                egui::Event::Touch { device_id, id, phase, .. } => {
                    let key = (*device_id, *id);
                    match phase {
                        egui::TouchPhase::Start => {
                            if !self.fingers.contains(&key) && self.fingers.len() < MAX_TOUCHES {
                                self.fingers.push(key);
                            }
                        }
                        egui::TouchPhase::End | egui::TouchPhase::Cancel => self.fingers.retain(|f| *f != key),
                        egui::TouchPhase::Move => {}
                    }
                    if self.fingers.len() >= 2 && !matches!(self.state, GateState::Passed) {
                        // The withheld press is dropped: it never happened as far as the UI knows.
                        self.state = GateState::Gesture;
                    }
                    out.push(event);
                }
                egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed, pos, .. } if self.touching() => {
                    match (&mut self.state, *pressed) {
                        (GateState::Idle, true) => {
                            self.state = GateState::Pending { press: event.clone(), origin: *pos, since: now, last: None };
                        }
                        (GateState::Pending { press, last, .. }, false) => {
                            // A tap: the press and its release go through together.
                            out.push(egui::Event::PointerMoved(press_pos(press)));
                            out.push(press.clone());
                            if let Some(at) = last.take() {
                                out.push(egui::Event::PointerMoved(at));
                            }
                            out.push(event);
                            self.state = GateState::Idle;
                        }
                        (GateState::Gesture, _) => {}
                        (GateState::Passed, false) => {
                            out.push(event);
                            self.state = GateState::Idle;
                        }
                        _ => out.push(event),
                    }
                }
                egui::Event::PointerMoved(at) if self.touching() => match &mut self.state {
                    GateState::Pending { origin, last, .. } => {
                        *last = Some(*at);
                        if at.distance(*origin) > TOUCH_SLOP {
                            self.release_pending(&mut out);
                        }
                    }
                    GateState::Gesture => {}
                    _ => out.push(event),
                },
                _ => out.push(event),
            }
        }
        if let GateState::Pending { since, .. } = self.state
            && now - since >= TOUCH_GESTURE_WAIT
        {
            self.release_pending(&mut out);
        }
        if self.fingers.is_empty() {
            match self.state {
                GateState::Gesture => {
                    // The fingers left the screen: no pointer hovers anything now.
                    out.push(egui::Event::PointerGone);
                    self.state = GateState::Idle;
                }
                GateState::Pending { .. } => self.release_pending(&mut out),
                // Its release, if not in this frame, arrives with no finger down and passes.
                GateState::Passed => self.state = GateState::Idle,
                GateState::Idle => {}
            }
        }
        raw.events = out;
    }

    /// A finger is on the screen, or a press it made is being held back.
    fn touching(&self) -> bool {
        !self.fingers.is_empty() || !matches!(self.state, GateState::Idle | GateState::Passed)
    }

    /// The lone finger keeps going: send its press (and where it has moved since) as a drag.
    fn release_pending(&mut self, out: &mut Vec<egui::Event>) {
        if let GateState::Pending { press, last, .. } = std::mem::take(&mut self.state) {
            out.push(egui::Event::PointerMoved(press_pos(&press)));
            out.push(press);
            if let Some(at) = last {
                out.push(egui::Event::PointerMoved(at));
            }
            self.state = GateState::Passed;
        }
    }

    /// A press is waiting for the gesture window to pass: the app must draw another frame to
    /// let it through even if the finger rests without moving.
    pub(crate) fn waiting(&self) -> bool {
        matches!(self.state, GateState::Pending { .. })
    }
}

fn press_pos(press: &egui::Event) -> egui::Pos2 {
    match press {
        egui::Event::PointerButton { pos, .. } => *pos,
        _ => egui::Pos2::ZERO,
    }
}

#[cfg(test)]
mod tests {
    fn multi_touch(num_touches: usize, zoom_delta: f32, translation_delta: egui::Vec2) -> egui::MultiTouchInfo {
        egui::MultiTouchInfo {
            start_time: 0.0,
            start_pos: egui::Pos2::ZERO,
            center_pos: egui::Pos2::ZERO,
            num_touches,
            zoom_delta,
            zoom_delta_2d: egui::vec2(zoom_delta, zoom_delta),
            rotation_delta: 0.0,
            translation_delta,
            force: 0.0,
        }
    }

    #[test]
    fn two_finger_touch_pinches_and_pans() {
        use egui::vec2;
        let g = super::touch_gesture(Some(multi_touch(2, 1.25, vec2(3.0, -4.0))));
        assert_eq!(g, super::TouchGesture { zoom: 1.25, pan: vec2(3.0, -4.0), center: Some(egui::Pos2::ZERO) });
        let g = super::touch_gesture(Some(multi_touch(3, 0.8, vec2(-1.0, 2.0))));
        assert_eq!(g, super::TouchGesture { zoom: 0.8, pan: vec2(-1.0, 2.0), center: Some(egui::Pos2::ZERO) });
    }

    fn touch(id: u64, phase: egui::TouchPhase, at: egui::Pos2) -> egui::Event {
        egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(id), phase, pos: at, force: None }
    }

    fn button(pressed: bool, at: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
    }

    fn gate_frame(gate: &mut super::TouchGate, time: f64, events: Vec<egui::Event>) -> Vec<egui::Event> {
        let mut raw = egui::RawInput { time: Some(time), events, ..Default::default() };
        gate.filter(&mut raw);
        raw.events.into_iter().filter(|e| !matches!(e, egui::Event::Touch { .. })).collect()
    }

    fn presses(events: &[egui::Event]) -> usize {
        events.iter().filter(|e| matches!(e, egui::Event::PointerButton { pressed: true, .. })).count()
    }

    #[test]
    fn touch_gate_lets_a_tap_through() {
        use egui::{TouchPhase::*, pos2};
        let at = pos2(100.0, 100.0);
        let mut gate = super::TouchGate::default();
        let first = gate_frame(&mut gate, 0.0, vec![touch(1, Start, at), egui::Event::PointerMoved(at), button(true, at)]);
        assert_eq!(presses(&first), 0, "the press waits for a second finger");
        let tap = gate_frame(&mut gate, 0.05, vec![touch(1, End, at), button(false, at)]);
        assert_eq!(presses(&tap), 1);
        assert!(matches!(tap.last(), Some(egui::Event::PointerButton { pressed: false, .. })));
        assert!(!gate.waiting());
    }

    #[test]
    fn touch_gate_drops_the_press_of_a_two_finger_gesture() {
        use egui::{TouchPhase::*, pos2};
        let (a, b) = (pos2(100.0, 100.0), pos2(200.0, 100.0));
        let mut gate = super::TouchGate::default();
        let mut seen = gate_frame(&mut gate, 0.0, vec![touch(1, Start, a), egui::Event::PointerMoved(a), button(true, a)]);
        seen.extend(gate_frame(&mut gate, 0.03, vec![touch(2, Start, b)]));
        for i in 1..20 {
            let d = i as f32 * 5.0;
            seen.extend(gate_frame(
                &mut gate,
                0.03 + f64::from(i) * 0.02,
                vec![
                    touch(1, Move, pos2(100.0 - d, 100.0)),
                    touch(2, Move, pos2(200.0 + d, 100.0)),
                    egui::Event::PointerMoved(pos2(100.0 - d, 100.0)),
                ],
            ));
        }
        seen.extend(gate_frame(&mut gate, 0.5, vec![touch(1, End, a), touch(2, End, b), button(false, a)]));
        assert_eq!(presses(&seen), 0);
        assert!(!seen.iter().any(|e| matches!(e, egui::Event::PointerButton { .. })));
        assert_eq!(seen.last(), Some(&egui::Event::PointerGone));
        // The next lone tap is gated afresh and goes through.
        gate_frame(&mut gate, 1.0, vec![touch(3, Start, a), button(true, a)]);
        assert_eq!(presses(&gate_frame(&mut gate, 1.05, vec![touch(3, End, a), button(false, a)])), 1);
    }

    #[test]
    fn touch_gate_lets_a_moving_or_resting_finger_drag() {
        use egui::{TouchPhase::*, pos2};
        let at = pos2(100.0, 100.0);
        let mut gate = super::TouchGate::default();
        gate_frame(&mut gate, 0.0, vec![touch(1, Start, at), button(true, at)]);
        let moved = gate_frame(&mut gate, 0.02, vec![touch(1, Move, pos2(120.0, 100.0)), egui::Event::PointerMoved(pos2(120.0, 100.0))]);
        assert_eq!(presses(&moved), 1);
        assert_eq!(moved.last(), Some(&egui::Event::PointerMoved(pos2(120.0, 100.0))));
        let released = gate_frame(&mut gate, 0.3, vec![touch(1, End, at), button(false, at)]);
        assert!(matches!(released.as_slice(), [egui::Event::PointerButton { pressed: false, .. }]));

        let mut gate = super::TouchGate::default();
        gate_frame(&mut gate, 0.0, vec![touch(1, Start, at), button(true, at)]);
        assert!(gate.waiting());
        assert_eq!(presses(&gate_frame(&mut gate, 0.2, vec![])), 1, "a resting finger is let through");
        assert!(!gate.waiting());
    }

    #[test]
    fn touch_gate_leaves_mouse_input_alone() {
        use egui::pos2;
        let at = pos2(10.0, 10.0);
        let mut gate = super::TouchGate::default();
        let events = vec![egui::Event::PointerMoved(at), button(true, at), egui::Event::PointerMoved(pos2(11.0, 10.0)), button(false, at)];
        assert_eq!(gate_frame(&mut gate, 0.0, events.clone()), events);
    }

    #[test]
    fn touch_gate_bounds_hostile_touch_streams() {
        use egui::{TouchPhase::*, pos2};
        let mut gate = super::TouchGate::default();
        let starts = (0..1000).map(|i| touch(i, Start, pos2(f32::NAN, f32::INFINITY))).collect();
        gate_frame(&mut gate, f64::NAN, starts);
        assert!(gate.fingers.len() <= super::MAX_TOUCHES);
        let ends = (0..1000).map(|i| touch(i, Cancel, pos2(0.0, 0.0))).collect();
        gate_frame(&mut gate, 0.1, ends);
        assert!(gate.fingers.is_empty());
    }

    #[test]
    fn touch_gesture_ignores_one_finger_and_hostile_values() {
        use egui::vec2;
        assert_eq!(super::touch_gesture(None), super::TouchGesture::NONE);
        assert_eq!(super::touch_gesture(Some(multi_touch(1, 1.5, vec2(5.0, 5.0)))), super::TouchGesture::NONE);
        for zoom in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(super::touch_gesture(Some(multi_touch(2, zoom, vec2(1.0, 1.0)))).zoom, 1.0);
        }
        let g = super::touch_gesture(Some(multi_touch(2, 1.0, vec2(f32::NAN, 1.0))));
        assert_eq!(g.pan, egui::Vec2::ZERO);
    }

    // ---- the router: pens and fingers told apart (the browser) ----

    use super::{DeviceEvent, Touch};
    use egui::{PointerButton, TouchPhase, pos2};

    /// A 800×600 camera page at the origin, with a toolbar strip above y = 50.
    fn on_camera(p: egui::Pos2) -> bool {
        p.y > 50.0 && p.x < 800.0 && p.y < 600.0
    }

    fn device_frame(t: &mut Touch, time: f64, events: Vec<DeviceEvent>) -> Vec<egui::Event> {
        let mut raw = egui::RawInput { time: Some(time), ..Default::default() };
        t.device_input(&mut raw, events, &on_camera);
        raw.events
    }

    fn finger(id: u64, phase: TouchPhase, x: f32, y: f32) -> DeviceEvent {
        DeviceEvent::Finger { id, phase, pos: pos2(x, y) }
    }

    fn pointer_events(events: &[egui::Event]) -> usize {
        events.iter().filter(|e| matches!(e, egui::Event::PointerButton { .. } | egui::Event::PointerMoved(_))).count()
    }

    fn finger_touches(events: &[egui::Event]) -> usize {
        events.iter().filter(|e| matches!(e, egui::Event::Touch { device_id, .. } if *device_id == super::FINGER_DEVICE)).count()
    }

    #[test]
    fn the_pen_is_the_mouse() {
        let mut t = Touch::default();
        let at = pos2(300.0, 300.0);
        let down = device_frame(&mut t, 0.0, vec![DeviceEvent::PenDown { pos: at, button: PointerButton::Primary }]);
        assert!(down.contains(&egui::Event::PointerMoved(at)));
        assert!(down.iter().any(|e| matches!(e, egui::Event::PointerButton { pressed: true, button: PointerButton::Primary, .. })));
        // The pen's own touch lets a pen held still open the context menu.
        assert!(down.iter().any(|e| matches!(e, egui::Event::Touch { device_id, phase: TouchPhase::Start, .. } if *device_id == super::PEN_DEVICE)));
        let moved = device_frame(&mut t, 0.1, vec![DeviceEvent::PenMove { pos: pos2(320.0, 300.0) }]);
        assert!(moved.contains(&egui::Event::PointerMoved(pos2(320.0, 300.0))));
        let up = device_frame(&mut t, 0.2, vec![DeviceEvent::PenUp { pos: pos2(320.0, 300.0), button: PointerButton::Primary }]);
        assert!(up.iter().any(|e| matches!(e, egui::Event::PointerButton { pressed: false, .. })));
        assert!(up.iter().any(|e| matches!(e, egui::Event::Touch { phase: TouchPhase::End, .. })));
        assert!(t.pen_seen && t.in_use);
        // The barrel button is a right click, without the pen's touch.
        let barrel = device_frame(&mut t, 0.3, vec![DeviceEvent::PenDown { pos: at, button: PointerButton::Secondary }]);
        assert!(barrel.iter().any(|e| matches!(e, egui::Event::PointerButton { pressed: true, button: PointerButton::Secondary, .. })));
        assert!(!barrel.iter().any(|e| matches!(e, egui::Event::Touch { .. })));
        assert_eq!(device_frame(&mut t, 0.4, vec![DeviceEvent::PenLeave]), vec![egui::Event::PointerGone]);
    }

    #[test]
    fn one_finger_on_the_camera_orbits_and_never_clicks() {
        let mut t = Touch::default();
        let mut seen = device_frame(&mut t, 0.0, vec![finger(1, TouchPhase::Start, 400.0, 300.0)]);
        assert!(t.camera_active());
        for i in 1..=10 {
            seen.extend(device_frame(&mut t, f64::from(i) * 0.02, vec![finger(1, TouchPhase::Move, 400.0 + 5.0 * i as f32, 300.0)]));
        }
        seen.extend(device_frame(&mut t, 0.5, vec![finger(1, TouchPhase::End, 450.0, 300.0)]));
        for _ in 0..3 {
            seen.extend(device_frame(&mut t, 0.6, vec![]));
        }
        assert_eq!(pointer_events(&seen), 0, "a drag on the camera is not the mouse: {seen:?}");
        // Orbit starts once the finger clears the slop, then follows it.
        assert!(t.router.orbit.x > 30.0 && t.router.orbit.x <= 50.0, "{:?}", t.router.orbit);
        assert_eq!(t.router.orbit_from, Some(pos2(400.0, 300.0)));
        assert!(!t.camera_active());
    }

    #[test]
    fn a_finger_tap_on_the_camera_selects_until_a_pen_is_used() {
        let mut t = Touch::default();
        let at = pos2(400.0, 300.0);
        let mut seen = device_frame(&mut t, 0.0, vec![finger(1, TouchPhase::Start, at.x, at.y)]);
        seen.extend(device_frame(&mut t, 0.1, vec![finger(1, TouchPhase::End, at.x + 2.0, at.y)]));
        assert_eq!(pointer_events(&seen), 0, "the click comes on the next frames");
        let click: Vec<_> = (0..3).map(|i| device_frame(&mut t, 0.12 + f64::from(i) * 0.02, vec![])).collect();
        assert!(click[0].contains(&egui::Event::PointerMoved(at)));
        assert!(click[0].iter().any(|e| matches!(e, egui::Event::PointerButton { pressed: true, .. })));
        assert!(matches!(click[1].as_slice(), [egui::Event::PointerButton { pressed: false, .. }]));
        assert_eq!(click[2], vec![egui::Event::PointerGone]);
        assert!(!t.waiting());

        device_frame(&mut t, 1.0, vec![DeviceEvent::PenDown { pos: at, button: PointerButton::Primary }]);
        device_frame(&mut t, 1.1, vec![DeviceEvent::PenUp { pos: at, button: PointerButton::Primary }, DeviceEvent::PenLeave]);
        let mut seen = device_frame(&mut t, 3.0, vec![finger(2, TouchPhase::Start, at.x, at.y)]);
        seen.extend(device_frame(&mut t, 3.1, vec![finger(2, TouchPhase::End, at.x, at.y)]));
        for i in 0..3 {
            seen.extend(device_frame(&mut t, 3.2 + f64::from(i) * 0.02, vec![]));
        }
        assert_eq!(pointer_events(&seen), 0, "with a pen in use, fingers only move the camera");
    }

    #[test]
    fn two_and_three_finger_taps_undo_and_redo() {
        let mut t = Touch::default();
        device_frame(&mut t, 0.0, vec![finger(1, TouchPhase::Start, 300.0, 300.0), finger(2, TouchPhase::Start, 380.0, 300.0)]);
        let lift = device_frame(&mut t, 0.15, vec![finger(1, TouchPhase::End, 301.0, 300.0), finger(2, TouchPhase::End, 381.0, 301.0)]);
        assert_eq!(pointer_events(&lift), 0);
        assert_eq!(t.router.command.take(), Some("edit.undo"));
        let three = [(1, 300.0), (2, 360.0), (3, 420.0)];
        device_frame(&mut t, 1.0, three.iter().map(|(id, x)| finger(*id, TouchPhase::Start, *x, 300.0)).collect());
        device_frame(&mut t, 1.2, three.iter().map(|(id, x)| finger(*id, TouchPhase::End, *x, 300.0)).collect());
        assert_eq!(t.router.command.take(), Some("edit.redo"));
        // A pinch is not a tap, and neither is a long hold.
        device_frame(&mut t, 2.0, vec![finger(1, TouchPhase::Start, 300.0, 300.0), finger(2, TouchPhase::Start, 380.0, 300.0)]);
        device_frame(&mut t, 2.1, vec![finger(1, TouchPhase::Move, 260.0, 300.0), finger(2, TouchPhase::Move, 420.0, 300.0)]);
        device_frame(&mut t, 2.2, vec![finger(1, TouchPhase::End, 260.0, 300.0), finger(2, TouchPhase::End, 420.0, 300.0)]);
        device_frame(&mut t, 3.0, vec![finger(1, TouchPhase::Start, 300.0, 300.0), finger(2, TouchPhase::Start, 380.0, 300.0)]);
        device_frame(&mut t, 4.5, vec![finger(1, TouchPhase::End, 300.0, 300.0), finger(2, TouchPhase::End, 380.0, 300.0)]);
        assert_eq!(t.router.command, None);
    }

    #[test]
    fn two_fingers_held_still_send_no_mouse_press() {
        // The context menu on a two-finger long press came from a mouse press under the first
        // finger; on the camera, fingers send none, so egui never sees a long press.
        let mut t = Touch::default();
        let mut seen = device_frame(&mut t, 0.0, vec![finger(1, TouchPhase::Start, 300.0, 300.0)]);
        seen.extend(device_frame(&mut t, 0.05, vec![finger(2, TouchPhase::Start, 380.0, 300.0)]));
        for i in 0..60 {
            seen.extend(device_frame(&mut t, 0.1 + f64::from(i) * 0.02, vec![]));
        }
        seen.extend(device_frame(&mut t, 1.5, vec![finger(1, TouchPhase::End, 300.0, 300.0), finger(2, TouchPhase::End, 380.0, 300.0)]));
        assert_eq!(pointer_events(&seen), 0);
        assert_eq!(finger_touches(&seen), 4, "egui gets the touches for its pinch tracking");
    }

    #[test]
    fn a_finger_off_the_camera_is_the_mouse() {
        let mut t = Touch::default();
        let at = pos2(100.0, 20.0);
        let down = device_frame(&mut t, 0.0, vec![finger(1, TouchPhase::Start, at.x, at.y)]);
        assert!(down.iter().any(|e| matches!(e, egui::Event::PointerButton { pressed: true, pos, .. } if *pos == at)));
        // A second finger beside a mouse finger does nothing.
        let second = device_frame(&mut t, 0.05, vec![finger(2, TouchPhase::Start, 300.0, 300.0), finger(2, TouchPhase::Move, 350.0, 300.0)]);
        assert!(second.is_empty(), "{second:?}");
        let moved = device_frame(&mut t, 0.1, vec![finger(1, TouchPhase::Move, 120.0, 20.0)]);
        assert_eq!(moved.first(), Some(&egui::Event::PointerMoved(pos2(120.0, 20.0))));
        assert_eq!(pointer_events(&moved), 1);
        let up = device_frame(&mut t, 0.2, vec![finger(1, TouchPhase::End, 120.0, 20.0), finger(2, TouchPhase::End, 350.0, 300.0)]);
        assert!(up.iter().any(|e| matches!(e, egui::Event::PointerButton { pressed: false, .. })));
        assert_eq!(up.last(), Some(&egui::Event::PointerGone));
        // Its touch goes with it (a finger held on a control opens its menu); the other's doesn't.
        assert!(!t.camera_active() && finger_touches(&up) == 1);
    }

    #[test]
    fn the_palm_is_ignored_while_the_pen_writes() {
        let mut t = Touch::default();
        device_frame(&mut t, 0.0, vec![finger(1, TouchPhase::Start, 400.0, 300.0), finger(2, TouchPhase::Start, 480.0, 300.0)]);
        assert!(t.camera_active());
        // The pen comes down: the fingers' gesture is cancelled, not finished.
        let pen = device_frame(&mut t, 0.1, vec![DeviceEvent::PenDown { pos: pos2(200.0, 200.0), button: PointerButton::Primary }]);
        assert_eq!(pen.iter().filter(|e| matches!(e, egui::Event::Touch { phase: TouchPhase::Cancel, .. })).count(), 2);
        assert!(!t.camera_active());
        // A palm landing while it writes, and the old fingers moving, are ignored.
        let palm = device_frame(
            &mut t,
            0.2,
            vec![finger(3, TouchPhase::Start, 500.0, 400.0), finger(3, TouchPhase::Move, 520.0, 400.0), finger(1, TouchPhase::Move, 420.0, 300.0)],
        );
        assert_eq!(finger_touches(&palm), 0);
        assert_eq!(pointer_events(&palm), 0);
        device_frame(&mut t, 0.3, vec![DeviceEvent::PenUp { pos: pos2(200.0, 200.0), button: PointerButton::Primary }]);
        let lifted = device_frame(
            &mut t,
            0.4,
            vec![finger(1, TouchPhase::End, 420.0, 300.0), finger(2, TouchPhase::End, 480.0, 300.0), finger(3, TouchPhase::End, 520.0, 400.0)],
        );
        assert!(lifted.is_empty() && t.router.command.is_none());
        // Well after the pen, fingers move the camera again.
        device_frame(&mut t, 2.0, vec![finger(4, TouchPhase::Start, 400.0, 300.0)]);
        assert!(t.camera_active());
    }

    #[test]
    fn the_mouse_ends_touch_mode() {
        let mut t = Touch::default();
        device_frame(&mut t, 0.0, vec![finger(1, TouchPhase::Start, 400.0, 300.0)]);
        assert!(t.in_use);
        let mut raw = egui::RawInput { time: Some(0.1), ..Default::default() };
        raw.events.push(egui::Event::PointerButton {
            pos: pos2(1.0, 1.0),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        t.device_input(&mut raw, vec![], &on_camera);
        assert!(!t.in_use);
    }

    #[test]
    fn hostile_device_input_stays_bounded() {
        let mut t = Touch::default();
        let nan = f32::NAN;
        let mut events: Vec<_> = (0..1000).map(|i| finger(i, TouchPhase::Start, 400.0, 300.0)).collect();
        events.push(finger(5000, TouchPhase::Start, nan, nan));
        events.push(DeviceEvent::PenDown { pos: pos2(nan, 1.0), button: PointerButton::Primary });
        events.push(DeviceEvent::PenMove { pos: pos2(f32::INFINITY, 1.0) });
        device_frame(&mut t, f64::NAN, events);
        assert!(t.router.fingers.len() <= super::MAX_TOUCHES && t.router.ignored.len() <= super::MAX_TOUCHES);
        let ends: Vec<_> = (0..1000).map(|i| finger(i, TouchPhase::Cancel, 400.0, 300.0)).collect();
        device_frame(&mut t, 1.0, ends);
        // Every tracked finger has ended, so nothing is left in the camera.
        assert!(!t.camera_active());
    }

    #[test]
    fn a_pen_never_leaves_fingers_shut_out() {
        let mut t = Touch::default();
        let at = pos2(300.0, 300.0);
        // An up reporting another button still lifts the pen.
        device_frame(&mut t, 0.0, vec![DeviceEvent::PenDown { pos: at, button: PointerButton::Primary }]);
        device_frame(&mut t, 0.1, vec![DeviceEvent::PenUp { pos: at, button: PointerButton::Secondary }]);
        assert!(!t.router.pen_down);
        device_frame(&mut t, 2.0, vec![finger(1, TouchPhase::Start, 400.0, 300.0)]);
        assert!(t.camera_active(), "fingers move the view once the pen is up");
        device_frame(&mut t, 2.1, vec![finger(1, TouchPhase::End, 400.0, 300.0)]);
        // A pen hovering in the hand doesn't keep fingers away.
        device_frame(&mut t, 3.0, vec![DeviceEvent::PenMove { pos: at }]);
        device_frame(&mut t, 3.05, vec![DeviceEvent::PenMove { pos: at }, finger(2, TouchPhase::Start, 400.0, 300.0)]);
        assert!(t.camera_active(), "a hovering pen is not a palm");
    }

    #[test]
    fn a_lost_finger_end_does_not_stick() {
        let mut t = Touch::default();
        // Finger 1 starts and its end is lost; the browser reuses the id for the next touch.
        device_frame(&mut t, 0.0, vec![finger(1, TouchPhase::Start, 400.0, 300.0)]);
        device_frame(&mut t, 5.0, vec![finger(1, TouchPhase::Start, 420.0, 300.0)]);
        assert!(t.camera_active());
        assert_eq!(t.router.fingers.len(), 1);
        device_frame(&mut t, 5.1, vec![finger(1, TouchPhase::End, 420.0, 300.0)]);
        assert!(!t.camera_active() && t.router.fingers.is_empty());
        // An id ignored as a palm whose end was lost is a finger again when it lands anew.
        device_frame(
            &mut t,
            6.0,
            vec![DeviceEvent::PenDown { pos: pos2(1.0, 1.0), button: PointerButton::Primary }, finger(7, TouchPhase::Start, 400.0, 300.0)],
        );
        device_frame(&mut t, 6.1, vec![DeviceEvent::PenUp { pos: pos2(1.0, 1.0), button: PointerButton::Primary }]);
        device_frame(&mut t, 9.0, vec![finger(7, TouchPhase::Start, 400.0, 300.0)]);
        assert!(t.camera_active(), "a reused id is not ignored forever");
    }

    // ---- the app: what the gestures do to the camera and the menus ----

    struct Frames {
        h: crate::scenario::Harness,
        time: f64,
    }

    impl Frames {
        fn new() -> Self {
            let mut h = crate::scenario::Harness::new();
            if let Err(e) = h.app.session.run_script(&solidcraft_engine::sample::script()) {
                panic!("sample: {e}");
            }
            h.frames(3);
            Frames { h, time: 100.0 }
        }

        fn frame(&mut self, events: Vec<DeviceEvent>) {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
                time: Some(self.time),
                focused: true,
                ..Default::default()
            };
            let ctx = self.h.ctx.clone();
            self.h.app.device_input(&ctx, &mut raw, events);
            self.h.app.raw_input_hook(&mut raw);
            let app = &mut self.h.app;
            let mut out = ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }

        fn centre(&self) -> egui::Pos2 {
            match self.h.app.touch.camera_rect {
                Some(r) => r.center(),
                None => panic!("no camera page was drawn"),
            }
        }
    }

    #[test]
    fn fingers_orbit_and_pinch_the_view_without_a_menu() {
        let mut f = Frames::new();
        f.frame(vec![]);
        let c = f.centre();
        let before = f.h.app.cam;
        f.frame(vec![finger(1, TouchPhase::Start, c.x, c.y)]);
        for i in 1..=12 {
            f.frame(vec![finger(1, TouchPhase::Move, c.x + 6.0 * i as f32, c.y)]);
        }
        f.frame(vec![finger(1, TouchPhase::End, c.x + 72.0, c.y)]);
        let orbited = f.h.app.cam;
        assert!((orbited.yaw - before.yaw).abs() > 1e-3, "one finger turns the view: {before:?} -> {orbited:?}");
        assert!(f.h.app.session.selection.is_empty(), "an orbit selects nothing");

        // Two fingers held still past the long-press time open no menu.
        f.frame(vec![finger(2, TouchPhase::Start, c.x - 40.0, c.y), finger(3, TouchPhase::Start, c.x + 40.0, c.y)]);
        for _ in 0..90 {
            f.frame(vec![]);
        }
        assert!(f.h.app.viewport.context_menu.is_none(), "a two-finger hold must not open the menu");
        // Spreading them zooms in.
        let distance = f.h.app.cam.distance;
        for i in 1..=10 {
            let d = 40.0 + 8.0 * i as f32;
            f.frame(vec![finger(2, TouchPhase::Move, c.x - d, c.y), finger(3, TouchPhase::Move, c.x + d, c.y)]);
        }
        f.frame(vec![finger(2, TouchPhase::End, c.x - 120.0, c.y), finger(3, TouchPhase::End, c.x + 120.0, c.y)]);
        assert!(f.h.app.cam.distance < distance * 0.8, "spread zooms in: {distance} -> {}", f.h.app.cam.distance);
        assert!(f.h.app.viewport.context_menu.is_none());
    }

    #[test]
    fn a_two_finger_tap_undoes() {
        let mut f = Frames::new();
        f.frame(vec![]);
        let c = f.centre();
        let revision = f.h.app.session.revision;
        f.frame(vec![finger(1, TouchPhase::Start, c.x - 30.0, c.y), finger(2, TouchPhase::Start, c.x + 30.0, c.y)]);
        f.frame(vec![]);
        f.frame(vec![finger(1, TouchPhase::End, c.x - 30.0, c.y), finger(2, TouchPhase::End, c.x + 30.0, c.y)]);
        f.frame(vec![]);
        assert_ne!(f.h.app.session.revision, revision, "the tap undid the last step");
    }

    /// Touch and pen get bigger handles and wider grab areas; a mouse keeps the sizes the app
    /// always had, and a point reaches further than a line so it wins where both are in reach.
    #[test]
    fn touch_and_pen_get_bigger_handles_and_a_mouse_keeps_its_own() {
        use super::Sizes;
        let mut t = Touch::default();
        assert_eq!(t.sizes(), Sizes::MOUSE);
        t.in_use = true;
        assert_eq!(t.sizes(), Sizes::TOUCH);
        t.in_use = false;
        assert_eq!(t.sizes(), Sizes::MOUSE, "using the mouse again brings the old sizes back");
        let (m, f) = (Sizes::MOUSE, Sizes::TOUCH);
        // What the picker, the triad and the text frame used before.
        assert_eq!((m.point, m.vertex, m.curve, m.edge, m.axis), (8.0, 7.0, 6.0, 6.0, 5.0));
        assert_eq!((m.ball, m.arrow, m.ring, m.frame), (9.0, 6.0, 5.0, 8.0));
        assert_eq!((m.dot, m.halo, m.line_halo), (1.0, 0.0, 0.0), "a mouse draws no halo and the dots as always");
        for (mouse, touch, what) in [
            (m.point, f.point, "point"),
            (m.vertex, f.vertex, "vertex"),
            (m.curve, f.curve, "curve"),
            (m.edge, f.edge, "edge"),
            (m.axis, f.axis, "axis"),
            (m.ball, f.ball, "ball"),
            (m.arrow, f.arrow, "arrow"),
            (m.ring, f.ring, "ring"),
            (m.frame, f.frame, "frame"),
        ] {
            assert!(touch > mouse, "{what}: {touch} should exceed {mouse}");
        }
        assert!(f.dot > 1.0 && f.halo > 0.0 && f.line_halo > 0.0);
        assert!(f.point > f.curve && f.vertex > f.edge, "a point wins over a line in reach of both");
        // The picker looks this far round the pointer.
        assert_eq!(m.widest(), 8.0);
        assert_eq!(f.widest(), f.point);
        // Each size is finite and bounded.
        for v in [f.point, f.vertex, f.curve, f.edge, f.axis, f.ball, f.arrow, f.ring, f.frame, f.dot, f.halo, f.line_halo] {
            assert!(v.is_finite() && v > 0.0 && v < 100.0, "{v}");
        }
    }
}
