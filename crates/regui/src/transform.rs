use egui::{Pos2, Rect, Vec2, emath::Rot2};

/// Maps the child ui's coordinates to the parent ui's coordinates.
///
/// An optional mirror, then a uniform scale, then a rotation, then a translation. Mirror,
/// scale and rotation are all around the child's origin; [`Regui`](crate::Regui) picks the
/// translation so that the result lands in the space it allocated in the parent.
///
/// Skew and non-uniform scale are left out on purpose: egui strokes, corner radii and
/// blur widths are all single numbers, so they cannot survive a transform that scales x
/// and y differently. The mirror is exempt because it is isometric — it changes which way
/// the child faces, never how wide anything is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    /// Uniform scale, applied after the mirror.
    pub scale: f32,

    /// Rotation, applied after the scale.
    pub rotation: Rot2,

    /// Translation, applied last.
    pub translation: Vec2,

    /// Reflect the child across its own vertical axis, before everything else.
    ///
    /// This is not a negative [`Self::scale`]: scale is uniform, so `-1` means
    /// `diag(-1, -1)`, which is a half turn — it preserves orientation and leaves the
    /// child facing the same way. Turning the child around means flipping one axis, and
    /// that needs its own flag.
    ///
    /// Text inside a mirrored child reads backwards, which is a good reason to keep
    /// labels out of one.
    pub mirror_x: bool,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    /// Leaves everything where it is.
    pub const IDENTITY: Self = Self {
        scale: 1.0,
        rotation: Rot2::IDENTITY,
        translation: Vec2::ZERO,
        mirror_x: false,
    };

    /// A uniform scale around the origin.
    pub fn from_scale(scale: f32) -> Self {
        Self {
            scale,
            ..Self::IDENTITY
        }
    }

    /// A rotation around the origin, in radians, clockwise on screen.
    pub fn from_rotation(angle: f32) -> Self {
        Self {
            rotation: Rot2::from_angle(angle),
            ..Self::IDENTITY
        }
    }

    /// A translation.
    pub fn from_translation(translation: Vec2) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    /// A reflection across the vertical axis through the origin.
    pub fn from_mirror_x() -> Self {
        Self {
            mirror_x: true,
            ..Self::IDENTITY
        }
    }

    /// Apply just the mirror, which comes before everything else.
    fn mirrored(self, vec: Vec2) -> Vec2 {
        if self.mirror_x {
            Vec2::new(-vec.x, vec.y)
        } else {
            vec
        }
    }

    /// Map a position from child space to parent space.
    pub fn mul_pos(self, pos: Pos2) -> Pos2 {
        (self.rotation * (self.scale * self.mirrored(pos.to_vec2())) + self.translation).to_pos2()
    }

    /// Map a direction or a distance from child space to parent space.
    ///
    /// Unlike [`Self::mul_pos`], this ignores the translation.
    pub fn mul_vec(self, vec: Vec2) -> Vec2 {
        self.rotation * (self.scale * self.mirrored(vec))
    }

    /// This transform followed by `other`.
    ///
    /// `a.then(b).mul_pos(p) == b.mul_pos(a.mul_pos(p))`. Composing child-to-parent
    /// transforms up a chain of nested children is how a point in the innermost one is
    /// placed on the screen everything is finally drawn on.
    pub fn then(self, other: Self) -> Self {
        Self {
            scale: self.scale * other.scale,
            // A mirror between the two rotations reverses the sense of the first
            // (`M·R·M = R⁻¹`), and `other`'s mirror is applied before its rotation.
            rotation: other.rotation
                * if other.mirror_x {
                    self.rotation.inverse()
                } else {
                    self.rotation
                },
            translation: other.mul_vec(self.translation) + other.translation,
            mirror_x: self.mirror_x ^ other.mirror_x,
        }
    }

    /// The transform that undoes this one.
    pub fn inverse(self) -> Self {
        // Undoing a mirrored transform keeps the rotation rather than reversing it:
        // conjugating a rotation by a reflection reverses its sense (`M·R·M = R⁻¹`), and
        // the mirror the inverse still has to apply does that conjugating.
        let rotation = if self.mirror_x {
            self.rotation
        } else {
            self.rotation.inverse()
        };
        let scale = 1.0 / self.scale;
        Self {
            scale,
            rotation,
            translation: rotation * (-self.mirrored(self.translation) * scale),
            mirror_x: self.mirror_x,
        }
    }

    /// The smallest axis-aligned rectangle in parent space that contains the transformed
    /// `rect`.
    ///
    /// This is the same as the transformed rectangle when [`Self::is_axis_aligned`], and
    /// larger than it otherwise.
    pub fn bounding_rect(self, rect: Rect) -> Rect {
        Rect::from_points(&[
            self.mul_pos(rect.left_top()),
            self.mul_pos(rect.right_top()),
            self.mul_pos(rect.right_bottom()),
            self.mul_pos(rect.left_bottom()),
        ])
    }

    /// Does this transform keep horizontal lines horizontal?
    ///
    /// If it does, rectangles stay rectangles, so egui's clip rectangles survive the
    /// transform exactly. If it doesn't, they have to be widened to their bounding box.
    ///
    /// A mirror does not disturb this: it maps a horizontal line onto a horizontal line.
    pub fn is_axis_aligned(self) -> bool {
        // A tenth of a degree. Well below what anyone can see, and well above the error
        // that building a `Rot2` from an angle introduces.
        const EPSILON: f32 = 0.001_745;
        self.rotation.angle().abs() < EPSILON
    }

    /// This transform as a scale and a translation, when that is all it is.
    ///
    /// `None` for a rotated or mirrored transform, which no amount of scaling and shifting
    /// reproduces. Handy where only rectangles are on offer, such as accessibility bounds.
    pub fn as_scale_translation(self) -> Option<egui::emath::TSTransform> {
        (self.is_axis_aligned() && !self.mirror_x).then_some(egui::emath::TSTransform {
            scaling: self.scale,
            translation: self.translation,
        })
    }

    /// Is this transform usable, i.e. finite and not collapsed to nothing?
    pub fn is_valid(self) -> bool {
        self.scale.is_finite()
            && self.scale.abs() > f32::EPSILON
            && self.rotation.is_finite()
            && self.translation.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::Transform;
    use egui::{Vec2, emath::Rot2, pos2, vec2};

    fn assert_close(a: egui::Pos2, b: egui::Pos2) {
        assert!((a - b).length() < 1e-4, "{a:?} != {b:?}");
    }

    #[test]
    fn inverse_undoes_the_transform() {
        let transform = Transform {
            scale: 2.5,
            rotation: Rot2::from_angle(0.7),
            translation: vec2(13.0, -4.0),
            mirror_x: false,
        };
        let point = pos2(3.0, 8.0);
        assert_close(transform.inverse().mul_pos(transform.mul_pos(point)), point);
        assert_close(transform.mul_pos(transform.inverse().mul_pos(point)), point);
    }

    #[test]
    fn inverse_undoes_a_mirrored_transform() {
        let transform = Transform {
            scale: 2.5,
            rotation: Rot2::from_angle(0.7),
            translation: vec2(13.0, -4.0),
            mirror_x: true,
        };
        let point = pos2(3.0, 8.0);
        assert_close(transform.inverse().mul_pos(transform.mul_pos(point)), point);
        assert_close(transform.mul_pos(transform.inverse().mul_pos(point)), point);
    }

    #[test]
    fn a_mirror_is_not_a_negative_scale() {
        let point = pos2(3.0, 8.0);
        // Flipping one axis turns the child around...
        assert_close(Transform::from_mirror_x().mul_pos(point), pos2(-3.0, 8.0));
        // ...while a negative uniform scale is a half turn, which does not.
        assert_close(Transform::from_scale(-1.0).mul_pos(point), pos2(-3.0, -8.0));
    }

    #[test]
    fn a_mirror_keeps_lines_horizontal() {
        assert!(Transform::from_mirror_x().is_axis_aligned());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(10.0, 4.0));
        let bounds = Transform::from_mirror_x().bounding_rect(rect);
        assert_eq!(
            bounds,
            egui::Rect::from_min_max(pos2(-10.0, 0.0), pos2(0.0, 4.0))
        );
    }

    #[test]
    fn identity_is_axis_aligned_but_a_quarter_turn_is_not() {
        assert!(Transform::IDENTITY.is_axis_aligned());
        assert!(Transform::from_scale(3.0).is_axis_aligned());
        assert!(Transform::from_translation(Vec2::splat(5.0)).is_axis_aligned());
        assert!(!Transform::from_rotation(std::f32::consts::FRAC_PI_2).is_axis_aligned());
        // A half turn keeps lines horizontal, but it flips them, which egui's clip
        // rectangles cannot express either.
        assert!(!Transform::from_rotation(std::f32::consts::PI).is_axis_aligned());
    }

    #[test]
    fn then_composes_in_order() {
        let point = pos2(3.0, 8.0);
        for first in [
            Transform::from_scale(2.0),
            Transform::from_rotation(0.4),
            Transform::from_translation(vec2(5.0, -2.0)),
            Transform::from_mirror_x(),
            Transform {
                scale: 1.7,
                rotation: Rot2::from_angle(-0.9),
                translation: vec2(-3.0, 11.0),
                mirror_x: true,
            },
        ] {
            for second in [
                Transform::from_scale(0.5),
                Transform::from_rotation(-1.2),
                Transform::from_translation(vec2(-7.0, 4.0)),
                Transform::from_mirror_x(),
                Transform {
                    scale: 0.8,
                    rotation: Rot2::from_angle(2.1),
                    translation: vec2(6.0, 6.0),
                    mirror_x: true,
                },
            ] {
                assert_close(
                    first.then(second).mul_pos(point),
                    second.mul_pos(first.mul_pos(point)),
                );
            }
        }
    }

    #[test]
    fn then_identity_changes_nothing() {
        let transform = Transform {
            scale: 1.3,
            rotation: Rot2::from_angle(0.6),
            translation: vec2(2.0, 9.0),
            mirror_x: true,
        };
        let point = pos2(-4.0, 5.0);
        assert_close(
            transform.then(Transform::IDENTITY).mul_pos(point),
            transform.mul_pos(point),
        );
        assert_close(
            Transform::IDENTITY.then(transform).mul_pos(point),
            transform.mul_pos(point),
        );
    }

    #[test]
    fn bounding_rect_grows_when_rotated() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(10.0, 10.0));
        assert_eq!(Transform::IDENTITY.bounding_rect(rect), rect);

        let rotated = Transform::from_rotation(std::f32::consts::FRAC_PI_4).bounding_rect(rect);
        let expected = 10.0 * std::f32::consts::SQRT_2;
        assert!((rotated.width() - expected).abs() < 1e-4);
        assert!((rotated.height() - expected).abs() < 1e-4);
    }
}
