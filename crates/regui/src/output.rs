use crate::Transform;
use egui::{AccessKitSubtree, Context, Id, PlatformOutput, ViewportId};
use std::time::Duration;

/// Merge the child's platform output into the parent's.
///
/// Most of a [`PlatformOutput`] is about the app as a whole rather than one viewport, so
/// it can be merged as-is: opening a url, copying text, or asking for another pass all
/// mean the same thing whichever viewport asked for them. The rest needs care, and is
/// handled here.
pub(crate) fn forward_platform_output(
    ctx: &Context,
    mut output: PlatformOutput,
    to_parent: Transform,
    pointer_is_over_child: bool,
) {
    // The IME rectangles are in child coordinates. Move them into the parent's space, or
    // the OS will put the candidate window in the wrong place.
    if let Some(ime) = &mut output.ime {
        ime.rect = to_parent.bounding_rect(ime.rect);
        ime.cursor_rect = to_parent.bounding_rect(ime.cursor_rect);
    }

    // Each pass produces a whole AccessKit tree, so `append` overwrites rather than
    // merges: forwarding the child's would throw the parent's away, and the app would lose
    // accessibility for everything outside the child. The child's nodes are grafted onto
    // the parent's tree by `forward_accesskit` instead, which is the only way they end up
    // in the right place anyway - this tree has them where the child drew them, which is
    // not where the parent painted the child.
    output.accesskit_update = None;

    // The parent counts its own passes.
    output.num_completed_passes = 0;

    ctx.output_mut(|parent| {
        // `append` takes the child's cursor unconditionally, but a child the pointer is
        // not even over has no business changing it.
        let parent_cursor = parent.cursor_icon;
        parent.append(output);
        if !pointer_is_over_child {
            parent.cursor_icon = parent_cursor;
        }
    });
}

/// Make the parent repaint whenever the child wants to.
///
/// A repaint request inside the child marks the _child_ viewport as needing a repaint, but
/// the child has no window of its own. Without this, a child animation would run for one
/// pass and then freeze until something else woke the app up.
pub(crate) fn forward_repaint(ctx: &Context, child_id: ViewportId, parent_id: ViewportId) {
    if ctx.has_requested_repaint_for(&child_id) {
        ctx.request_repaint_of(parent_id);
    }

    let delay = ctx.requested_repaint_delay_for(&child_id);
    if delay < Duration::MAX {
        ctx.request_repaint_after_for(delay, parent_id);
    }
}

/// Put the child's widgets into the parent's accessibility tree, under `id`.
///
/// Returns what was grafted, so a caller that skips a pass can graft it again: a child that
/// is not running is still on screen, and a screen reader that loses half a window every
/// time it stops changing is worse than no screen reader.
pub(crate) fn forward_accesskit(
    ctx: &Context,
    child_id: ViewportId,
    id: Id,
    to_parent: Transform,
    previous: Option<AccessKitSubtree>,
) -> Option<AccessKitSubtree> {
    let subtree = ctx.take_accesskit_subtree(child_id).or(previous)?;
    // A node's bounds are a rectangle, and a rotated or mirrored child does not put its
    // widgets in rectangles any more. Nothing to do but leave those out of the tree.
    let placement = to_parent.as_scale_translation()?;
    ctx.graft_accesskit_subtree(&subtree, id, placement);
    Some(subtree)
}

/// When the child next needs a pass of its own, on the parent's input clock.
///
/// `None` means it asked for nothing, and only then may a retained image be shown
/// indefinitely: a repaint request is how egui says "I am mid-animation", and an animation
/// drawn once and then frozen is worse than one that costs a pass.
pub(crate) fn repaint_deadline(ctx: &Context, child_id: ViewportId, now: f64) -> Option<f64> {
    if ctx.has_requested_repaint_for(&child_id) {
        return Some(now);
    }

    let delay = ctx.requested_repaint_delay_for(&child_id);
    (delay < Duration::MAX).then_some(now + delay.as_secs_f64())
}
