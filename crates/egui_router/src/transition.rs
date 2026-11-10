use crate::TransitionConfig;
use egui::{AsIdSalt, Ui, UiBuilder, Vec2};

/// Trait for declaring a transition.
/// Prefer [`ComposableTransitionTrait`] unless you need to create a new ui to apply the transition.
pub trait TransitionTrait {
    /// Create a child ui with the transition applied
    fn create_child_ui(&self, ui: &mut Ui, t: f32, id_salt: impl AsIdSalt) -> Ui;
}

/// Trait for declaring a composable transition.
pub trait ComposableTransitionTrait {
    /// Apply the transition to the ui
    fn apply(&self, ui: &mut Ui, t: f32);
}

impl<T: ComposableTransitionTrait> TransitionTrait for T {
    fn create_child_ui(&self, ui: &mut Ui, t: f32, id_salt: impl AsIdSalt) -> Ui {
        let mut child = ui.new_child(UiBuilder::new().max_rect(ui.max_rect()).id_salt(id_salt));
        self.apply(&mut child, t);
        child
    }
}

/// Enum containing all possible transitions
#[derive(Debug, Clone)]
pub enum Transition {
    /// Simple fade transition
    Fade(FadeTransition),
    /// No transition
    NoTransition(NoTransition),
    /// Slide transition
    Slide(SlideTransition),
    /// Combined slide and fade transitions
    SlideFade(SlideFadeTransition),
}

impl TransitionTrait for Transition {
    fn create_child_ui(&self, ui: &mut Ui, t: f32, id_salt: impl AsIdSalt) -> Ui {
        match self {
            Transition::Fade(fade) => fade.create_child_ui(ui, t, id_salt),
            Transition::NoTransition(no_transition) => {
                no_transition.create_child_ui(ui, t, id_salt)
            }
            Transition::Slide(slide) => slide.create_child_ui(ui, t, id_salt),
            Transition::SlideFade(slide_fade) => slide_fade.create_child_ui(ui, t, id_salt),
        }
    }
}

/// Simple fade transition
#[derive(Debug, Clone)]
pub struct FadeTransition;

/// No transition
#[derive(Debug, Clone)]
pub struct NoTransition;

/// Slide transition
#[derive(Debug, Clone)]
pub struct SlideTransition {
    /// Amount and direction to slide. Default is [`Vec2::X`] (so it will slide in from the right)
    pub amount: Vec2,
}

/// Combining slide and fade transitions
#[derive(Debug, Clone)]
pub struct SlideFadeTransition(pub SlideTransition, pub FadeTransition);

impl Default for SlideTransition {
    fn default() -> Self {
        Self { amount: Vec2::X }
    }
}

impl SlideTransition {
    /// Create a new slide transition. Default is [`Vec2::X`] (so it will slide in from the right)
    pub fn new(amount: Vec2) -> Self {
        Self { amount }
    }
}

impl ComposableTransitionTrait for FadeTransition {
    fn apply(&self, ui: &mut Ui, t: f32) {
        ui.set_opacity(t);
    }
}

impl ComposableTransitionTrait for NoTransition {
    fn apply(&self, _: &mut Ui, _: f32) {}
}

impl TransitionTrait for SlideTransition {
    fn create_child_ui(&self, ui: &mut Ui, t: f32, id_salt: impl AsIdSalt) -> Ui {
        let available_size = ui.available_size();
        let offset = available_size * (1.0 - t) * self.amount;
        // Round to pixels to prevent sub-pixel jitter and flickering in child components
        let offset = Vec2::new(offset.x.round(), offset.y.round());
        let child_rect = ui.max_rect().translate(offset);

        ui.new_child(UiBuilder::new().max_rect(child_rect).id_salt(id_salt))
    }
}

impl TransitionTrait for SlideFadeTransition {
    fn create_child_ui(&self, ui: &mut Ui, t: f32, id_salt: impl AsIdSalt) -> Ui {
        let mut child = self.0.create_child_ui(ui, t, id_salt);
        self.1.apply(&mut child, t);
        child
    }
}

impl From<FadeTransition> for Transition {
    fn from(fade: FadeTransition) -> Self {
        Transition::Fade(fade)
    }
}

impl From<NoTransition> for Transition {
    fn from(no_transition: NoTransition) -> Self {
        Transition::NoTransition(no_transition)
    }
}

impl From<SlideTransition> for Transition {
    fn from(slide: SlideTransition) -> Self {
        Transition::Slide(slide)
    }
}

impl From<SlideFadeTransition> for Transition {
    fn from(slide_fade: SlideFadeTransition) -> Self {
        Transition::SlideFade(slide_fade)
    }
}

/// Configuration for a transition, containing the in and out transitions
/// The in transition is the transition that will be applied to the page that is being navigated to
/// The out transition is the transition that will be applied to the page that is being navigated from
pub enum TransitionType {
    /// Forward transition
    Forward {
        /// Will be applied to the page that is being navigated to
        in_: Transition,
        /// Will be applied to the page that is being navigated from
        out: Transition,
    },
    /// Backward transition (will play the out transition in reverse)
    Backward {
        /// Will be applied to the page that is being navigated to
        in_: Transition,
        /// Will be applied to the page that is being navigated from
        out: Transition,
    },
}

/// A transition that is currently running.
///
/// Drive it with [`ActiveTransition::advance`] once per frame and render the
/// two pages with [`ActiveTransition::show`]. The two are separate so the
/// render pass can take `&self` — useful if the transition lives in state that
/// is only borrowed immutably while rendering.
pub struct ActiveTransition {
    duration: Option<f32>,
    progress: f32,
    easing: fn(f32) -> f32,
    in_: Transition,
    out: Transition,
    backward: bool,
    /// If true, the transition is controlled manually (e.g., by a gesture)
    manual_control: bool,
}

/// Whether an [`ActiveTransition`] has run to completion.
pub enum ActiveTransitionResult {
    /// The transition finished this frame and can be dropped.
    Done,
    /// The transition is still running.
    Continue,
}

impl ActiveTransition {
    /// A transition for navigating forward (pushing a page).
    pub fn forward(config: TransitionConfig) -> Self {
        Self {
            duration: config.duration,
            easing: config.easing,
            progress: 0.0,
            in_: config.in_,
            out: config.out,
            backward: false,
            manual_control: false,
        }
    }

    /// A transition for navigating backward (popping a page). Plays the
    /// forward transition in reverse.
    pub fn backward(config: TransitionConfig) -> Self {
        Self {
            duration: config.duration,
            easing: config.easing,
            progress: 0.0,
            in_: config.in_,
            out: config.out,
            backward: true,
            manual_control: false,
        }
    }

    /// A transition whose progress is driven by the caller (e.g. a
    /// swipe gesture) rather than by elapsed time. [`ActiveTransition::advance`]
    /// is a no-op on it, and it never reports [`ActiveTransitionResult::Done`].
    pub fn manual(config: TransitionConfig) -> Self {
        Self {
            duration: config.duration,
            easing: config.easing,
            progress: 0.0,
            in_: config.in_,
            out: config.out,
            backward: false,
            manual_control: true,
        }
    }

    /// How far the transition has run, from 0 to 1.
    pub fn progress(&self) -> f32 {
        self.progress
    }

    /// Whether this is a backward (pop) transition.
    pub fn is_backward(&self) -> bool {
        self.backward
    }

    /// The progress [`ActiveTransition::show`] gives the page being navigated
    /// to this frame, from 0 to 1 in both directions: eased, or as set while a
    /// [`manual`](ActiveTransition::manual) transition is driven by hand.
    ///
    /// Use it to animate something in step with the pages, like an element
    /// that flies from one page to the other.
    pub fn eased_progress(&self) -> f32 {
        let t = self.progress.min(1.0);
        if self.manual_control {
            t
        } else {
            (self.easing)(t)
        }
    }

    /// Set the progress directly. Used to drive a
    /// [`manual`](ActiveTransition::manual) transition from a gesture.
    pub fn set_progress(&mut self, progress: f32) {
        self.progress = progress.clamp(0.0, 1.0);
    }

    /// Fall back to `duration` when the [`TransitionConfig`] didn't set one.
    pub fn with_default_duration(mut self, duration: Option<f32>) -> Self {
        if self.duration.is_none() {
            self.duration = duration;
        }
        self
    }

    /// Advance the transition clock by `dt` seconds, returning whether it
    /// finished. `default_duration` applies when neither the
    /// [`TransitionConfig`] nor [`ActiveTransition::with_default_duration`] set
    /// one — pass `ui.style().animation_time`.
    ///
    /// Call this once per frame, before [`ActiveTransition::show`].
    pub fn advance(&mut self, dt: f32, default_duration: f32) -> ActiveTransitionResult {
        if self.manual_control {
            return ActiveTransitionResult::Continue;
        }
        self.progress += dt / self.duration.unwrap_or(default_duration);
        if self.progress >= 1.0 {
            ActiveTransitionResult::Done
        } else {
            ActiveTransitionResult::Continue
        }
    }

    /// Render the incoming page (`content_in`) and, if there is one, the
    /// outgoing page (`content_out`) at the current progress.
    ///
    /// The `usize` in each pair is a unique, stable id for that page — it
    /// isolates the page in egui's auto-id space so two pages rendered in the
    /// same frame don't collide.
    pub fn show<State>(
        &self,
        ui: &mut Ui,
        state: &mut State,
        (in_id, content_in): (usize, impl FnOnce(&mut Ui, &mut State)),
        content_out: Option<(usize, impl FnOnce(&mut Ui, &mut State))>,
    ) {
        let t = self.progress.min(1.0);
        ui.ctx().request_repaint();

        // Apply easing only when not manually controlled (for gestures, use linear)
        let eased_t = if self.manual_control {
            t
        } else {
            (self.easing)(t)
        };

        let eased_t_rev = if self.manual_control {
            1.0 - t
        } else {
            (self.easing)(1.0 - t)
        };

        if self.backward {
            with_temp_auto_id(ui, in_id, |ui| {
                let mut out_ui = self
                    .out
                    .create_child_ui(ui, eased_t, ("router_child", in_id));
                content_in(&mut out_ui, state);
            });

            if let Some((out_id, content_out)) = content_out {
                with_temp_auto_id(ui, out_id, |ui| {
                    let mut in_ui =
                        self.in_
                            .create_child_ui(ui, eased_t_rev, ("router_child", out_id));
                    content_out(&mut in_ui, state);
                });
            }
        } else {
            if let Some((out_id, content_out)) = content_out {
                with_temp_auto_id(ui, out_id, |ui| {
                    let mut out_ui =
                        self.out
                            .create_child_ui(ui, eased_t_rev, ("router_child", out_id));
                    content_out(&mut out_ui, state);
                });
            }

            with_temp_auto_id(ui, in_id, |ui| {
                let mut in_ui = self
                    .in_
                    .create_child_ui(ui, eased_t, ("router_child", in_id));
                content_in(&mut in_ui, state);
            });
        }
    }

    /// Render a single page with no transition running. `with_id` isolates it
    /// in egui's auto-id space exactly as [`ActiveTransition::show`] does, so
    /// widget ids stay stable across the start and end of a transition.
    pub fn show_default(ui: &mut Ui, with_id: usize, content: impl FnOnce(&mut Ui)) {
        with_temp_auto_id(ui, with_id, |ui| {
            let mut ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(ui.max_rect())
                    .id_salt(("router_child", with_id)),
            );
            content(&mut ui);
        });
    }
}

fn with_temp_auto_id(ui: &mut Ui, id: usize, content: impl FnOnce(&mut Ui)) {
    ui.skip_ahead_auto_ids(id);
    content(ui);
    ui.skip_ahead_auto_ids(usize::MAX - (id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eased_progress_matches_what_show_gives_the_incoming_page() {
        let config = TransitionConfig::default();

        for mut transition in [
            ActiveTransition::forward(config.clone()),
            ActiveTransition::backward(config.clone()),
        ] {
            transition.set_progress(0.25);
            let eased = transition.eased_progress();
            assert!((eased - egui::emath::ease_in_ease_out(0.25)).abs() < 1e-6);
            assert!(eased < 0.25, "ease-in-out starts slow");
        }

        // A gesture drives the pages linearly, so the progress is passed on as is.
        let mut manual = ActiveTransition::manual(config);
        manual.set_progress(0.25);
        assert!((manual.eased_progress() - 0.25).abs() < 1e-6);
    }
}
