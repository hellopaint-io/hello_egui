//! Tests for [`regui::Regui::auto_size`], which shrinks a child to what it lays out.

#![cfg(feature = "wgpu")]

use std::cell::Cell;
use std::rc::Rc;

use egui::{Rect, Vec2, vec2};
use egui_kittest::{
    Harness,
    wgpu::{WgpuTestRenderer, create_render_state, default_wgpu_setup},
};
use regui::Regui;

const SIZE: [f32; 2] = [400.0, 300.0];

/// Far larger than anything the tests below put in it, so a child that comes back this big
/// is one that did not shrink at all.
const MAX: Vec2 = vec2(360.0, 260.0);

const BUTTON: Vec2 = vec2(60.0, 24.0);

fn render_state() -> egui_wgpu::RenderState {
    create_render_state(
        default_wgpu_setup(),
        egui_wgpu::RendererOptions::PREDICTABLE,
    )
}

/// Where the parent reserved the child, which is the only place its size is observable
/// from outside.
type Placed = Rc<Cell<Rect>>;

/// How many times the content function was called, measuring passes included.
type Calls = Rc<Cell<u32>>;

fn child(
    auto_size: bool,
    content: impl FnMut(&mut egui::Ui) + 'static,
) -> (Harness<'static>, Placed, Calls) {
    let state = render_state();
    let installed = state.clone();
    let placed = Placed::new(Cell::new(Rect::NOTHING));
    let reported = Rc::clone(&placed);
    let calls = Calls::default();
    let counted = Rc::clone(&calls);
    let mut content = content;

    let harness = Harness::builder()
        .with_size(SIZE)
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui(move |ui| {
            regui::install_wgpu(ui.ctx(), installed.clone());
            let output = Regui::new("child")
                .size(MAX)
                .auto_size(auto_size)
                .show(ui, |ui| {
                    counted.set(counted.get() + 1);
                    content(ui);
                });
            reported.set(output.response.rect);
        });
    (harness, placed, calls)
}

/// The parent's rect for the child, once it has settled — an auto-sized child is laid out
/// at its maximum on its first pass, since it has not been measured yet.
fn settled(auto_size: bool, content: impl FnMut(&mut egui::Ui) + 'static) -> Rect {
    let (mut harness, placed, _calls) = child(auto_size, content);
    for _ in 0..4 {
        harness.run();
    }
    placed.get()
}

#[test]
fn a_child_shrinks_to_what_it_laid_out() {
    let rect = settled(true, |ui| {
        let _ = ui.allocate_exact_size(BUTTON, egui::Sense::click());
    });
    // Room for the ui's own margin around the button, but nowhere near the whole maximum.
    assert!(
        rect.width() < MAX.x * 0.5 && rect.height() < MAX.y * 0.5,
        "expected a child the size of what it holds, got {rect:?} out of a maximum of {MAX:?}"
    );
    assert!(
        rect.width() >= BUTTON.x && rect.height() >= BUTTON.y,
        "the child shrank past its own contents: {rect:?}"
    );
}

#[test]
fn without_it_the_child_is_the_size_it_was_given() {
    // The control for the test above: same contents, and the child is its full size.
    let rect = settled(false, |ui| {
        let _ = ui.allocate_exact_size(BUTTON, egui::Sense::click());
    });
    assert!(
        (rect.size() - MAX).length() < 0.5,
        "expected the size it was given ({MAX:?}), got {rect:?}"
    );
}

#[test]
fn a_child_never_grows_past_the_size_it_was_given() {
    let rect = settled(true, |ui| {
        let _ = ui.allocate_exact_size(vec2(10_000.0, 10_000.0), egui::Sense::hover());
    });
    assert!(
        rect.width() <= MAX.x + 0.5 && rect.height() <= MAX.y + 0.5,
        "expected the maximum to hold, got {rect:?}"
    );
}

#[test]
fn a_child_follows_its_contents_when_they_change() {
    let big = Rc::new(Cell::new(false));
    let toggled = Rc::clone(&big);
    let (mut harness, placed, _calls) = child(true, move |ui| {
        let size = if toggled.get() { BUTTON * 3.0 } else { BUTTON };
        let _ = ui.allocate_exact_size(size, egui::Sense::click());
    });
    for _ in 0..4 {
        harness.run();
    }
    let small = placed.get();

    big.set(true);
    for _ in 0..4 {
        harness.run();
    }
    let grown = placed.get();

    assert!(
        grown.width() > small.width() + BUTTON.x && grown.height() > small.height() + BUTTON.y,
        "expected the child to grow with its contents: {small:?} -> {grown:?}"
    );
}

#[test]
fn measuring_costs_a_pass_of_the_content() {
    // The price of `auto_size`, stated so that anyone who makes it free has a test to
    // delete rather than a surprise to discover.
    //
    // One settled pass each, not a whole `run`: how many passes egui needs to settle is not
    // the thing being measured, and it is not the same for both.
    fn calls_in_one_settled_pass(auto_size: bool) -> u32 {
        let (mut harness, _placed, calls) = child(auto_size, |ui| {
            let _ = ui.allocate_exact_size(BUTTON, egui::Sense::click());
        });
        for _ in 0..8 {
            harness.step();
        }
        let before = calls.get();
        harness.step();
        calls.get() - before
    }

    assert_eq!(
        calls_in_one_settled_pass(true),
        2 * calls_in_one_settled_pass(false),
        "expected one measuring pass per real pass"
    );
}

#[test]
fn a_menu_inside_the_child_is_measured_too() {
    // A child sized to its contents alone would clip its own menu, which is worse than
    // being too big.
    let (mut harness, placed, _calls) = child(true, |ui| {
        let response = ui.allocate_response(BUTTON, egui::Sense::click());
        egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&response));
        egui::Popup::menu(&response).show(|ui| {
            ui.set_min_size(vec2(160.0, 120.0));
            ui.label("a menu with some room in it");
        });
    });
    for _ in 0..4 {
        harness.run();
    }
    let rect = placed.get();
    assert!(
        rect.width() >= 160.0 && rect.height() >= 120.0 + BUTTON.y,
        "expected the open menu to be inside the child, got {rect:?}"
    );
}
