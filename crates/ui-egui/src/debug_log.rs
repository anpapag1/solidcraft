//! An on-screen log for finding out what touch, pen and the number pad did on a device with no
//! developer tools at hand (a tablet): Preferences › Interface › Show the debug log, or `?debug`
//! in the browser's address.
//!
//! A small badge in the 3D view's bottom-left corner counts the messages (red once an error has
//! come); tapping it opens the latest messages, newest first, with Copy and Clear. Anything may
//! [`push`] a line; errors shown in the status bar are added as errors.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use egui::{Pos2, Rect, vec2};

use crate::SolveApp;
use crate::theme::Tokens;
use crate::ui_kit::{self, Container, Surface};

/// The most lines kept; older ones go.
const KEEP: usize = 300;

#[derive(Clone, Debug)]
struct Line {
    /// Seconds since the app started (egui's clock).
    at: f64,
    error: bool,
    text: String,
}

#[derive(Default)]
struct Log {
    lines: VecDeque<Line>,
    /// Errors since the panel was last opened.
    unseen_errors: usize,
    open: bool,
    /// The badge and the panel as drawn last frame (a finger there is the mouse).
    drawn: Vec<Rect>,
}

thread_local! {
    static LOG: RefCell<Log> = RefCell::new(Log::default());
    static NOW: Cell<f64> = const { Cell::new(0.0) };
    /// The screen size last logged.
    static SCREEN: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
}

/// Add a line.
pub fn push(text: impl Into<String>) {
    add(text.into(), false);
}

/// Add an error line.
pub fn error(text: impl Into<String>) {
    add(text.into(), true);
}

fn add(text: String, error: bool) {
    let at = NOW.with(Cell::get);
    LOG.with(|log| {
        let mut log = log.borrow_mut();
        // The same line again only bumps its time: a busy loop doesn't flush the rest.
        if let Some(last) = log.lines.back_mut()
            && last.text == text
            && last.error == error
        {
            last.at = at;
            return;
        }
        if log.lines.len() >= KEEP {
            log.lines.pop_front();
        }
        if error {
            log.unseen_errors += 1;
        }
        log.lines.push_back(Line { at, error, text });
    });
}

/// The badge or the open panel covers this point.
pub(crate) fn covers(p: Pos2) -> bool {
    LOG.with(|log| log.borrow().drawn.iter().any(|r| r.contains(p)))
}

/// The lines, oldest first (tests).
#[cfg(test)]
pub(crate) fn lines() -> Vec<String> {
    LOG.with(|log| log.borrow().lines.iter().map(|l| l.text.clone()).collect())
}

/// Draw the badge (and the panel, when open) in the bottom-left corner of the 3D view, if the
/// preference is on. Call once a frame, after the view.
pub(crate) fn show(app: &SolveApp, ctx: &egui::Context) {
    NOW.with(|now| now.set(ctx.input(|i| i.time)));
    if !app.preferences.debug_log {
        LOG.with(|log| log.borrow_mut().drawn.clear());
        return;
    }
    // The screen's size in points and its pixel ratio, whenever either changes: what layout
    // decisions (the compact ribbon, the folded browser) are made from.
    let screen = ctx.content_rect();
    let key = (screen.width().round() as i32, (ctx.pixels_per_point() * 100.0).round() as i32);
    if SCREEN.with(|s| s.replace(key)) != key {
        push(format!("screen {} x {} pt, {:.2} pixels per point", key.0, screen.height().round(), key.1 as f32 / 100.0));
    }
    let t = Tokens::get();
    let view = app.viewport.layout.as_ref().map(|l| l.drawable).filter(|r| r.is_finite() && r.is_positive()).unwrap_or(ctx.content_rect());
    let corner = view.left_bottom() + vec2(t.space_8, -t.space_8);
    let (count, errors, open) = LOG.with(|log| {
        let log = log.borrow();
        (log.lines.len(), log.unseen_errors, log.open)
    });
    let mut drawn = Vec::new();
    let mut toggle = false;
    let mut clear = false;
    let mut copy = None;
    let badge = Container::new(ctx, Surface::Canvas)
        .area(egui::Id::new("sc_debug_badge"))
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(corner)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            Container::popup_frame(ui.style()).fill(if errors > 0 { t.form_error_fill } else { t.dialog_body }).show(ui, |ui| {
                let label = if errors > 0 { format!("Debug  {count}  ·  {errors} error(s)") } else { format!("Debug  {count}") };
                if ui_kit::button(ui, label, ui_kit::Button::Small).clicked() {
                    toggle = true;
                }
            });
        });
    drawn.push(badge.response.rect);
    if open {
        let width = (view.width() - t.space_8 * 2.0).clamp(160.0, 460.0);
        let height = (view.height() * 0.5).clamp(120.0, 340.0);
        let panel = Container::new(ctx, Surface::Canvas)
            .area(egui::Id::new("sc_debug_panel"))
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(badge.response.rect.left_top() - vec2(0.0, t.space_4))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                Container::popup_frame(ui.style()).fill(t.dialog_body).show(ui, |ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        ui_kit::text(ui, "Debug log (newest first)", ui_kit::Text::Heading("debug_log"));
                        if ui_kit::button(ui, "Copy", ui_kit::Button::Small).clicked() {
                            copy = Some(LOG.with(|log| {
                                log.borrow()
                                    .lines
                                    .iter()
                                    .map(|l| format!("{:9.2}s {}{}", l.at, if l.error { "ERROR " } else { "" }, l.text))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            }));
                        }
                        if ui_kit::button(ui, "Clear", ui_kit::Button::Small).clicked() {
                            clear = true;
                        }
                    });
                    egui::ScrollArea::vertical().max_height(height).auto_shrink([false, true]).show(ui, |ui| {
                        let lines: Vec<Line> = LOG.with(|log| log.borrow().lines.iter().rev().cloned().collect());
                        if lines.is_empty() {
                            ui_kit::text(ui, "Nothing yet.", ui_kit::Text::Hint("debug_log"));
                        }
                        for l in lines {
                            let text = format!("{:7.2}s  {}", l.at, l.text);
                            ui_kit::text(ui, &text, if l.error { ui_kit::Text::Error("debug_log") } else { ui_kit::Text::MonoNote("debug_log") });
                        }
                    });
                });
            });
        drawn.push(panel.response.rect);
    }
    if let Some(text) = copy {
        ctx.copy_text(text);
    }
    LOG.with(|log| {
        let mut log = log.borrow_mut();
        if toggle {
            log.open = !log.open;
        }
        if log.open {
            log.unseen_errors = 0;
        }
        if clear {
            log.lines.clear();
            log.unseen_errors = 0;
        }
        log.drawn = drawn;
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn repeats_fold_and_old_lines_go() {
        for i in 0..(super::KEEP + 20) {
            super::push(format!("line {i}"));
            super::push(format!("line {i}"));
        }
        let lines = super::lines();
        assert_eq!(lines.len(), super::KEEP);
        assert_eq!(lines.last().map(String::as_str), Some(format!("line {}", super::KEEP + 19).as_str()));
    }

    #[test]
    fn the_badge_shows_only_when_turned_on() {
        let mut h = crate::scenario::Harness::new();
        h.frames(2);
        assert!(!super::covers(egui::pos2(300.0, 900.0)) && super::LOG.with(|l| l.borrow().drawn.is_empty()));
        h.app.preferences.debug_log = true;
        super::error("something failed");
        h.frames(2);
        let badge = super::LOG.with(|l| l.borrow().drawn.first().copied());
        let Some(badge) = badge else { panic!("no badge") };
        assert!(super::covers(badge.center()));
        assert!(super::lines().iter().any(|l| l.starts_with("screen ")), "the screen size is logged once the log is on");
        assert!(h.widget("Debug  2  ·  1 error(s)").is_some(), "the badge counts the screen line and the error");
        h.step(&serde_json::json!({"click": [badge.center().x, badge.center().y]})).unwrap();
        h.frames(2);
        assert!(h.widget("Copy").is_some(), "the panel opened");
        assert!(super::LOG.with(|l| l.borrow().unseen_errors == 0));
    }
}
