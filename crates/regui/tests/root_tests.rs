//! Tests for [`regui::RootScope`], the way out of a child into the ui hosting it.

#![cfg(feature = "wgpu")]

use std::cell::Cell;
use std::rc::Rc;

use egui::accesskit::Role;
use egui::{Color32, Pos2, Rect, Vec2, vec2};
use egui_kittest::kittest::{Queryable as _, by};
use egui_kittest::{
    Harness,
    wgpu::{WgpuTestRenderer, create_render_state, default_wgpu_setup},
};
use regui::{Regui, Transform};

const SIZE: [f32; 2] = [320.0, 220.0];
const CHILD: Vec2 = vec2(120.0, 80.0);

type Runs = Rc<Cell<u32>>;

thread_local! {
    /// Where the parent allocated the child, straight off the regui response - an answer
    /// arrived at without going through [`regui::RootScope`].
    static PLACED: Cell<Rect> = const { Cell::new(Rect::NOTHING) };
}

/// What the escaping closure saw, so a test can check where it was told to draw.
type Seen = Rc<Cell<Option<Rect>>>;

fn render_state() -> egui_wgpu::RenderState {
    create_render_state(
        default_wgpu_setup(),
        egui_wgpu::RendererOptions::PREDICTABLE,
    )
}

/// A child that hands a rect back out to its host, optionally rotated.
///
/// The rect it hands out is its own full screen, which is the simplest thing with corners
/// to check: mapped out of a rotated child it has to come back as a bounding box.
fn harness(rotation: f32, escape: bool) -> (Harness<'static>, Runs, Seen) {
    let state = render_state();
    let installed = state.clone();
    let runs = Runs::default();
    let counted = Rc::clone(&runs);
    let seen: Seen = Seen::default();
    let reported = Rc::clone(&seen);

    let harness = Harness::builder()
        .with_size(SIZE)
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui(move |ui| {
            regui::install_wgpu(ui.ctx(), installed.clone());
            ui.painter()
                .rect_filled(ui.ctx().viewport_rect(), 0.0, Color32::DARK_GRAY);
            let output = Regui::new("child")
                .size(CHILD)
                .rotation(rotation)
                // Retained on a key that never changes, so anything that keeps running is
                // doing so for a reason of regui's own.
                .retain(1)
                .show_retained_with_root(ui, |ui, root| {
                    counted.set(counted.get() + 1);
                    ui.painter().rect_filled(
                        ui.ctx().viewport_rect(),
                        0.0,
                        Color32::from_rgb(20, 80, 20),
                    );
                    if escape {
                        // Mapped before queuing: the closure outlives this borrow of the
                        // scope, so it cannot ask the scope anything once it is running.
                        let rect = root.rect(ui.ctx().viewport_rect());
                        let reported = Rc::clone(&reported);
                        root.ui(move |ui| {
                            // Painted by the host, so it is neither clipped to the child
                            // nor turned with it.
                            reported.set(Some(root_marker(ui, rect)));
                        });
                    }
                });
            PLACED.with(|placed| placed.set(output.response.rect));
        });
    (harness, runs, seen)
}

/// Draw something at `rect` and report where it went.
fn root_marker(ui: &mut egui::Ui, rect: Rect) -> Rect {
    ui.painter().rect_filled(rect, 0.0, Color32::RED);
    rect
}

#[test]
fn an_escaping_closure_runs_in_the_host() {
    let (mut harness, _runs, seen) = harness(0.0, true);
    harness.run();
    let rect = seen.get().expect("the escaping closure never ran");
    // Checked against where the parent actually put the child, not against the scope's own
    // transform, so this fails if the two ever disagree.
    let placed = PLACED.with(|placed| placed.get());
    assert!(
        (rect.min - placed.min).length() < 0.5 && (rect.size() - CHILD).length() < 0.5,
        "expected the child's screen mapped onto where the parent placed it ({placed:?}), \
         got {rect:?}"
    );
}

#[test]
fn a_rotated_child_hands_out_an_upright_rect() {
    let quarter_turn = std::f32::consts::FRAC_PI_2;
    let (mut harness, _runs, seen) = harness(quarter_turn, true);
    harness.run();
    let rect = seen.get().expect("the escaping closure never ran");

    // A quarter turn swaps the child's width and height, and the bounding box of the
    // turned rect is what comes back - upright, so a menu hung off it is upright too.
    assert!(
        (rect.width() - CHILD.y).abs() < 0.5 && (rect.height() - CHILD.x).abs() < 0.5,
        "expected the turned child's bounding box, got {rect:?}"
    );
}

#[test]
fn the_transform_composes_the_whole_way_out() {
    // A child inside a child: the inner one's way out has to account for what the outer
    // one is already doing to it, or a menu from the inner one lands at the offset of the
    // outer one only.
    let state = render_state();
    let installed = state.clone();
    let seen: Rc<Cell<Option<(Transform, Transform, Transform)>>> = Rc::default();
    let reported = Rc::clone(&seen);

    let mut harness = Harness::builder()
        .with_size(SIZE)
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui(move |ui| {
            regui::install_wgpu(ui.ctx(), installed.clone());
            let reported = Rc::clone(&reported);
            let outer = Regui::new("outer")
                .size(CHILD)
                .rotation(0.3)
                .show_with_root(ui, move |ui, _outer_root| {
                    Regui::new("inner")
                        .size(vec2(50.0, 40.0))
                        .rotation(0.2)
                        .show_with_root(ui, |_ui, inner_root| inner_root.transform())
                });
            let inner_to_root = outer.inner.inner;
            reported.set(Some((outer.transform, inner_to_root, Transform::IDENTITY)));
        });
    harness.run();

    let (outer_transform, inner_to_root, _) = seen.get().expect("the children never ran");
    // Whatever the inner child's own placement was, its way out must end up where the
    // outer child's placement would put it.
    let origin_via_scope = inner_to_root.mul_pos(Pos2::ZERO);
    let outer_only = outer_transform.mul_pos(Pos2::ZERO);
    assert!(
        (origin_via_scope - outer_only).length() > 0.5,
        "the inner child's way out ignored its own placement: {inner_to_root:?}"
    );
    // ...and it must be the outer transform applied to something, i.e. it must have turned
    // by at least as much as the outer child did.
    let turned = inner_to_root.bounding_rect(Rect::from_min_size(Pos2::ZERO, vec2(50.0, 40.0)));
    assert!(
        turned.width() > 50.0 && turned.height() > 40.0,
        "expected the inner child's rect to come out turned by both rotations, got {turned:?}"
    );
}

#[test]
fn a_child_with_nothing_out_there_still_retains() {
    let (mut harness, runs, _seen) = harness(0.0, false);
    for _ in 0..8 {
        harness.step();
    }
    let settled = runs.get();
    for _ in 0..5 {
        harness.step();
    }
    assert_eq!(
        runs.get(),
        settled,
        "the escape hatch should not cost retention to a child that never uses it"
    );
}

#[test]
fn a_child_that_escaped_keeps_running() {
    // What it put out there is drawn by the host, from a pass that would not happen if the
    // child were retained - so the menu would come down the moment it sat one out.
    let (mut harness, runs, _seen) = harness(0.0, true);
    for _ in 0..4 {
        harness.run();
    }
    assert!(
        runs.get() >= 4,
        "a child handing ui to its host must keep running, ran {} times",
        runs.get()
    );
}

#[test]
fn a_shut_popup_costs_the_child_nothing() {
    // The one that decides whether any of this is worth having. A menu is written the same
    // way whether it is open or not - the popup decides - so if handing one out cost a pass
    // every time the button was drawn, every child with a menu would run forever.
    let state = render_state();
    let installed = state.clone();
    let runs = Runs::default();
    let counted = Rc::clone(&runs);

    let mut harness = Harness::builder()
        .with_size(SIZE)
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui(move |ui| {
            regui::install_wgpu(ui.ctx(), installed.clone());
            Regui::new("child")
                .size(CHILD)
                .retain(1)
                .show_retained_with_root(ui, |ui, root| {
                    counted.set(counted.get() + 1);
                    let response = ui.button("Tools");
                    root.popup(&response, |ui, response| {
                        egui::Popup::menu(response).show(|ui| {
                            let _ = ui.button("Brush");
                        });
                    });
                });
        });

    for _ in 0..8 {
        harness.step();
    }
    let settled = runs.get();
    for _ in 0..5 {
        harness.step();
    }
    assert_eq!(
        runs.get(),
        settled,
        "a child whose menu is shut should still be able to sit a pass out"
    );
}

#[test]
fn what_escapes_may_borrow_from_around_the_child() {
    // A menu reads the state its button was drawn from, and that state is not `'static`.
    // If the queued closure had to be, every real call site would have to clone its world
    // into the menu - so this is a compile-time test with an assertion stapled on.
    let state = render_state();
    let installed = state.clone();
    let seen: Seen = Seen::default();
    let reported = Rc::clone(&seen);

    let mut harness = Harness::builder()
        .with_size(SIZE)
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui(move |ui| {
            regui::install_wgpu(ui.ctx(), installed.clone());
            // Borrowed by the escaping closure below, and gone by the end of this frame.
            let borrowed = vec2(11.0, 13.0);
            let reported = Rc::clone(&reported);
            Regui::new("child")
                .size(CHILD)
                .show_with_root(ui, |ui, root| {
                    let anchor = root.rect(ui.ctx().viewport_rect());
                    let reported = Rc::clone(&reported);
                    root.ui(move |ui| {
                        let rect = Rect::from_min_size(anchor.min, borrowed);
                        ui.painter().rect_filled(rect, 0.0, Color32::RED);
                        reported.set(Some(rect));
                    });
                });
        });
    harness.run();

    let rect = seen.get().expect("the escaping closure never ran");
    assert_eq!(rect.size(), vec2(11.0, 13.0));
}

#[test]
fn an_escaped_popup_is_not_clipped_to_the_child() {
    let state = render_state();
    let installed = state.clone();
    let seen: Rc<Cell<Option<(Rect, Rect)>>> = Rc::default();
    let reported = Rc::clone(&seen);

    let mut harness = Harness::builder()
        .with_size(SIZE)
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui(move |ui| {
            regui::install_wgpu(ui.ctx(), installed.clone());
            let output = Regui::new("child")
                .size(CHILD)
                .show_with_root(ui, |ui, root| {
                    let response = ui.button("Open");
                    let child_rect = response.rect;
                    let reported = Rc::clone(&reported);
                    root.popup(&response, move |ui, response| {
                        // The response has to be talking about the host's coordinates, or
                        // a menu built on it lands wherever the child's rect points.
                        ui.painter().rect_filled(response.rect, 0.0, Color32::RED);
                        reported.set(Some((child_rect, response.rect)));
                    });
                });
            PLACED.with(|placed| placed.set(output.response.rect));
        });
    // Clicked rather than forced open: which popup is open is per-viewport, so opening one
    // from inside the child opens it somewhere nobody is looking.
    harness.run();
    harness.get(by().role(Role::Button).label("Open")).click();
    harness.run();

    let (child_rect, host_rect) = seen.get().expect("the popup closure never ran");
    // The response the closure gets has to be talking about the host, not the child: same
    // widget, same size, moved by however far the child sits into the parent.
    assert!(
        (host_rect.size() - child_rect.size()).length() < 0.5,
        "the popup's response changed size: {child_rect:?} -> {host_rect:?}"
    );
    assert!(
        (host_rect.min - child_rect.min).length() > 1.0,
        "the popup's response was handed back in the child's coordinates ({host_rect:?}), \
         so a menu built on it would land inside the child"
    );
    let placed = PLACED.with(|placed| placed.get());
    assert!(
        placed.contains_rect(host_rect),
        "expected the moved response inside where the child was placed ({placed:?}), \
         got {host_rect:?}"
    );
}

#[test]
fn a_menu_handed_out_of_a_retained_child_opens_when_its_button_is_clicked() {
    // The whole point, end to end: a button inside a retained child, its menu drawn by the
    // host, and a click that has to travel into the child, come back out as a queued
    // closure, and land as an open menu in the host's tree.
    let state = render_state();
    let installed = state.clone();

    let mut harness = Harness::builder()
        .with_size(SIZE)
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui(move |ui| {
            regui::install_wgpu(ui.ctx(), installed.clone());
            Regui::new("child")
                .size(CHILD)
                .retain(1)
                .show_retained_with_root(ui, |ui, root| {
                    let tools = ui.button("Tools");
                    root.popup(&tools, |_ui, tools| {
                        egui::Popup::menu(tools).show(|ui| {
                            let _ = ui.button("Select");
                        });
                    });
                });
        });

    for _ in 0..4 {
        harness.run();
    }
    harness.get(by().role(Role::Button).label("Tools")).click();
    for _ in 0..4 {
        harness.run();
    }
    assert!(
        harness
            .query(by().role(Role::Button).label("Select"))
            .is_some(),
        "the menu never opened"
    );
}

#[test]
fn a_click_on_a_handed_out_menu_does_not_also_hit_the_child_underneath() {
    // The menu is drawn by the host, so nothing inside the child covers where it is any
    // more. Without regui noticing that, the child goes on hit-testing under an open menu
    // and one click lands twice: once on the menu item, once on whatever the menu is over.
    let state = render_state();
    let installed = state.clone();
    let underneath = Rc::new(Cell::new(0u32));
    let hits = Rc::clone(&underneath);

    let mut harness = Harness::builder()
        .with_size(SIZE)
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui(move |ui| {
            regui::install_wgpu(ui.ctx(), installed.clone());
            let hits = Rc::clone(&hits);
            Regui::new("child")
                .size(vec2(300.0, 200.0))
                // What a child laid over something the user is still working on has to do:
                // it is not the shape of its rect, so it decides for itself when the
                // pointer is its own. That is also how it loses track of being covered.
                .sense(egui::Sense::hover())
                .pointer(true)
                .show_with_root(ui, move |ui, root| {
                    let tools = ui.button("Tools");
                    // Directly below the button, which is where a menu dropping out of it
                    // lands - so a click on the menu's first item is over this too.
                    if ui.button("Underneath").clicked() {
                        hits.set(hits.get() + 1);
                    }
                    root.popup(&tools, |_ui, tools| {
                        egui::Popup::menu(tools).show(|ui| {
                            let _ = ui.button("Pick me");
                        });
                    });
                });
        });

    for _ in 0..4 {
        harness.run();
    }
    harness.get(by().role(Role::Button).label("Tools")).click();
    for _ in 0..4 {
        harness.run();
    }
    harness
        .get(by().role(Role::Button).label("Pick me"))
        .click();
    for _ in 0..4 {
        harness.run();
    }

    assert_eq!(
        underneath.get(),
        0,
        "the click went through the menu and hit the child underneath it"
    );
}
