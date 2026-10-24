//! Pens and fingers in the browser.
//!
//! eframe reports a finger as the mouse as well as a touch, and a pen as either, so the app
//! couldn't tell them apart (a two-finger hold was also a long mouse press, which opened the
//! context menu). Pointer events say which device they came from: these listeners take pen and
//! touch pointer events (and the touch events that follow them) on their way down to the canvas,
//! before eframe sees them, and queue them as [`DeviceEvent`]s for
//! [`solidcraft_ui_egui::SolveApp::device_input`]. Mouse input still goes to eframe unchanged.

use std::cell::RefCell;
use std::rc::Rc;

use solidcraft_ui_egui::touch::DeviceEvent;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;

/// Mouse events this soon (ms) after a pen or touch pointer event are the browser's emulation
/// of it, not a mouse.
const EMULATED_MOUSE_MS: f64 = 800.0;
/// At most this many pens and fingers are tracked at once.
const MAX_POINTERS: usize = 16;

/// Pen and finger events since the last frame.
pub type Queue = Rc<RefCell<Vec<DeviceEvent>>>;

#[derive(Default)]
struct State {
    /// Pens and fingers down on the canvas: pointer id and whether it is a pen.
    active: Vec<(i32, bool)>,
    /// When (event time stamp, ms) a pen or finger was last seen.
    last_ms: f64,
}

/// Start listening on `window` for pens and fingers on `canvas`.
pub fn install(window: &web_sys::Window, canvas: &web_sys::HtmlCanvasElement, ctx: &egui::Context) -> Queue {
    let queue: Queue = Rc::default();
    let state = Rc::new(RefCell::new(State::default()));
    // The browser must not scroll, zoom or show its callout for touches on the canvas: the app
    // handles them.
    let style = canvas.style();
    for (name, value) in [("touch-action", "none"), ("-webkit-touch-callout", "none"), ("-webkit-user-select", "none"), ("user-select", "none")] {
        let _ = style.set_property(name, value);
    }

    for kind in ["pointerdown", "pointermove", "pointerup", "pointercancel", "pointerleave"] {
        let (queue, state, canvas, ctx) = (queue.clone(), state.clone(), canvas.clone(), ctx.clone());
        listen(window, kind, move |event: web_sys::Event| {
            let Some(event) = event.dyn_ref::<web_sys::PointerEvent>() else { return };
            pointer(event, &canvas, &ctx, &state, &queue);
        });
    }
    // The touch events that follow a finger's (or iPad pen's) pointer events: eframe would turn
    // them into the mouse again.
    for kind in ["touchstart", "touchmove", "touchend", "touchcancel"] {
        let canvas = canvas.clone();
        listen(window, kind, move |event: web_sys::Event| {
            if on(&event, &canvas) {
                event.stop_propagation();
                if event.cancelable() {
                    event.prevent_default();
                }
            }
        });
    }
    // Safari's pinch events zoom the page (and eframe reads them as trackpad pinches) while
    // fingers are down; a trackpad pinch, with no fingers on the screen, still goes through.
    for kind in ["gesturestart", "gesturechange", "gestureend"] {
        let state = state.clone();
        listen(window, kind, move |event: web_sys::Event| {
            if state.borrow().active.iter().any(|(_, pen)| !pen) {
                event.stop_propagation();
                event.prevent_default();
            }
        });
    }
    // The mouse events a browser makes up for a pen or a finger.
    for kind in ["mousemove", "mousedown", "mouseup"] {
        let state = state.clone();
        listen(window, kind, move |event: web_sys::Event| {
            if (event.time_stamp() - state.borrow().last_ms).abs() < EMULATED_MOUSE_MS {
                event.stop_propagation();
            }
        });
    }
    queue
}

/// One pen or finger pointer event.
fn pointer(event: &web_sys::PointerEvent, canvas: &web_sys::HtmlCanvasElement, ctx: &egui::Context, state: &RefCell<State>, queue: &Queue) {
    let pen = match event.pointer_type().as_str() {
        "pen" => true,
        "touch" => false,
        // The mouse stays eframe's.
        _ => return,
    };
    let id = event.pointer_id();
    let kind = event.type_();
    let mut st = state.borrow_mut();
    let tracked = st.active.iter().any(|(i, _)| *i == id);
    let here = on(event, canvas);
    // A pen or finger is ours from when it lands on the canvas until it lifts. A pen hovering
    // over the canvas is ours too, but not tracked: nothing tells us reliably when it goes.
    let hover = pen && !tracked && here && matches!(kind.as_str(), "pointermove" | "pointerleave");
    let lands = here && kind == "pointerdown";
    if !(tracked || hover || lands) {
        return;
    }
    if lands && !tracked {
        // Never let one through to eframe (it would be the mouse again): the oldest goes, its
        // end having been lost.
        if st.active.len() >= MAX_POINTERS {
            let lost = st.active.remove(0);
            solidcraft_ui_egui::debug_log::push(format!("browser: dropped pointer {} (its end never came)", lost.0));
        }
        st.active.push((id, pen));
    }
    // A tracked pen leaving the canvas while down still comes back up here.
    if tracked && kind == "pointerleave" {
        return;
    }
    st.last_ms = event.time_stamp();
    event.stop_propagation();
    let pos = at(event, canvas, ctx);
    // Which of the pen's buttons (tip 0, barrel 2), not a widget.
    let button = match web_sys::MouseEvent::button(event) {
        1 => egui::PointerButton::Middle,
        2 => egui::PointerButton::Secondary,
        _ => egui::PointerButton::Primary,
    };
    let ended = matches!(kind.as_str(), "pointerup" | "pointercancel");
    let device = if pen {
        match kind.as_str() {
            "pointerdown" => {
                // No mouse events made up from it; the canvas still takes the keyboard.
                event.prevent_default();
                let _ = canvas.focus();
                Some(DeviceEvent::PenDown { pos, button })
            }
            "pointermove" => Some(DeviceEvent::PenMove { pos }),
            "pointerup" => Some(DeviceEvent::PenUp { pos, button }),
            "pointercancel" => Some(DeviceEvent::PenCancel),
            "pointerleave" => Some(DeviceEvent::PenLeave),
            _ => None,
        }
    } else {
        let phase = match kind.as_str() {
            "pointerdown" => {
                event.prevent_default();
                let _ = canvas.focus();
                Some(egui::TouchPhase::Start)
            }
            "pointermove" => Some(egui::TouchPhase::Move),
            "pointerup" => Some(egui::TouchPhase::End),
            "pointercancel" => Some(egui::TouchPhase::Cancel),
            _ => None,
        };
        phase.map(|phase| DeviceEvent::Finger { id: u64::from(id.unsigned_abs()), phase, pos })
    };
    // Lifted (or taken by the system), it is no longer tracked; a lifted pen may still hover.
    if ended {
        st.active.retain(|(i, _)| *i != id);
    }
    drop(st);
    if let Some(device) = device {
        queue.borrow_mut().push(device);
        ctx.request_repaint();
    }
    if ended {
        refocus_text_field();
    }
}

/// iOS shows the keyboard only for a field focused while a touch is being handled: as eframe
/// does on a touch's end, a focused text field (eframe's text agent) is focused again here.
fn refocus_text_field() {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
    let Some(input) = document.active_element().and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok()) else { return };
    let _ = input.blur();
    let _ = input.focus();
}

/// The event happened on the canvas.
fn on(event: &web_sys::Event, canvas: &web_sys::HtmlCanvasElement) -> bool {
    let canvas: &wasm_bindgen::JsValue = canvas.as_ref();
    event.target().is_some_and(|t| {
        let t: &wasm_bindgen::JsValue = t.as_ref();
        t == canvas
    })
}

/// The pointer's position in egui points, as eframe works it out for the mouse.
fn at(event: &web_sys::PointerEvent, canvas: &web_sys::HtmlCanvasElement, ctx: &egui::Context) -> egui::Pos2 {
    let rect = canvas.get_bounding_client_rect();
    let zoom = ctx.zoom_factor();
    let zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
    egui::pos2((event.client_x() as f32 - rect.left() as f32) / zoom, (event.client_y() as f32 - rect.top() as f32) / zoom)
}

/// Listen for `kind` on `window` in the capture phase (before the canvas and eframe), able to
/// cancel the browser's own handling. The listener lives as long as the page.
fn listen(window: &web_sys::Window, kind: &str, handler: impl FnMut(web_sys::Event) + 'static) {
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(handler);
    let options = web_sys::AddEventListenerOptions::new();
    options.set_capture(true);
    options.set_passive(false);
    let _ = window.add_event_listener_with_callback_and_add_event_listener_options(kind, closure.as_ref().unchecked_ref(), &options);
    closure.forget();
}
