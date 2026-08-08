use crate::history::{DefaultHistory, History};
use crate::route_kind::RouteKind;
use crate::router_builder::{ErrorUi, RouterBuilder};
use crate::swipe_back::{swipe_back_gesture, SwipeBackConfig, SwipeBackEvent};
use crate::transition::{ActiveTransition, ActiveTransitionResult};
#[cfg(not(feature = "subsecond"))]
use crate::SubsecondMockRoute;
use crate::{
    CurrentTransition, Request, RouteState, RouterError, RouterResult, TransitionConfig, ID,
};
use egui::Ui;
use matchit::MatchError;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

/// A router instance
pub struct EguiRouter<State, History = DefaultHistory> {
    router: matchit::Router<RouteKind<State>>,
    history: Vec<RouteState<State>>,

    history_kind: History,

    forward_transition: TransitionConfig,
    backward_transition: TransitionConfig,
    replace_transition: TransitionConfig,

    current_transition: Option<CurrentTransition<State>>,
    default_duration: Option<f32>,

    error_ui: ErrorUi<State>,

    /// Enable iOS-style swipe-to-go-back gesture
    swipe_back_gesture_enabled: bool,
    /// Minimum distance from left edge to start the gesture (in pixels)
    swipe_back_edge_width: f32,
    /// Minimum swipe distance to trigger navigation (as fraction of screen width)
    swipe_back_threshold: f32,
}

impl<State: 'static, H: History + Default> EguiRouter<State, H> {
    /// Create a new [`RouterBuilder`]
    pub fn builder() -> RouterBuilder<State, H> {
        RouterBuilder::new()
    }

    pub(crate) fn from_builder(builder: RouterBuilder<State, H>, state: &mut State) -> Self {
        let mut router = Self {
            router: builder.router,
            history: Vec::new(),
            history_kind: builder.history_kind.unwrap_or_default(),
            current_transition: None,
            forward_transition: builder.forward_transition,
            backward_transition: builder.backward_transition,
            replace_transition: builder.replace_transition,
            default_duration: builder.default_duration,
            error_ui: builder.error_ui,
            swipe_back_gesture_enabled: builder.swipe_back_gesture_enabled,
            swipe_back_edge_width: builder.swipe_back_edge_width,
            swipe_back_threshold: builder.swipe_back_threshold,
        };

        if let Some((r, state_index)) = router
            .history_kind
            .active_route()
            .or(builder.default_route.map(|d| (d, None)))
        {
            router
                .navigate_impl(
                    state,
                    &r,
                    TransitionConfig::none(),
                    state_index.unwrap_or(0),
                )
                .unwrap();
        }

        router
    }

    /// Get the active route
    pub fn active_route(&self) -> Option<&str> {
        self.history.last().map(|r| r.path_with_query.as_str())
    }

    /// How many history entries are there?
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Iterate over the paths in the history
    pub fn history(&self) -> impl Iterator<Item = &str> {
        self.history.iter().map(|s| s.path_with_query.as_str())
    }

    fn parse_path(path: &str) -> (&str, BTreeMap<Cow<'_, str>, Cow<'_, str>>) {
        path.split_once('?')
            .map(|(path, q)| (path, form_urlencoded::parse(q.as_bytes()).collect()))
            .unwrap_or((path, BTreeMap::new()))
    }

    fn navigate_impl(
        &mut self,
        state: &mut State,
        path_with_query: &str,
        transition_config: TransitionConfig,
        new_state: u32,
    ) -> RouterResult {
        let (path, query) = Self::parse_path(path_with_query);

        let mut redirect = None;
        let result = self.router.at_mut(path);

        let result = match result {
            Ok(match_) => {
                match match_.value {
                    RouteKind::Route(handler) => {
                        let route = handler(Request {
                            state,
                            params: match_.params,
                            query,
                        });
                        self.history.push(RouteState {
                            path_with_query: path_with_query.to_string(),
                            route,
                            id: ID.fetch_add(1, Ordering::SeqCst),
                            state: new_state,
                        });

                        // Fire on_hiding on the previous top-of-stack (now second-to-last)
                        if self.history.len() >= 2 {
                            let idx = self.history.len() - 2;
                            if let Ok(route) = &mut self.history[idx].route {
                                route.on_hiding();
                            }
                        }

                        // Fire on_showing on the newly created route
                        if let Some(last) = self.history.last_mut() {
                            if let Ok(route) = &mut last.route {
                                route.on_showing();
                            }
                        }

                        self.current_transition = Some(CurrentTransition {
                            active_transition: ActiveTransition::forward(transition_config.clone())
                                .with_default_duration(self.default_duration),
                            leaving_route: None,
                        });
                    }
                    RouteKind::Redirect(r) => {
                        redirect = Some(r.clone());
                    }
                }
                Ok(())
            }
            Err(e) => match e {
                MatchError::NotFound => Err(RouterError::NotFound),
            },
        };

        if let Some(redirect) = redirect {
            self.history_kind.replace(&redirect, new_state)?;
            self.navigate_impl(state, &redirect, transition_config, new_state)?;
        }

        result
    }

    /// Navigate with a custom transition
    pub fn navigate_transition(
        &mut self,
        state: &mut State,
        path: impl Into<String>,
        transition_config: TransitionConfig,
    ) -> RouterResult {
        let path = path.into();
        let current_state = self.history.last().map_or(0, |r| r.state);
        let new_state = current_state + 1;
        self.history_kind.push(&path, new_state)?;
        self.navigate_impl(state, &path, transition_config, new_state)?;
        Ok(())
    }

    /// Navigate with the default transition
    pub fn navigate(&mut self, state: &mut State, route: impl Into<String>) -> RouterResult {
        self.navigate_transition(state, route, self.forward_transition.clone())
    }

    fn back_impl(&mut self, transition_config: TransitionConfig) {
        if self.history.len() > 1 {
            let mut leaving_route = self.history.pop();

            // Fire on_hiding on the leaving route
            if let Some(ref mut leaving) = leaving_route {
                if let Ok(route) = &mut leaving.route {
                    route.on_hiding();
                }
            }

            // Fire on_showing on the route that is now being revealed
            if let Some(last) = self.history.last_mut() {
                if let Ok(route) = &mut last.route {
                    route.on_showing();
                }
            }

            self.current_transition = Some(CurrentTransition {
                active_transition: ActiveTransition::backward(transition_config)
                    .with_default_duration(self.default_duration),
                leaving_route,
            });
        }
    }

    /// Go back with a custom transition
    pub fn back_transition(&mut self, transition_config: TransitionConfig) -> RouterResult {
        self.history_kind.back()?;
        self.back_impl(transition_config);
        Ok(())
    }

    /// Go back with the default transition
    pub fn back(&mut self) -> RouterResult {
        self.back_transition(self.backward_transition.clone())
    }

    /// Replace the current route with a custom transition
    pub fn replace_transition(
        &mut self,
        state: &mut State,
        path: impl Into<String>,
        transition_config: TransitionConfig,
    ) -> RouterResult {
        let mut redirect = None;

        let path_with_query = path.into();
        let (path, query) = Self::parse_path(&path_with_query);

        let result = self.router.at_mut(path);

        let current_state = self.history.last().map_or(0, |r| r.state);
        let new_state = current_state;

        let result = match result {
            Ok(match_) => match match_.value {
                RouteKind::Route(handler) => {
                    self.history_kind.replace(&path_with_query, new_state)?;
                    let mut leaving_route = self.history.pop();

                    // Fire on_hiding on the leaving route
                    if let Some(ref mut leaving) = leaving_route {
                        if let Ok(route) = &mut leaving.route {
                            route.on_hiding();
                        }
                    }

                    let route = handler(Request {
                        state,
                        params: match_.params,
                        query,
                    });
                    self.history.push(RouteState {
                        path_with_query: path_with_query.clone(),
                        route,
                        id: ID.fetch_add(1, Ordering::SeqCst),
                        state: new_state,
                    });

                    // Fire on_showing on the newly created route
                    if let Some(last) = self.history.last_mut() {
                        if let Ok(route) = &mut last.route {
                            route.on_showing();
                        }
                    }

                    self.current_transition = Some(CurrentTransition {
                        active_transition: ActiveTransition::forward(transition_config.clone())
                            .with_default_duration(self.default_duration),
                        leaving_route,
                    });

                    Ok(())
                }
                RouteKind::Redirect(r) => {
                    redirect = Some(r.clone());
                    Ok(())
                }
            },
            Err(MatchError::NotFound) => Err(RouterError::NotFound),
        };

        if let Some(redirect) = redirect {
            self.history_kind.replace(&redirect, new_state)?;
            self.replace_transition(state, redirect, transition_config)?;
        }

        result
    }

    /// Replace the current route with the default transition
    pub fn replace(&mut self, state: &mut State, path: impl Into<String>) -> RouterResult {
        self.replace_transition(state, path, self.replace_transition.clone())
    }

    /// Update the URL of the currently active route in place, preserving its
    /// state index, without re-running the route handler. Use this to mirror
    /// UI state (camera position, scroll offset, selected tab) into query
    /// params; calling `history.replaceState` directly will desync the
    /// router's history from the browser's and break back/forward navigation.
    pub fn replace_url(&mut self, path: impl Into<String>) -> RouterResult {
        let path_with_query = path.into();
        let current_state = self.history.last().map_or(0, |r| r.state);
        self.history_kind.replace(&path_with_query, current_state)?;
        if let Some(last) = self.history.last_mut() {
            last.path_with_query = path_with_query;
        }
        Ok(())
    }

    /// Render the router
    #[allow(clippy::too_many_lines)]
    pub fn ui(&mut self, ui: &mut Ui, state: &mut State) {
        // Handle iOS-style swipe-to-go-back gesture
        // The active route can override the router's default via enable_swipe()
        let swipe_enabled = self
            .history
            .last()
            .and_then(|r| r.route.as_ref().ok())
            .and_then(|route| route.enable_swipe())
            .unwrap_or(self.swipe_back_gesture_enabled);
        let swipe_event = swipe_back_gesture(
            ui,
            SwipeBackConfig {
                edge_width: self.swipe_back_edge_width,
                threshold: self.swipe_back_threshold,
                ..SwipeBackConfig::default()
            },
            swipe_enabled && self.history.len() > 1,
        );
        self.apply_swipe_event(swipe_event);

        for e in self.history_kind.update(ui.ctx()) {
            let state_index = e.state.unwrap_or(0);
            let path = e.location;

            // Prefer an exact path-and-state match. If the path differs (a
            // route mirrored UI state into query params via `replace_url`
            // after pushing) fall back to matching on the state index alone
            // so we still recognise this as a back/forward navigation.
            let matched = self
                .history
                .iter()
                .position(|r| r.path_with_query == path && r.state == state_index)
                .or_else(|| self.history.iter().position(|r| r.state == state_index));

            if let Some(idx) = matched {
                self.history[idx].path_with_query.clone_from(&path);
                let route_state = self.history[idx].state;
                let active_state = self.history.last().map_or(0, |r| r.state);

                // Retain all routes with a state less than or equal to the new state and the active state so that we can animate them out
                self.history
                    .retain(|r| r.state <= route_state || r.state == active_state);

                if route_state < active_state {
                    self.back_impl(self.backward_transition.clone());
                }
            } else {
                self.navigate_impl(state, &path, self.forward_transition.clone(), state_index)
                    .ok();
            }
        }

        let dt = ui.input(|i| i.stable_dt);
        let default_duration = ui.style().animation_time;

        if let Some((last, previous)) = self.history.split_last_mut() {
            let result = if let Some(transition) = &mut self.current_transition {
                // Advance before rendering, so the frame that reports `Done`
                // draws at the same progress it used to when `show` did both.
                let done = transition.active_transition.advance(dt, default_duration);
                let leaving_route_state = transition.leaving_route.as_mut().or(previous.last_mut());
                transition.active_transition.show(
                    ui,
                    state,
                    (last.id, |ui, state| match &mut last.route {
                        Ok(route) => {
                            route.ui_subsecond(ui, state);
                        }
                        Err(err) => {
                            (self.error_ui)(ui, state, err);
                        }
                    }),
                    leaving_route_state.map(|r| {
                        (r.id, |ui: &mut Ui, state: &mut _| match &mut r.route {
                            Ok(route) => {
                                route.ui_subsecond(ui, state);
                            }
                            Err(err) => {
                                (self.error_ui)(ui, state, err);
                            }
                        })
                    }),
                );
                Some(done)
            } else {
                ActiveTransition::show_default(ui, last.id, |ui| match &mut last.route {
                    Ok(route) => {
                        route.ui_subsecond(ui, state);
                    }
                    Err(err) => {
                        (self.error_ui)(ui, state, err);
                    }
                });
                None
            };

            match result {
                Some(ActiveTransitionResult::Done) => {
                    if let Some(mut transition) = self.current_transition.take() {
                        let is_backward = transition.active_transition.is_backward();
                        if is_backward {
                            // Leaving route is fully hidden
                            if let Some(ref mut leaving) = transition.leaving_route {
                                if let Ok(route) = &mut leaving.route {
                                    route.on_hide();
                                }
                            }
                            // Current top is fully shown again
                            if let Some(last) = self.history.last_mut() {
                                if let Ok(route) = &mut last.route {
                                    route.on_shown();
                                }
                            }
                        } else {
                            // Forward/replace completed
                            if let Some(ref mut leaving) = transition.leaving_route {
                                // Replace: leaving route is fully hidden
                                if let Ok(route) = &mut leaving.route {
                                    route.on_hide();
                                }
                            } else if self.history.len() >= 2 {
                                // Forward: previous top is now fully hidden
                                let idx = self.history.len() - 2;
                                if let Ok(route) = &mut self.history[idx].route {
                                    route.on_hide();
                                }
                            }
                            // The new top route is now fully shown
                            if let Some(last) = self.history.last_mut() {
                                if let Ok(route) = &mut last.route {
                                    route.on_shown();
                                }
                            }
                        }
                    }
                }
                Some(ActiveTransitionResult::Continue) | None => {}
            }
        }
    }

    /// Apply what the swipe-back gesture asked for this frame.
    fn apply_swipe_event(&mut self, event: Option<SwipeBackEvent>) {
        match event {
            Some(SwipeBackEvent::Begin) => {
                if self.current_transition.is_none() {
                    let mut transition = CurrentTransition {
                        active_transition: ActiveTransition::manual(
                            self.backward_transition.clone(),
                        )
                        .with_default_duration(self.default_duration),
                        leaving_route: None,
                    };
                    // Initialize progress to 1.0 (fully showing current page)
                    transition.active_transition.set_progress(1.0);
                    self.current_transition = Some(transition);
                }
            }
            Some(SwipeBackEvent::Progress(progress)) => {
                if let Some(transition) = &mut self.current_transition {
                    transition.active_transition.set_progress(progress);
                }
            }
            Some(SwipeBackEvent::Cancel) => {
                self.current_transition = None;
            }
            Some(SwipeBackEvent::Commit { progress }) => {
                let mut popped = self.history.pop();

                // Fire on_hiding on the popped route
                if let Some(ref mut leaving) = popped {
                    if let Ok(route) = &mut leaving.route {
                        route.on_hiding();
                    }
                }

                // Fire on_showing on the route being revealed
                if let Some(last) = self.history.last_mut() {
                    if let Ok(route) = &mut last.route {
                        route.on_showing();
                    }
                }

                // Complete the back navigation
                if let Some(transition) = &mut self.current_transition {
                    transition.active_transition =
                        ActiveTransition::backward(self.backward_transition.clone())
                            .with_default_duration(self.default_duration);
                    transition.active_transition.set_progress(1.0 - progress);
                    transition.leaving_route = popped;
                }
                // Actually perform the back navigation
                self.history_kind.back().ok();
            }
            None => {}
        }
    }
}
