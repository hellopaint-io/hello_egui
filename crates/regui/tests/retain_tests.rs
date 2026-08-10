//! Tests for `Regui::retain`, which draws the image from a previous pass instead of
//! running the child again.

#![cfg(feature = "wgpu")]

use std::cell::Cell;
use std::rc::Rc;

use egui::{Color32, Event, Modifiers, PointerButton, Pos2, Rect, Vec2, vec2};
use egui_kittest::{
    Harness,
    wgpu::{WgpuTestRenderer, create_render_state, default_wgpu_setup},
};
use regui::{Regui, Transform};

const SIZE: [f32; 2] = [320.0, 220.0];
const CHILD: Vec2 = vec2(240.0, 150.0);

/// Somewhere the pointer is not over the child.
const OUTSIDE: Pos2 = Pos2::new(300.0, 210.0);

/// How many times the ui function has run.
type Runs = Rc<Cell<u32>>;

/// Where the child ended up in the parent, so a test can aim at something inside it.
///
/// `regui` keeps the child out of the parent's accessibility tree, so kittest cannot find
/// a widget in there by label - the transform is how you reach one.
type Placement = Rc<Cell<Transform>>;

/// A harness whose child is retained on `key`, counting the passes it actually runs.
fn harness(
    key: Rc<Cell<u64>>,
    mut build: impl FnMut(&mut egui::Ui) + 'static,
) -> (Harness<'static>, Runs, Placement) {
    let render_state = create_render_state(
        default_wgpu_setup(),
        egui_wgpu::RendererOptions::PREDICTABLE,
    );
    let installed = render_state.clone();
    let runs = Runs::default();
    let counted = Rc::clone(&runs);
    let placement: Placement = Rc::new(Cell::new(Transform::IDENTITY));
    let placed = Rc::clone(&placement);

    let harness = Harness::builder()
        .with_size(SIZE)
        .renderer(WgpuTestRenderer::from_render_state(render_state))
        .build_ui(move |ui| {
            regui::install_wgpu(ui.ctx(), installed.clone());
            let rect = ui.ctx().viewport_rect();
            ui.painter().rect_filled(rect, 0.0, Color32::DARK_GRAY);
            let output = Regui::new("child")
                .size(CHILD)
                .retain(key.get())
                .show_retained(ui, |ui| {
                    counted.set(counted.get() + 1);
                    build(ui);
                });
            placed.set(output.transform);
        });
    (harness, runs, placement)
}

/// Settle the harness with the pointer parked away from the child.
fn settle(harness: &mut Harness<'_>) {
    harness
        .input_mut()
        .events
        .push(Event::PointerMoved(OUTSIDE));
    harness.run();
}

#[test]
fn an_unchanged_child_stops_running() {
    let key = Rc::new(Cell::new(0));
    let (mut harness, runs, _) = harness(Rc::clone(&key), |ui| {
        ui.label("nothing ever happens here");
    });

    settle(&mut harness);
    let settled = runs.get();

    for _ in 0..5 {
        harness.step();
    }
    assert_eq!(
        runs.get(),
        settled,
        "a child that is unchanged, unhovered and unfocused should not have run again"
    );
}

#[test]
fn a_changed_key_runs_the_child_again() {
    let key = Rc::new(Cell::new(0));
    let (mut harness, runs, _) = harness(Rc::clone(&key), |ui| {
        ui.label("still nothing");
    });

    settle(&mut harness);
    harness.step();
    let settled = runs.get();

    key.set(1);
    harness.step();
    assert!(
        runs.get() > settled,
        "the child should run again once its content key changes"
    );
}

#[test]
fn a_hovered_child_keeps_running() {
    let key = Rc::new(Cell::new(0));
    let (mut harness, runs, _) = harness(Rc::clone(&key), |ui| {
        ui.label("hover me");
    });

    settle(&mut harness);

    harness
        .input_mut()
        .events
        .push(Event::PointerMoved(Pos2::new(40.0, 40.0)));
    harness.step();
    let hovered = runs.get();
    harness.step();
    assert!(
        runs.get() > hovered,
        "the child should keep running while the pointer is over it, so it can react to it"
    );
}

/// The pass that wakes a retained child has no widget rectangles from the pass before it,
/// so without a priming pass the click that woke it would land on nothing. This is what a
/// tap on a touch screen looks like: a press at a position that was never hovered.
#[test]
fn a_tap_that_wakes_the_child_still_lands() {
    let key = Rc::new(Cell::new(0));
    let clicks = Rc::new(Cell::new(0));
    let counted = Rc::clone(&clicks);
    let button_rect = Rc::new(Cell::new(Rect::ZERO));
    let recorded = Rc::clone(&button_rect);
    let (mut harness, _runs, placement) = harness(Rc::clone(&key), move |ui| {
        let response = ui.button("tap me");
        recorded.set(response.rect);
        if response.clicked() {
            counted.set(counted.get() + 1);
        }
    });

    settle(&mut harness);
    let button = placement.get().mul_pos(button_rect.get().center());

    for pressed in [true, false] {
        harness.input_mut().events.push(Event::PointerButton {
            pos: button,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        });
        harness.step();
    }

    assert_eq!(
        clicks.get(),
        1,
        "the tap that woke the retained child should have reached the button"
    );
}

/// A retained child is not running, so nothing inside it can ask for the repaint that would
/// let it run. If `regui` asked for one on its behalf the app would never idle.
#[test]
fn a_retained_child_lets_the_app_idle() {
    let key = Rc::new(Cell::new(0));
    let (mut harness, _runs, _) = harness(Rc::clone(&key), |ui| {
        ui.label("quiet");
    });

    settle(&mut harness);
    harness.step();
    harness.step();

    assert!(
        !harness.ctx.has_requested_repaint(),
        "a settled, retained child should leave nothing asking for another frame"
    );
}
