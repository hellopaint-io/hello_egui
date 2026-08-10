//! Running ui outside the child that asked for it.
//!
//! A child has a screen of its own, and everything it draws is clipped to it. That is what
//! makes a child cheap to keep - its image is the whole of it - and it is also why a menu
//! dropped from a button near the child's edge is cut in half, and why a menu inside a
//! rotated child comes out rotated with it.
//!
//! Neither is what a menu is for. A menu belongs to the screen, not to the thing that
//! opened it: it should be legible whatever its owner is doing, and it should be free to
//! cover anything. So a child can hand a closure back out to the ui hosting it, along with
//! the rect it wants to hang off, mapped out of the child's coordinates and squared up.

use std::cell::RefCell;

use egui::{Rect, Response, Ui};

use crate::Transform;

/// A way out of the child currently running, into the ui hosting it.
///
/// Handed to the content function by [`Regui::show_with_root`](crate::Regui::show_with_root)
/// and its retained twin. Anything queued here runs once the child's pass is over, against
/// the host's [`Ui`] - so it is not clipped to the child, not scaled by it and not rotated
/// with it.
///
/// # Nesting
///
/// One level. A child inside a child escapes to the child hosting it, not all the way out.
/// [`Self::transform`] is the whole way out regardless, so geometry is right at any depth;
/// it is the layer that only rises by one. Nest reguis and put a menu in the inner one and
/// the outer one still clips it.
pub struct RootScope<'a> {
    /// The child's coordinates to the host's.
    to_root: Transform,

    /// Queued in the order it was asked for, and run in that order.
    deferred: RefCell<Vec<Box<dyn FnOnce(&mut Ui) + 'a>>>,
}

impl<'a> RootScope<'a> {
    pub(crate) fn new(to_root: Transform) -> Self {
        Self {
            to_root,
            deferred: RefCell::new(Vec::new()),
        }
    }

    /// Did anything ask to be run out here?
    pub(crate) fn is_empty(&self) -> bool {
        self.deferred.borrow().is_empty()
    }

    /// Run what was queued, against the host's ui.
    pub(crate) fn run(self, ui: &mut Ui) {
        for deferred in self.deferred.into_inner() {
            deferred(ui);
        }
    }

    /// How this child's coordinates map to the ui hosting it.
    pub fn transform(&self) -> Transform {
        self.to_root
    }

    /// Where a rect of the child's lands in the host.
    ///
    /// The bounding box, so the answer is a rect even when the child is rotated and the
    /// mapped corners are not square to the screen. That is the point: whatever hangs off
    /// this rect is meant to sit upright, so it needs somewhere upright to hang from.
    pub fn rect(&self, rect: Rect) -> Rect {
        self.to_root.bounding_rect(rect)
    }

    /// The same response, moved to where the host would say it is.
    ///
    /// Everything that positions itself against a response - a menu, a tooltip - reads
    /// these rects, so a response handed out here has to be talking about the host's
    /// coordinates. Only the geometry changes; what was clicked stays what was clicked.
    pub fn response(&self, response: &Response) -> Response {
        let mut moved = response.clone();
        moved.rect = self.rect(response.rect);
        moved.interact_rect = self.rect(response.interact_rect);
        moved
    }

    /// Queue `content` to run against the host's ui once this child's pass is over.
    pub fn ui(&self, content: impl FnOnce(&mut Ui) + 'a) {
        self.deferred.borrow_mut().push(Box::new(content));
    }

    /// Queue a popup hanging off `response`, run against the host's ui.
    ///
    /// `content` is handed the response back with its rects in the host's coordinates
    /// ([`Self::response`]), so building a menu on it puts the menu under the widget it
    /// belongs to rather than under wherever that widget's rect happens to point once it
    /// has left the child.
    ///
    /// ```no_run
    /// # let (ui, scope): (&mut egui::Ui, &regui::RootScope<'_>) = unimplemented!();
    /// let button = ui.button("Tools");
    /// scope.popup(&button, |ui, button| {
    ///     egui::Popup::menu(button).show(|ui| {
    ///         let _ = ui.button("Brush");
    ///     });
    /// });
    /// ```
    ///
    /// # Only while it is open
    ///
    /// Unlike [`Self::ui`], this queues nothing while the popup is shut. It has to: a child
    /// with anything out there cannot be retained, so a call site that queued its menu
    /// every pass - which is how [`egui::Popup::menu`] is normally written, since the popup
    /// decides for itself whether to draw - would keep its child awake forever for a menu
    /// nobody had opened.
    ///
    /// Open is read from egui's memory, so this fits any popup that tracks itself there:
    /// [`egui::Popup::menu`], [`egui::Popup::context_menu`], anything built on
    /// [`egui::Popup::from_toggle_button_response`]. A popup that keeps its own open flag
    /// has to gate itself and go through [`Self::ui`].
    pub fn popup(&self, response: &Response, content: impl FnOnce(&mut Ui, &Response) + 'a) {
        if !popup_wanted(response) {
            return;
        }
        let moved = self.response(response);
        self.ui(move |ui| {
            // The response came out of the child's viewport, and a popup reads its layer to
            // decide where it sits in the stack and whether it is a submenu of something.
            // Out here it belongs to whatever layer the host is drawing on.
            let mut moved = moved;
            moved.layer_id = ui.layer_id();
            content(ui, &moved);
        });
    }
}

/// Is there a popup on this response to draw, or about to be one?
///
/// The click is not redundant with the memory: the pass that opens a popup is the pass the
/// click arrives on, and the toggle that records it happens inside the popup - which has
/// not run yet.
fn popup_wanted(response: &Response) -> bool {
    response.clicked()
        || response.secondary_clicked()
        || egui::Popup::is_id_open(&response.ctx, egui::Popup::default_response_id(response))
}
