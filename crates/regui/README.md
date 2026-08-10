# regui

[![egui_ver](https://img.shields.io/badge/egui-0.35.0-blue)](https://github.com/emilk/egui)
[![Latest version](https://img.shields.io/crates/v/regui.svg)](https://crates.io/crates/regui)
[![Documentation](https://docs.rs/regui/badge.svg)](https://docs.rs/regui)
[![unsafe forbidden](https://img.shields.io/badge/unsafe-forbidden-success.svg)](https://github.com/rust-secure-code/safety-dance/)
[![License](https://img.shields.io/crates/l/regui.svg)](https://crates.io/crates/regui)



[content]:<>

**re**tained **egui** — render an egui ui inside another egui ui.

`regui` runs a part of your ui in its own egui viewport and paints the result into the
parent ui. Because the child is a real viewport, you can move, scale and rotate it, and
with the `wgpu` feature it renders into a texture, which lets you run shaders over it and
— the point of the name — keep the result and stop running the child until something it
shows actually changes.

The child shares the parent's `Context`, so it shares memory, style and fonts. It looks
and behaves like the rest of your app, but it gets its own input, its own hit-testing and
its own focus.

```rust
# egui::__run_test_ui(|ui| {
use regui::Regui;

Regui::new("preview")
    .size(egui::vec2(320.0, 240.0))
    .scale(0.5)
    .show(ui, |ui| {
        ui.heading("I am half the size");
        ui.button("...and still clickable").clicked();
    });
# });
```

## How it renders

By default `Regui` tessellates the child itself and hands the triangles to the parent's
painter, so it works with any egui backend — no wgpu needed. An untransformed child comes
out pixel for pixel identical to the same ui drawn straight into the parent.

With the `wgpu` feature, `.offscreen(true)` renders the child into a texture instead and
draws that. This clips a rotated child exactly, keeps text crisp at any scale, and lets a
shader work on the child's image — `.blur(radius)` blurs the child's own content:

```rust
# egui::__run_test_ui(|ui| {
# #[cfg(feature = "wgpu")]
regui::Regui::new("preview")
    .size(egui::vec2(200.0, 120.0))
    .blur(8.0)
    .interactive(false)
    .show(ui, |ui| {
        ui.label("out of focus");
    });
# });
```

The child is rendered with the parent's own `egui_wgpu::Renderer`, so it shares the font
atlas and every other texture, and costs one extra render pass rather than a second
renderer.

## Retained rendering

Once the child is a texture, a pass that would draw the same thing again is a pass worth
skipping. Hand `.retain(key)` a hash of the state the child displays and it draws the
image it already has — no layout, no tessellation, no render pass — until that hash
changes:

```rust
# egui::__run_test_ui(|ui| {
# #[cfg(feature = "wgpu")]
# let (unread, selected) = (3u32, 1u32);
# #[cfg(feature = "wgpu")]
regui::Regui::new("sidebar")
    .size(egui::vec2(240.0, 600.0))
    .retain(u64::from(unread) << 32 | u64::from(selected))
    .show_retained(ui, |ui| {
        ui.label("expensive to lay out, rarely different");
    });
# });
```

The transform is applied to the quad rather than baked into the image, so a retained child
can still be moved, scaled and rotated for free.

`regui` runs the child anyway, key or no key, whenever it could not sit still: while the
pointer is over it, while anything inside it has keyboard focus, while a menu is open or a
widget is being dragged inside it, and until every repaint it asked for has been served.
Size, scale, style and theme are folded into the key for you. What is left for the caller
is the state the ui function reads — get that wrong and the panel shows something stale.

## Caveats

- The default backend clips axis-aligned, because egui's clip rectangles are. Rotate a
  child and its clip rectangles grow to their bounding boxes, so content that should be cut
  off at the edge of a scroll area can spill a little. `.offscreen(true)` clips exactly.
- Child popups and tooltips cannot leave the child's rect: all of the child's shapes go
  into a single parent layer.
- The child is not in the parent's accessibility tree.
- A paint callback inside a rotated child is dropped, since a paint callback draws into an
  axis-aligned rectangle of the screen.
- A retained child's viewport is dropped while it is skipped, so anything it kept in
  per-viewport memory (an area's remembered position) starts over when it wakes. Open
  menus and drags keep it awake precisely because of this.
- Needs `egui::Context::run_hosted_viewport`, which is not in a released egui yet.
