use crate::{
    RootScope, Transform, backend,
    input::{self, Gate},
    output,
};
use egui::{
    AsId, Event, FullOutput, Id, Pos2, Rect, Response, Sense, Ui, Vec2, ViewportId, emath::Rot2,
};

/// Run a part of your ui in its own egui viewport, and paint the result into this ui.
///
/// The child ui shares this [`egui::Context`], so it shares memory, style and fonts, but
/// it gets its own input, its own hit-testing and its own focus. Because it is painted
/// rather than laid out, you can move, scale and rotate it.
///
/// ```
/// # egui::__run_test_ui(|ui| {
/// use regui::Regui;
///
/// let output = Regui::new("preview")
///     .size(egui::vec2(200.0, 100.0))
///     .scale(0.5)
///     .rotation(0.1)
///     .show(ui, |ui| ui.button("click me").clicked());
///
/// if output.inner {
///     println!("the button inside the child ui was clicked");
/// }
/// # });
/// ```
///
/// # Caveats
///
/// All of the child's shapes go into a single layer of the parent, so the child's popups
/// and tooltips cannot leave the child's rect.
#[must_use = "You should call .show()"]
pub struct Regui {
    id_salt: Id,
    size: Vec2,
    scale: f32,
    rotation: f32,
    mirror_x: bool,
    offset: Vec2,
    crisp: bool,
    interactive: bool,

    /// Place the child here instead of laying it out. See [`Regui::transform`].
    placement: Option<Transform>,

    /// What the parent's rect senses. `None` follows `interactive`.
    sense: Option<Sense>,

    /// Whether the child sees the pointer, when the caller would rather decide. See
    /// [`Regui::pointer`].
    pointer: Option<bool>,

    /// Blur the child's own content, in points. Zero for none. Needs the `wgpu` feature.
    blur: f32,

    /// Render through a texture even without an effect asking for it.
    offscreen: bool,

    /// Reuse the last image while this key is unchanged. See [`Regui::retain`].
    retain: Option<u64>,

    /// Shrink the child to what it lays out, treating `size` as a maximum. See
    /// [`Regui::auto_size`].
    auto_size: bool,
}

/// What [`Regui::show`] gives you back.
pub struct ReguiOutput<R> {
    /// The parent's response for the area the child was painted into.
    ///
    /// Its rect is the child's rect after the transform, so it grows when you rotate the
    /// child.
    pub response: Response,

    /// Whatever your ui function returned.
    pub inner: R,

    /// Maps the child's coordinates to the parent's.
    pub transform: Transform,

    /// The viewport the child ran in.
    ///
    /// Useful with [`egui::Context::input_for`] and friends, to ask questions about the
    /// child rather than the parent.
    pub viewport_id: ViewportId,
}

/// What we need to remember between passes.
#[derive(Clone, Copy, Default)]
struct State {
    /// Did anything inside the child have keyboard focus at the end of the last pass?
    ///
    /// Focus is decided during a pass, so we can only know one pass late. That is a pass
    /// of latency on "may the child see key presses", which is invisible in practice: you
    /// have to click or tab into a widget before you can type into it.
    child_has_focus: bool,

    /// Was the child receiving pointer events last pass?
    ///
    /// Used to tell the child the pointer has left, which it cannot work out on its own:
    /// it just stops hearing about a pointer that, as far as it knows, is still there.
    had_pointer: bool,

    /// The key the image sitting in the child's texture was rendered for.
    ///
    /// `None` while there is nothing worth reusing: the child has never rendered
    /// off-screen, or something invalidated what it left behind.
    cached_key: Option<u64>,

    /// Is something inside the child mid-interaction - an open menu, a drag, a caret?
    ///
    /// Such a child has to keep running: skipping a pass drops its viewport, and with it
    /// the menu that was open and the drag that was in progress.
    child_busy: bool,

    /// When the child last asked to be repainted, on the parent's input clock.
    ///
    /// `None` when it asked for nothing, which is the only case where a still image can
    /// be shown indefinitely.
    repaint_at: Option<f64>,

    /// Did the previous pass actually run the child?
    ///
    /// Interaction is decided from the widget rectangles of the previous pass, and a
    /// skipped pass leaves none, so the pass that wakes a child has to prime them first.
    ran_last_pass: bool,

    /// What the child measured at, for an auto-sized one. See [`Regui::auto_size`].
    ///
    /// `None` until it has run once, which is the only time an auto-sized child is laid out
    /// at the maximum it was given.
    measured_size: Option<Vec2>,

    /// Did the child hand anything out to the ui hosting it last pass?
    ///
    /// Such a child cannot be retained: what it queued is drawn by the host, from a pass
    /// that would not happen, so its menu would come down the moment it sat one out.
    /// [`State::child_busy`] does not cover this - the menu is not in the child's
    /// viewport any more, so the child reports no popup open.
    had_root_ui: bool,
}

/// Where a running child's coordinates land on the screen, for a child inside a child.
fn to_root_id(viewport: ViewportId) -> Id {
    Id::new("regui_to_root").with(viewport)
}

/// The popups this child handed to its host that the host still has open.
fn open_popups_id(viewport: ViewportId) -> Id {
    Id::new("regui_open_popups").with(viewport)
}

/// What the child hosting this one has already done to it, if anything.
fn ancestor_transform(ctx: &egui::Context, parent_id: ViewportId) -> Transform {
    ctx.data(|data| data.get_temp(to_root_id(parent_id)))
        .unwrap_or(Transform::IDENTITY)
}

/// The child's accessibility nodes, kept apart from [`State`] because they are neither
/// `Copy` nor small.
///
/// A retained child is on screen without running, so its nodes have to be grafted onto
/// every one of the parent's trees, not just the ones where it laid itself out.
#[derive(Clone, Default)]
struct Accessibility(Option<egui::AccessKitSubtree>);

impl Regui {
    /// Start building a child ui.
    ///
    /// `id_salt` only has to be unique within the parent ui, like the salt of any other
    /// egui widget.
    pub fn new(id_salt: impl AsId) -> Self {
        Self {
            id_salt: Id::new(id_salt),
            size: Vec2::splat(200.0),
            scale: 1.0,
            rotation: 0.0,
            mirror_x: false,
            offset: Vec2::ZERO,
            crisp: false,
            interactive: true,
            placement: None,
            sense: None,
            pointer: None,
            blur: 0.0,
            offscreen: false,
            retain: None,
            auto_size: false,
        }
    }

    /// How big the child ui thinks its screen is, in the child's own points.
    ///
    /// This is the child's `screen_rect`; the space taken up in the parent is this size
    /// after the transform. Defaults to 200x200. With [`Self::auto_size`] it is a maximum
    /// rather than the size itself.
    #[inline]
    pub fn size(mut self, size: Vec2) -> Self {
        self.size = size;
        self
    }

    /// Scale the child, around its own top left corner.
    #[inline]
    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    /// Rotate the child clockwise, in radians.
    ///
    /// The parent will reserve the space the rotated child needs, which is more than the
    /// child's own size.
    #[inline]
    pub fn rotation(mut self, radians: f32) -> Self {
        self.rotation = radians;
        self
    }

    /// Reflect the child across its own vertical axis.
    ///
    /// See [`Transform::mirror_x`] — in particular, this is not the same as a negative
    /// [`Self::scale`], and text inside a mirrored child reads backwards.
    #[inline]
    pub fn mirror_x(mut self, mirror_x: bool) -> Self {
        self.mirror_x = mirror_x;
        self
    }

    /// Shift the child away from where it would otherwise be painted.
    ///
    /// This does not change how much space the child takes up in the parent, so it is a
    /// good way to nudge or animate a child without disturbing the layout around it.
    #[inline]
    pub fn offset(mut self, offset: Vec2) -> Self {
        self.offset = offset;
        self
    }

    /// Place the child with a transform of your own, instead of laying it out.
    ///
    /// Normally `Regui` reserves the space the transformed child needs at the ui's cursor
    /// and derives the transform from where that landed. Pass one here when the placement
    /// is already decided by something outside the layout — a camera, a scene graph, an
    /// animation — and the child has to line up with it exactly.
    ///
    /// This reserves no space: the child is an overlay on whatever is already there, and
    /// the parent's rect only exists to catch input. [`Self::scale`], [`Self::rotation`]
    /// and [`Self::mirror_x`] are ignored, while [`Self::offset`] still nudges the result.
    #[inline]
    pub fn transform(mut self, transform: Transform) -> Self {
        self.placement = Some(transform);
        self
    }

    /// Rasterize the child's text for the scale it is drawn at, instead of scaling the
    /// glyphs as geometry.
    ///
    /// Turn this on when you magnify a child and its text looks soft. It costs memory:
    /// every text size in the child gets its own entry in the font atlas.
    ///
    /// The `wgpu` backend does not need this.
    #[inline]
    pub fn crisp(mut self, crisp: bool) -> Self {
        self.crisp = crisp;
        self
    }

    /// Blur the child's own content, with the given radius in points.
    ///
    /// Unlike [`crate::BackdropBlur`], which blurs what is _behind_ a rect, this blurs the
    /// child ui itself: use it to push a panel out of focus, or to fade one in and out.
    /// The child stays interactive while blurred, which is usually not what you want, so
    /// pair it with [`Self::interactive`].
    ///
    /// Needs the `wgpu` feature and [`crate::install_wgpu`]; it turns
    /// [`Self::offscreen`] on, since a shader needs an image to work on.
    #[cfg(feature = "wgpu")]
    #[inline]
    pub fn blur(mut self, radius: f32) -> Self {
        self.blur = radius;
        self
    }

    /// Render the child into a texture, rather than handing its triangles to the parent.
    ///
    /// Turn this on for exact clipping when the child is rotated, and for text that stays
    /// crisp at any scale without [`Self::crisp`]'s cost to the font atlas. It is on
    /// automatically when an effect needs it.
    ///
    /// Needs the `wgpu` feature and [`crate::install_wgpu`]. Without them this falls back
    /// to handing the parent triangles, and says so in the log.
    #[cfg(feature = "wgpu")]
    #[inline]
    pub fn offscreen(mut self, offscreen: bool) -> Self {
        self.offscreen = offscreen;
        self
    }

    /// May the user interact with the child?
    ///
    /// On by default. Turn it off for a child that should only be looked at: it then sees
    /// no input at all, and clicks fall through to whatever is behind it. Useful for
    /// previews, thumbnails and backdrops.
    #[inline]
    pub fn interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    /// Decide for yourself whether the child sees the pointer this pass.
    ///
    /// `Regui` normally works this out from the parent's [`Response`]: the pointer is over
    /// its rect, or it is being dragged. Both halves of that can be wrong for a child that
    /// is not the shape of its rect. A child given [`Self::sense`] of [`Sense::hover`]
    /// cannot be dragged at all, so a drag that starts inside it and leaves - which is
    /// every slider drag worth the name - looks like the pointer simply left. And a child
    /// that is mostly transparent, laid over something the user is still working on, is
    /// "under the pointer" almost always while being under it almost never.
    ///
    /// Since a child being fed the pointer is never retained, getting this right is also
    /// what decides whether [`Self::retain`] saves anything.
    #[inline]
    pub fn pointer(mut self, over: bool) -> Self {
        self.pointer = Some(over);
        self
    }

    /// What the parent's rect senses, if not the [`Self::interactive`] default of
    /// [`Sense::click_and_drag`].
    ///
    /// The child still sees pointer events either way — this is only about what the
    /// *parent* thinks happened over the child's rect. Set it to [`Sense::hover`] for a
    /// child that covers something else the user needs to keep dragging, such as a canvas:
    /// a click-and-drag rect on top would make the child the drag target everywhere, and
    /// the canvas underneath would never see a gesture again.
    ///
    /// The child cannot then be the parent's drag target either, so whatever sits
    /// underneath has to be told when the child wants the pointer. Ask the child (inside
    /// the ui function, where `wants_pointer_input` reports the child's own viewport) and
    /// hand the answer on.
    #[inline]
    pub fn sense(mut self, sense: Sense) -> Self {
        self.sense = Some(sense);
        self
    }

    /// Shrink the child's screen to what its contents lay out, up to [`Self::size`].
    ///
    /// Like an [`egui::Area`], which is as big as what you put in it and no bigger. Without
    /// this a child is exactly the size you name, and a child that is bigger than its
    /// contents costs the difference for nothing: its screen is the texture it renders
    /// into, so a bar of chrome given a whole panel's worth of room pays for the whole
    /// panel every pass it runs.
    ///
    /// The child keeps its origin and shrinks towards it, again like an `Area`: contents
    /// laid out from the top left end up in the same place, contents laid out against the
    /// *far* edge of the screen they were given do not, since that edge has moved. Give
    /// those a [`Self::size`] they are meant to fill, or place them against something other
    /// than their own screen.
    ///
    /// Whatever the child draws in areas of its own — its menus, its tooltips — is measured
    /// too, so opening one grows the child rather than being clipped by it.
    ///
    /// # Cost
    ///
    /// An extra layout pass, every pass the child runs. Measuring means laying the contents
    /// out at the maximum size first, with egui's sizing-pass rules so that widgets which
    /// would otherwise fill the space report what they actually need; that pass is thrown
    /// away and the real one runs at the answer. The content function is therefore called
    /// twice per pass, so it has to be safe to call for its layout alone — the measuring
    /// pass gets no events, and anything it hands out through [`RootScope`] is dropped.
    ///
    /// Pair it with [`Self::retain`], which is what stops that cost being paid on a pass
    /// where nothing changed.
    #[inline]
    pub fn auto_size(mut self, auto_size: bool) -> Self {
        self.auto_size = auto_size;
        self
    }

    /// Keep the last image and stop running the child while `content_key` stays the same.
    ///
    /// This is what makes `regui` *re*tained: a child that has not changed, is not under
    /// the pointer and is not mid-interaction costs one textured quad per frame - no
    /// layout, no tessellation, no render pass. Use it for chrome that is expensive to lay
    /// out and rarely changes, and pair it with [`Self::show_retained`], which tells you
    /// whether the child ran.
    ///
    /// `content_key` has to change whenever anything the child *displays* changes; hash
    /// exactly that state and nothing else. `regui` folds in what it knows about on its
    /// own - size, scale, style and theme - so you do not have to.
    ///
    /// The child is run anyway, key or no key, whenever it could not sit still: while the
    /// pointer is over it, while anything inside it holds keyboard focus, while a menu is
    /// open or a widget is being dragged inside it, and until every repaint it asked for
    /// (an animation, a blinking caret) has been served. So an animating child is correct,
    /// it just is not saving you anything until it settles.
    ///
    /// Needs the `wgpu` feature and [`crate::install_wgpu`]: there has to be a texture to
    /// keep. This turns [`Self::offscreen`] on. It does nothing on a child that is also
    /// blurred, since the blur pads the texture out past the child's own bounds.
    #[cfg(feature = "wgpu")]
    #[inline]
    pub fn retain(mut self, content_key: u64) -> Self {
        self.retain = Some(content_key);
        self
    }

    /// Run the child ui and paint it.
    pub fn show<R>(self, ui: &mut Ui, mut content: impl FnMut(&mut Ui) -> R) -> ReguiOutput<R> {
        self.show_with_root(ui, move |ui, _| content(ui))
    }

    /// Run the child ui and paint it, with a way out into the ui hosting it.
    ///
    /// See [`RootScope`] for what the second argument is for: menus and anything else that
    /// has no business being clipped to the child or rotated with it.
    ///
    /// `'a` is how long whatever the child hands out may borrow for. It is a parameter
    /// rather than an elided lifetime on purpose: elided, it would be higher-ranked, the
    /// content function would have to satisfy every possible `'a`, and the only closures
    /// that do are `'static` ones - which rules out a menu that reads the state its button
    /// was drawn from. Inferred at the call site instead, it comes out as "at least as long
    /// as this call", which is exactly as long as the queued closures live.
    pub fn show_with_root<'a, R>(
        self,
        ui: &mut Ui,
        content: impl FnMut(&mut Ui, &RootScope<'a>) -> R,
    ) -> ReguiOutput<R> {
        let retained = self.show_impl(ui, content);
        ReguiOutput {
            response: retained.response,
            #[expect(clippy::expect_used)] // Without a retain key the child always runs.
            inner: retained
                .inner
                .expect("Bug in regui: the child was skipped without `Regui::retain`"),
            transform: retained.transform,
            viewport_id: retained.viewport_id,
        }
    }

    /// Run the child ui and paint it, unless [`Self::retain`] says the last image will do.
    ///
    /// `inner` is `None` on the passes where the child did not run, so whatever the ui
    /// function returns has to be something the caller can go without for a while - a
    /// click that was not clicked, a rect that has not moved.
    pub fn show_retained<R>(
        self,
        ui: &mut Ui,
        mut content: impl FnMut(&mut Ui) -> R,
    ) -> ReguiOutput<Option<R>> {
        self.show_impl(ui, move |ui, _| content(ui))
    }

    /// [`Self::show_retained`], with a way out into the ui hosting the child.
    ///
    /// A child that put anything out there last pass does not reuse its image: what it
    /// queued is drawn by a pass that did not happen, so retaining a child with a menu
    /// open would take the menu down. See [`RootScope`].
    pub fn show_retained_with_root<'a, R>(
        self,
        ui: &mut Ui,
        content: impl FnMut(&mut Ui, &RootScope<'a>) -> R,
    ) -> ReguiOutput<Option<R>> {
        self.show_impl(ui, content)
    }

    fn show_impl<'a, R>(
        self,
        ui: &mut Ui,
        mut content: impl FnMut(&mut Ui, &RootScope<'a>) -> R,
    ) -> ReguiOutput<Option<R>> {
        let Self {
            id_salt,
            size,
            scale,
            rotation,
            mirror_x,
            offset,
            crisp,
            interactive,
            placement,
            sense,
            pointer,
            blur,
            offscreen,
            // A blurred child is rendered inset into a padded texture, so the quad that
            // draws it is not the one a retained pass would build. Not worth a second
            // code path: a blur is there to be animated.
            retain: retain_key,
            auto_size,
        } = self;
        let retain_key = retain_key.filter(|_| blur <= 0.0);
        let offscreen = offscreen || blur > 0.0 || retain_key.is_some();

        let id = ui.make_persistent_id(id_salt);
        let viewport_id = ViewportId::from_hash_of(id);
        let ctx = ui.ctx().clone();
        let parent_id = ctx.viewport_id();

        let sense = sense.unwrap_or(if interactive {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        });

        let mut state: State = ctx.data_mut(|data| data.get_temp(id)).unwrap_or_default();

        // Space has to be reserved before the child can be measured - measuring means
        // running it, and a child that turns out to be retained never runs. So an
        // auto-sized child is laid out at the size it came out at last time, and measured
        // again only on a pass that was going to run anyway. What that costs is a pass of
        // lag in the *parent's* geometry, not the child's: the pass that changes size still
        // renders at the size it measured, it is the rect the parent reserved and hit-tests
        // against that is a pass behind.
        let max_size = size;
        let size = if auto_size {
            state.measured_size.unwrap_or(max_size)
        } else {
            size
        };

        let (transform, response) = match placement {
            Some(placement) => place(ui, id, size, placement, offset, sense),
            None => allocate(ui, size, scale, rotation, mirror_x, offset, sense),
        };

        if !transform.is_valid() {
            // A scale of zero or a NaN rotation would make the inverse transform, and
            // therefore every pointer position we hand the child, garbage.
            log::warn!("regui: skipping a child ui with an unusable transform: {transform:?}");
            // No child, so nothing to escape from: the content is running in the host
            // already and whatever it queues can just run there too.
            let scope = RootScope::new(Transform::IDENTITY, Vec::new());
            let inner = content(ui, &scope);
            drop(scope.run(ui));
            return ReguiOutput {
                response,
                inner: Some(inner),
                transform,
                viewport_id,
            };
        }

        // Anything the host draws on top of the child is not the child's to react to - a
        // menu the child handed out, a modal over the whole app. The parent's `Response`
        // works this out on its own, but a caller deciding [`Regui::pointer`] for itself
        // cannot: it knows the shape of its own chrome, not what is covering it. Missing
        // this is subtle rather than obvious - the child goes on hit-testing underneath an
        // open menu, so a click on a menu item lands on the menu *and* on whatever the menu
        // is drawn over.
        let occluded = ui
            .input(|input| input.pointer.latest_pos())
            .is_some_and(|pos| {
                ctx.layer_id_at(pos)
                    .is_some_and(|layer| layer != ui.layer_id())
            });
        let has_pointer =
            interactive && !occluded && pointer.unwrap_or_else(|| input::wants_pointer(&response));
        let gate = Gate {
            pointer: has_pointer,
            keyboard: interactive && state.child_has_focus,
        };
        let pointer_left = state.had_pointer && !has_pointer;
        state.had_pointer = has_pointer;

        // `native_pixels_per_point`, not the effective scale: egui multiplies it by the
        // global zoom factor for us, and doing that twice would zoom the child twice.
        let native_pixels_per_point = ctx.pixels_per_point() / ctx.zoom_factor();
        let child_pixels_per_point =
            native_pixels_per_point * if crisp { transform.scale.abs() } else { 1.0 };

        // What the child's own pass will report back as its `pixels_per_point`, and
        // therefore the density its texture is allocated at: egui multiplies the native
        // scale we hand it by the app-wide zoom factor.
        let texture_pixels_per_point = child_pixels_per_point * ctx.zoom_factor();

        let now = ui.input(|input| input.time);
        let caller_key = retain_key;
        let retain_key =
            retain_key.map(|key| content_key(&ctx, key, size, texture_pixels_per_point));

        // An accessibility action addresses a widget, and a child that does not run this
        // pass has no widgets to address - the action would be forwarded into a pass that
        // never happens. Rare enough that running every child on one costs nothing.
        let accesskit_action = ui.input(|input| {
            input
                .events
                .iter()
                .any(|event| matches!(event, Event::AccessKitActionRequest(_)))
        });

        let reused = retain_key.is_some_and(|key| {
            !accesskit_action
                && may_reuse(&state, key, has_pointer, pointer_left, now)
                && reuse(ui, id, size, texture_pixels_per_point, transform)
        });
        if reused {
            keep_accessible(&ctx, viewport_id, id, transform);
            state.ran_last_pass = false;
            ctx.data_mut(|data| data.insert_temp(id, state));
            return ReguiOutput {
                response,
                inner: None,
                transform,
                viewport_id,
            };
        }

        // Now that the child is definitely running, ask how big it wants to be. The answer
        // is what it renders at this pass, and what the parent will reserve for it next.
        let size = if auto_size {
            let measured = measure(
                ui,
                &mut content,
                viewport_id,
                max_size,
                child_pixels_per_point,
            );
            state.measured_size = Some(measured);
            measured
        } else {
            size
        };
        let retain_key =
            caller_key.map(|key| content_key(&ctx, key, size, texture_pixels_per_point));

        // The way out for anything the child would rather not have clipped to itself.
        // Composed with whatever an enclosing child is already doing, so the geometry is
        // right at any depth even though the layer only rises by one.
        // Read here, in the host's pass, because that is the viewport the answer is kept
        // in. See [`RootScope::open`].
        let open_popups: Vec<Id> = ctx
            .data(|data| data.get_temp(open_popups_id(viewport_id)))
            .unwrap_or_default();
        let scope = RootScope::new(
            ancestor_transform(&ctx, parent_id).then(transform),
            open_popups,
        );
        ctx.data_mut(|data| data.insert_temp(to_root_id(viewport_id), scope.transform()));

        let (inner, rendered_offscreen) = run_child(
            ui,
            |ui| in_scope(ui, auto_size, |ui| content(ui, &scope)),
            &Pass {
                id,
                viewport_id,
                parent_id,
                size,
                transform,
                pixels_per_point: child_pixels_per_point,
                gate,
                pointer_left,
                pointer_is_over: response.contains_pointer(),
                blur,
                offscreen,
                prime: retain_key.is_some() && !state.ran_last_pass,
                now,
            },
            &mut state,
        );
        // Only an off-screen pass leaves an image behind; without one there is nothing to
        // retain, whatever the caller asked for.
        state.cached_key = retain_key.filter(|_| rendered_offscreen);
        state.had_root_ui = !scope.is_empty();
        ctx.data_mut(|data| data.insert_temp(id, state));

        // After the pass, so the child's own image is under whatever it put out here, and
        // after the state is written, so a menu that runs its own regui sees the truth.
        let open_popups = scope.run(ui);
        ctx.data_mut(|data| data.insert_temp(open_popups_id(viewport_id), open_popups));

        ReguiOutput {
            response,
            inner: Some(inner),
            transform,
            viewport_id,
        }
    }
}

/// Run the content at the same ui depth whether this is the pass that counts or the one
/// that measures it.
///
/// Only for an auto-sized child, which is the only one that gets measured. A nesting level
/// that appeared in one pass and not the other would move every widget id inside it, and
/// the two passes would be talking about different widgets.
fn in_scope<R>(ui: &mut Ui, wrap: bool, content: impl FnOnce(&mut Ui) -> R) -> R {
    if wrap {
        ui.scope_builder(egui::UiBuilder::new(), content).inner
    } else {
        content(ui)
    }
}

/// Lay the child out at the biggest it may be, and see how much of that it wants.
///
/// A sizing pass in egui's sense: a widget that would otherwise fill whatever it is given
/// reports what it needs instead, so the answer is the child's own size rather than an echo
/// of the size the question was asked with. The pass is thrown away afterwards — it exists
/// to be measured, not seen — but it is a real pass of the content, so it is given no
/// events to act on and nothing it hands out is kept.
fn measure<'a, R>(
    ui: &mut Ui,
    content: &mut impl FnMut(&mut Ui, &RootScope<'a>) -> R,
    viewport_id: ViewportId,
    max: Vec2,
    pixels_per_point: f32,
) -> Vec2 {
    let ctx = ui.ctx().clone();
    let input = input::priming_input(input::child_input(
        ui,
        &input::ChildInput {
            viewport_id,
            size: max,
            pixels_per_point,
            // Nothing positional survives `priming_input` with the pointer gated off, so
            // there is nothing for this to map and the real transform is not needed.
            to_child: Transform::IDENTITY,
            gate: Gate {
                pointer: false,
                keyboard: false,
            },
            pointer_gone: false,
            resync_modifiers: None,
        },
    ));

    let (output, wanted) = ctx.run_hosted_viewport(viewport_id, input, |ui| {
        let laid_out = ui
            .scope_builder(egui::UiBuilder::new().sizing_pass(), |ui| {
                // Anything handed out from a pass nobody sees is not wanted, and dropping
                // the scope with it still queued is how it is refused.
                let scope = RootScope::new(Transform::IDENTITY, Vec::new());
                drop(content(ui, &scope));
            })
            .response
            .rect;
        // A menu or a tooltip is an area of its own, so what the contents laid out says
        // nothing about it — and it is inside the child, so it has to fit too.
        ui.ctx().memory(|memory| {
            memory
                .areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|layer| layer.order != egui::Order::Background)
                .filter_map(|layer| memory.area_rect(layer.id))
                .fold(laid_out, |acc, area| acc.union(area))
        })
    });
    output.drop_without_applying_deltas();

    // From the child's origin, since that is the corner it keeps: `max` of the far corner,
    // not the size of a rect that might not start at zero.
    wanted.max.to_vec2().max(Vec2::ZERO).min(max)
}

/// Everything one pass of a child needs that the builder does not decide on its own.
struct Pass {
    id: Id,
    viewport_id: ViewportId,
    parent_id: ViewportId,
    size: Vec2,
    transform: Transform,
    pixels_per_point: f32,
    gate: Gate,
    pointer_left: bool,
    pointer_is_over: bool,
    blur: f32,
    offscreen: bool,

    /// Lay the child out once, inertly, before the pass that counts. See [`State::ran_last_pass`].
    prime: bool,

    /// The parent's input time, for the repaint deadline this pass leaves behind.
    now: f64,
}

/// Run the child, hand its output to the parent, and paint it.
///
/// Writes back what the next pass has to know about this one - focus, busyness, the
/// repaint it asked for - and returns whether it left an image behind to retain.
fn run_child<R>(
    ui: &mut Ui,
    mut content: impl FnMut(&mut Ui) -> R,
    pass: &Pass,
    state: &mut State,
) -> (R, bool) {
    let ctx = ui.ctx().clone();
    // A child that did not run last pass has no input state left to speak of, so tell it
    // what is held rather than leaving it to guess.
    let resync_modifiers = (!state.ran_last_pass).then(|| ui.input(|input| input.modifiers));
    let child_input = |pointer_gone| {
        input::child_input(
            ui,
            &input::ChildInput {
                viewport_id: pass.viewport_id,
                size: pass.size,
                pixels_per_point: pass.pixels_per_point,
                to_child: pass.transform.inverse(),
                gate: pass.gate,
                pointer_gone,
                resync_modifiers,
            },
        )
    };

    if pass.prime {
        // egui decides what was clicked from the widget rectangles of the previous pass,
        // and the passes we skipped left none - so the first pass back would hand every
        // widget in the child an empty hit-test and swallow the tap that woke it. Lay the
        // child out once with nothing in the input that can fire, which registers the
        // rectangles, and let the real pass below interact against those.
        let (output, ()) = ctx.run_hosted_viewport(
            pass.viewport_id,
            input::priming_input(child_input(false)),
            |ui| drop(content(ui)),
        );
        output.drop_without_applying_deltas();
    }

    let (output, (inner, child_has_focus, child_busy)) =
        ctx.run_hosted_viewport(pass.viewport_id, child_input(pass.pointer_left), |ui| {
            let inner = content(ui);
            // Read these inside the pass: they all report on whichever viewport is
            // current, and in here that is the child.
            let has_focus = ui.memory(|memory| memory.focused().is_some());
            let busy = ui.ctx().any_popup_open() || ui.ctx().dragged_id().is_some();
            (inner, has_focus, busy)
        });

    state.child_has_focus = child_has_focus;
    state.child_busy = child_busy;
    state.ran_last_pass = true;

    let FullOutput {
        platform_output,
        mut textures_delta,
        shapes,
        pixels_per_point,
        viewport_output: _,
    } = output;

    // A hosted viewport leaves this frame's texture uploads in the `Context` for the
    // viewport that hosts us to hand to its backend, so there is nothing here to pass
    // on. Anything else would mean egui had drained them behind our back.
    debug_assert!(
        textures_delta.is_empty(),
        "regui: expected egui to leave the texture uploads to the hosting viewport"
    );
    textures_delta.clear();

    output::forward_platform_output(&ctx, platform_output, pass.transform, pass.pointer_is_over);
    let accessibility = Accessibility(output::forward_accesskit(
        &ctx,
        pass.viewport_id,
        pass.id,
        pass.transform,
        None,
    ));
    ctx.data_mut(|data| data.insert_temp(pass.id, accessibility));
    output::forward_repaint(&ctx, pass.viewport_id, pass.parent_id);
    state.repaint_at = output::repaint_deadline(&ctx, pass.viewport_id, pass.now);

    let primitives = ctx.tessellate(shapes, pixels_per_point);
    let rendered_offscreen = paint(
        ui,
        Painted {
            id: pass.id,
            primitives,
            size: pass.size,
            pixels_per_point,
            transform: pass.transform,
            blur_radius: pass.blur * pixels_per_point,
            offscreen: pass.offscreen,
        },
    );

    (inner, rendered_offscreen)
}

/// Put a skipped child's widgets into the parent's tree again.
///
/// A retained child is on screen, so it has to be in the tree; it just did not build one
/// this pass, so the nodes from the pass that did are grafted instead.
fn keep_accessible(ctx: &egui::Context, viewport_id: ViewportId, id: Id, transform: Transform) {
    let remembered: Accessibility = ctx.data_mut(|data| data.get_temp(id)).unwrap_or_default();
    let grafted = output::forward_accesskit(ctx, viewport_id, id, transform, remembered.0);
    ctx.data_mut(|data| data.insert_temp(id, Accessibility(grafted)));
}

/// Paint the image the child left behind last time, if it is still there.
///
/// The transform is applied to the quad rather than baked into the image, so a child that
/// only moves - a sliding panel, a spinning thumbnail - stays retained while it does.
#[cfg(feature = "wgpu")]
fn reuse(ui: &Ui, id: Id, size: Vec2, pixels_per_point: f32, transform: Transform) -> bool {
    let Some(render_state) = crate::wgpu_state::render_state(ui.ctx()) else {
        return false;
    };
    let Some(shape) = backend::texture::reuse(&render_state, id, size, pixels_per_point, transform)
    else {
        return false;
    };
    ui.painter().add(shape);
    true
}

/// Without wgpu there is no texture to keep, so a child never gets to sit a pass out.
#[cfg(not(feature = "wgpu"))]
fn reuse(_: &Ui, _: Id, _: Vec2, _: f32, _: Transform) -> bool {
    false
}

/// May the image from the last pass stand in for running the child again?
///
/// Only when nothing it shows has changed *and* it is not in the middle of anything: a
/// skipped pass drops the child's viewport, taking its open menus, its drag and its
/// hit-testing with it.
fn may_reuse(
    state: &State,
    retain_key: u64,
    has_pointer: bool,
    pointer_left: bool,
    now: f64,
) -> bool {
    state.cached_key == Some(retain_key)
        && !has_pointer
        // The pass that hands the child a `PointerGone` has to be a real one, or the child
        // keeps a hover highlight on the widget the pointer left.
        && !pointer_left
        && !state.child_has_focus
        && !state.child_busy
        && !state.had_root_ui
        // A child that asked to be woken has something left to draw - an animation, a
        // caret - so let it, and reconsider once it stops asking.
        && state.repaint_at.is_none_or(|at| at > now)
}

/// Everything `regui` folds into the caller's key on its own.
///
/// These all change what the child looks like without changing the state the caller
/// hashed, and getting one of them wrong shows up as a panel that keeps a stale image
/// after a theme switch or a window move between monitors.
fn content_key(ctx: &egui::Context, caller_key: u64, size: Vec2, pixels_per_point: f32) -> u64 {
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    caller_key.hash(&mut hasher);
    size.x.to_bits().hash(&mut hasher);
    size.y.to_bits().hash(&mut hasher);
    pixels_per_point.to_bits().hash(&mut hasher);
    // The whole style, by identity: egui replaces the `Arc` rather than mutating it, so a
    // changed pointer is exactly "someone restyled the app".
    let theme = ctx.theme();
    std::sync::Arc::as_ptr(&ctx.style_of(theme)).hash(&mut hasher);
    theme.hash(&mut hasher);
    hasher.finish()
}

/// Everything the backends need to draw one pass of a child.
struct Painted {
    id: Id,
    primitives: Vec<egui::ClippedPrimitive>,
    size: Vec2,
    pixels_per_point: f32,
    transform: Transform,

    /// Blur radius over the child's own image, in physical pixels.
    blur_radius: f32,

    /// Whether to go through a texture rather than hand the parent triangles.
    offscreen: bool,
}

/// Draw the child, off-screen if asked for and possible, and by replaying its triangles
/// otherwise.
///
/// Returns whether it went through a texture, which is the same question as "is there an
/// image left over for the next pass to reuse".
fn paint(ui: &Ui, painted: Painted) -> bool {
    let Painted {
        primitives,
        transform,
        ..
    } = painted;

    #[cfg(feature = "wgpu")]
    let primitives = {
        let mut primitives = primitives;
        if painted.offscreen {
            if let Some(render_state) = crate::wgpu_state::render_state(ui.ctx()) {
                let request = backend::texture::Request {
                    id: painted.id,
                    primitives,
                    size: painted.size,
                    pixels_per_point: painted.pixels_per_point,
                    transform,
                    blur_radius: painted.blur_radius,
                };
                if let Some(shape) = backend::texture::render(ui, &render_state, request) {
                    ui.painter().add(shape);
                    return true;
                }
                // It could not render after all, so there is nothing left to fall back with.
                primitives = Vec::new();
            } else {
                crate::wgpu_state::warn_not_installed(ui.ctx(), "Regui::offscreen");
                primitives = Vec::new();
            }
        }
        primitives
    };

    backend::shapes::paint(ui, primitives, transform);
    false
}

/// Reserve room for the child in the parent and work out where it lands.
///
/// Rotating around the child's own origin moves it off the space we were given, so lay out
/// the bounding box of the rotated child and then shift the child back into it.
fn allocate(
    ui: &mut Ui,
    size: Vec2,
    scale: f32,
    rotation: f32,
    mirror_x: bool,
    offset: Vec2,
    sense: Sense,
) -> (Transform, Response) {
    let child_rect = Rect::from_min_size(Pos2::ZERO, size);
    let unplaced = Transform {
        scale,
        rotation: Rot2::from_angle(rotation),
        translation: Vec2::ZERO,
        mirror_x,
    };
    let bounds = unplaced.bounding_rect(child_rect);
    let (rect, response) = ui.allocate_exact_size(bounds.size(), sense);
    let transform = Transform {
        translation: (rect.min - bounds.min) + offset,
        ..unplaced
    };
    (transform, response)
}

/// Take the caller's transform as given, and only interact with where it puts the child.
///
/// No space is reserved: a caller who brought their own transform has already decided
/// where the child goes, and reserving space at the ui cursor would push the layout around
/// for a child that is not there.
fn place(
    ui: &mut Ui,
    id: Id,
    size: Vec2,
    placement: Transform,
    offset: Vec2,
    sense: Sense,
) -> (Transform, Response) {
    let transform = Transform {
        translation: placement.translation + offset,
        ..placement
    };
    let bounds = transform.bounding_rect(Rect::from_min_size(Pos2::ZERO, size));
    let response = ui.interact(bounds, id, sense);
    (transform, response)
}
