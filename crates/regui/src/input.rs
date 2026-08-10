use crate::Transform;
use egui::{Event, Modifiers, Pos2, RawInput, Rect, Response, Ui, Vec2, ViewportId, ViewportInfo};

/// Which of the parent's events the child is allowed to see this pass.
#[derive(Clone, Copy)]
pub(crate) struct Gate {
    /// Forward pointer and touch events.
    pub pointer: bool,

    /// Forward keyboard, text and IME events.
    pub keyboard: bool,
}

/// Everything about the child that decides what its input looks like.
pub(crate) struct ChildInput {
    pub viewport_id: ViewportId,

    /// The child's own screen size, in its own points.
    pub size: Vec2,

    /// The density the child rasterizes at.
    pub pixels_per_point: f32,

    /// Maps a position in the parent to the same place in the child.
    pub to_child: Transform,

    /// Which of the parent's events the child may see.
    pub gate: Gate,

    /// Tell the child the pointer has left, which it cannot work out on its own: it just
    /// stops hearing about a pointer that, as far as it knows, is still there.
    pub pointer_gone: bool,

    /// Restate the held modifiers, for a child with no input state to remember them.
    pub resync_modifiers: Option<Modifiers>,
}

/// Build the [`RawInput`] for the child viewport out of the parent's input.
pub(crate) fn child_input(ui: &Ui, child: &ChildInput) -> RawInput {
    let &ChildInput {
        viewport_id,
        size,
        pixels_per_point,
        to_child,
        gate,
        pointer_gone,
        resync_modifiers,
    } = child;

    let mut input = ui.input(|input| RawInput {
        viewport_id,
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
        // The resolved value, not `raw.max_texture_side`, which the integration only sets
        // on some passes and is `None` on the rest. Getting this wrong is expensive and
        // not obviously a texture problem: it feeds into the font atlas' `TextOptions`, so
        // a child that disagrees with its parent makes egui rebuild the atlas at the start
        // of every pass, twice per frame. The rebuild during the child's pass throws away
        // the atlas the parent's already-laid-out text points into, and the parent renders
        // gibberish.
        max_texture_side: Some(input.max_texture_side),
        time: input.raw.time,
        predicted_dt: input.raw.predicted_dt,
        focused: input.raw.focused,
        system_theme: input.raw.system_theme,
        events: {
            // A child that starts a pass with no state of its own - its first ever, or the
            // first after it sat some out - thinks nothing is held, because held modifiers
            // only reach a viewport through the event stream. Say so once, and only when
            // there is something to say: egui reads a non-empty event list as "something
            // happened, repaint", so an event restated every pass never lets the app idle.
            let mut events: Vec<Event> = resync_modifiers
                .filter(|modifiers| *modifiers != Modifiers::default())
                .map(Event::ModifiersChanged)
                .into_iter()
                .collect();
            events.extend(
                input
                    .raw
                    .events
                    .iter()
                    .filter_map(|event| remap_event(event, to_child, gate)),
            );
            events
        },
        ..Default::default()
    });

    if pointer_gone {
        input.events.push(Event::PointerGone);
    }

    // `run_hosted_viewport` would inherit the parent's scale, but we want our own, so
    // that text stays crisp when the child is magnified.
    let focused = input.focused;
    input.viewports.insert(
        viewport_id,
        ViewportInfo {
            native_pixels_per_point: Some(pixels_per_point),
            focused: Some(focused),
            ..Default::default()
        },
    );

    input
}

/// Strip an input down to what a pass may be run with twice.
///
/// A retained child that wakes up has to be laid out once before it can be interacted
/// with, and that first lay-out must not be able to *do* anything: the ui function runs
/// during it, and running a click or a keystroke twice is worse than losing it. Only the
/// pointer position and the modifier state survive, which is exactly what hit-testing
/// needs and nothing that can fire.
pub(crate) fn priming_input(mut input: RawInput) -> RawInput {
    input.events.retain(|event| {
        matches!(
            event,
            Event::PointerMoved(_) | Event::ModifiersChanged(_) | Event::WindowFocused(_)
        )
    });
    input
}

/// Translate one parent event into child space, or drop it if the child may not see it.
fn remap_event(event: &Event, to_child: Transform, gate: Gate) -> Option<Event> {
    let pointer = gate.pointer;
    let keyboard = gate.keyboard;

    match event {
        // Positional: the child's coordinate space is not the parent's.
        Event::PointerMoved(pos) => pointer.then(|| Event::PointerMoved(to_child.mul_pos(*pos))),
        Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers,
        } => pointer.then(|| Event::PointerButton {
            pos: to_child.mul_pos(*pos),
            button: *button,
            pressed: *pressed,
            modifiers: *modifiers,
        }),
        Event::Touch {
            device_id,
            id,
            phase,
            pos,
            force,
        } => pointer.then(|| Event::Touch {
            device_id: *device_id,
            id: *id,
            phase: *phase,
            pos: to_child.mul_pos(*pos),
            force: *force,
        }),

        // Directional: rotate and scale, but do not translate.
        Event::MouseMoved(delta) => pointer.then(|| Event::MouseMoved(to_child.mul_vec(*delta))),
        Event::MouseWheel {
            unit,
            delta,
            phase,
            modifiers,
        } => pointer.then(|| Event::MouseWheel {
            unit: *unit,
            delta: to_child.mul_vec(*delta),
            phase: *phase,
            modifiers: *modifiers,
        }),

        // Pointer events with nothing to remap.
        Event::PointerGone => pointer.then_some(Event::PointerGone),
        Event::Zoom(_) | Event::Rotate(_) => pointer.then(|| event.clone()),

        // Keyboard and clipboard.
        Event::Key { .. }
        | Event::Text(_)
        | Event::Copy
        | Event::Cut
        | Event::Paste(_)
        | Event::Ime(_) => keyboard.then(|| event.clone()),

        // Neither of these gates on anything: the child needs both to interpret its own
        // events, and dropping one would leave it thinking a key is still held. Modifiers
        // in particular are not keyboard input - they decide what a ctrl-click means.
        Event::WindowFocused(_) | Event::ModifiersChanged(_) => Some(event.clone()),

        // The child's widgets sit in the parent's accessibility tree - that is what
        // grafting the subtree is for - so whatever drives that tree addresses them
        // through the parent, and a request for a node the child owns arrives here.
        // Dropping it leaves every action on the child's widgets silently doing nothing:
        // a screen reader cannot press its buttons, and `scroll_to_me` on a row of a
        // scrolling menu leaves the row where it was. Gated on nothing, since an action
        // is neither pointer nor keyboard input, and one for a node the child does not
        // own is harmless - it matches no widget there and is ignored.
        Event::AccessKitActionRequest(_) => Some(event.clone()),

        // Screenshots address a specific viewport, so they are never ours to forward.
        _ => None,
    }
}

/// Should the child see pointer events this pass?
///
/// Yes while the pointer is over us, and yes while the child is being dragged even if the
/// pointer has since left - otherwise dragging a slider would stop the moment you left the
/// child's rect.
pub(crate) fn wants_pointer(response: &Response) -> bool {
    response.contains_pointer() || response.dragged() || response.is_pointer_button_down_on()
}
