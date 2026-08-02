use egui::{scroll_area, Id, NumExt, Sense, Ui};

/// The state of the iOS-style swipe-to-go-back gesture.
///
/// Lives in egui's temporary memory, keyed on [`SwipeBackConfig::id`], so the
/// caller doesn't have to store it.
#[derive(Debug, Clone)]
enum SwipeBackGestureState {
    /// No gesture is happening
    Idle,
    /// User is actively swiping
    Swiping {
        /// Distance swiped in pixels
        distance: f32,
    },
    /// Gesture was cancelled due to vertical movement, wait for release
    Cancelled,
}

/// Velocity that commits the navigation regardless of distance, in points per
/// second.
const FLICK_VELOCITY_THRESHOLD: f32 = 100.0;

/// Minimum swipe distance before the gesture steals the drag from a scroll
/// area, in points.
const DRAG_STEAL_DISTANCE: f32 = 10.0;

/// Tuning for [`swipe_back_gesture`].
#[derive(Debug, Clone, Copy)]
pub struct SwipeBackConfig {
    /// Minimum distance from the left edge to start the gesture, in points.
    pub edge_width: f32,
    /// Minimum swipe distance to trigger navigation, as a fraction of the
    /// screen width.
    pub threshold: f32,
    /// Id the gesture state and its interaction are keyed on.
    pub id: Id,
}

impl Default for SwipeBackConfig {
    fn default() -> Self {
        Self {
            edge_width: 40.0,
            threshold: 0.4,
            id: Id::new("router_swipe_back_gesture"),
        }
    }
}

/// What the gesture is asking the navigation stack to do this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SwipeBackEvent {
    /// The gesture started. Begin a manual backward transition at progress 1.0
    /// (the current page fully shown).
    Begin,
    /// Set the manual transition's progress. 1.0 is the current page fully
    /// shown, 0.0 fully swiped away.
    Progress(f32),
    /// The gesture was abandoned. Drop the manual transition.
    Cancel,
    /// The gesture was released past the threshold, or flicked. Pop the stack
    /// and run a backward transition starting at `1.0 - progress`.
    Commit {
        /// How far the swipe got, as a fraction of the screen width.
        progress: f32,
    },
}

/// Run one frame of the iOS-style swipe-to-go-back gesture.
///
/// The caller owns the navigation stack and the transition; this only reads
/// pointer input and reports what the gesture wants. Pass `active: false` when
/// there is nothing to go back to — the gesture then does nothing and leaves
/// its stored state untouched.
///
/// # Example
/// ```no_run
/// # use egui_router::swipe_back::{swipe_back_gesture, SwipeBackConfig, SwipeBackEvent};
/// # fn f(ui: &mut egui::Ui, can_go_back: bool) {
/// match swipe_back_gesture(ui, SwipeBackConfig::default(), can_go_back) {
///     Some(SwipeBackEvent::Begin) => { /* start a manual transition */ }
///     Some(SwipeBackEvent::Progress(t)) => { /* set_progress(t) */ }
///     Some(SwipeBackEvent::Cancel) => { /* drop the transition */ }
///     Some(SwipeBackEvent::Commit { progress }) => { /* pop, animate the rest */ }
///     None => {}
/// }
/// # }
/// ```
#[allow(clippy::too_many_lines)]
pub fn swipe_back_gesture(
    ui: &mut Ui,
    config: SwipeBackConfig,
    active: bool,
) -> Option<SwipeBackEvent> {
    if !active {
        return None;
    }

    let gesture_id = config.id;

    // Get or create gesture state
    let last_state = ui.data_mut(|data| {
        data.get_temp_mut_or(gesture_id, SwipeBackGestureState::Idle)
            .clone()
    });

    let mut gesture_state = last_state;
    let mut event = None;

    // Get the content rect for interaction
    let content_rect = ui.available_rect_before_wrap();
    let sense = ui.interact(content_rect, gesture_id, Sense::hover());

    // Check if there's something blocking the drag (e.g., scroll area)
    let is_something_blocking_drag = ui.ctx().dragged_id().is_some_and(|id| {
        // Ignore if the dragged id is a scroll area
        scroll_area::State::load(ui.ctx(), id).is_some()
    }) && !ui.ctx().is_being_dragged(gesture_id);

    if sense.contains_pointer() && !is_something_blocking_drag {
        let (pointer_pos, delta, any_released, velocity) = ui.input(|input| {
            (
                input.pointer.interact_pos(),
                if input.pointer.is_decidedly_dragging() {
                    Some(input.pointer.delta())
                } else {
                    None
                },
                input.pointer.any_released(),
                input.pointer.velocity(),
            )
        });

        if let Some(delta) = delta {
            match gesture_state {
                SwipeBackGestureState::Idle => {
                    // Check if the gesture started from the left edge
                    if let Some(pos) = pointer_pos {
                        if pos.x <= content_rect.min.x + config.edge_width {
                            // Cancel if velocity is more vertical than horizontal
                            if velocity.y.abs() > velocity.x.abs() && velocity.y.abs() > 0.0 {
                                // Vertical movement dominates, don't start the gesture
                                gesture_state = SwipeBackGestureState::Cancelled;
                            } else {
                                // Start the gesture
                                gesture_state = SwipeBackGestureState::Swiping { distance: 0.0 };
                                event = Some(SwipeBackEvent::Begin);
                            }
                        }
                    }
                }
                SwipeBackGestureState::Swiping { distance, .. } => {
                    // Cancel if velocity becomes too vertical before we've committed
                    if distance < DRAG_STEAL_DISTANCE
                        && velocity.y.abs() > velocity.x.abs()
                        && velocity.y.abs() > 0.0
                    {
                        // Vertical movement dominates, cancel the gesture
                        gesture_state = SwipeBackGestureState::Cancelled;
                        event = Some(SwipeBackEvent::Cancel);
                    } else {
                        // Update the gesture distance (only positive horizontal movement)
                        let new_distance = (distance + delta.x).max(0.0);

                        gesture_state = SwipeBackGestureState::Swiping {
                            distance: new_distance,
                        };

                        if new_distance > DRAG_STEAL_DISTANCE {
                            // Steal the drag in case a scroll area is also detecting it
                            ui.ctx().set_dragged_id(gesture_id);
                        }

                        let screen_width = content_rect.width();
                        let progress = 1.0 - (new_distance / screen_width).at_most(1.0);
                        event = Some(SwipeBackEvent::Progress(progress));
                    }
                }
                SwipeBackGestureState::Cancelled => {
                    // Wait for release before allowing new gestures
                }
            }
        }

        if any_released {
            if let SwipeBackGestureState::Swiping { distance } = gesture_state {
                let screen_width = content_rect.width();
                let progress = distance / screen_width;

                // Check if we've swiped far enough OR flicked fast enough to trigger back navigation
                let should_navigate_back =
                    progress >= config.threshold || velocity.x >= FLICK_VELOCITY_THRESHOLD;

                event = Some(if should_navigate_back {
                    SwipeBackEvent::Commit { progress }
                } else {
                    // Cancel the gesture - animate back to the current page
                    SwipeBackEvent::Cancel
                });

                gesture_state = SwipeBackGestureState::Idle;
            } else {
                gesture_state = SwipeBackGestureState::Idle;
            }
        }
    } else {
        // Pointer left the area, cancel the gesture
        if matches!(gesture_state, SwipeBackGestureState::Swiping { .. }) {
            event = Some(SwipeBackEvent::Cancel);
        }
        gesture_state = SwipeBackGestureState::Idle;
    }

    // Save the gesture state
    ui.data_mut(|data| {
        data.insert_temp(gesture_id, gesture_state);
    });

    event
}
