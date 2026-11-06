//! The part of the Android window surface that the system bars leave to the app.
//!
//! `ANativeWindow` is the *whole* window surface, including the status bar and the
//! navigation/gesture bar. `NativeActivity` lays its content view out below/above those bars and
//! reports where it ended up through `onContentRectChanged`; `android-activity` stores that in
//! `AndroidApp::content_rect`, while winit 0.30 ignores the event. The shell reads the rectangle
//! every frame and hands egui the resulting safe area; without it the ribbon is drawn underneath
//! the status bar on devices that lay the surface out edge-to-edge.
//!
//! Everything here is plain integer/float arithmetic so it can be unit-tested on the host.

/// The content rectangle in physical pixels, relative to the window's top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ContentRect {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
}

impl ContentRect {
    /// The rectangle, or `None` before the Java side has laid the content view out.
    ///
    /// A zero or inverted rectangle means "not known yet", never "the content fills the window":
    /// treating it as known would put the interface under the system bars during startup.
    pub(crate) fn usable(self) -> Option<Self> {
        (self.right > self.left && self.bottom > self.top).then_some(self)
    }

    /// Insets around a `width` x `height` surface, in physical pixels.
    ///
    /// `None` when the rectangle is not usable. Each inset is clamped at zero, so a rectangle
    /// that misses the surface by a pixel is treated as flush with that edge rather than as a
    /// negative inset that would grow the interface.
    pub(crate) fn insets(self, width: i32, height: i32) -> Option<Insets> {
        let rect = self.usable()?;
        if width <= 0 || height <= 0 {
            return None;
        }
        Some(Insets {
            left: rect.left.max(0).min(width),
            top: rect.top.max(0).min(height),
            right: width.saturating_sub(rect.right).max(0).min(width),
            bottom: height.saturating_sub(rect.bottom).max(0).min(height),
        })
    }
}

/// The space taken by the system bars on each side, in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Insets {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
}

impl Insets {
    /// The insets in egui points as `[left, right, top, bottom]`.
    ///
    /// `None` for a density that is not a positive, finite number; dividing by it would produce
    /// NaN or infinite margins and hide the interface.
    pub(crate) fn to_points(self, pixels_per_point: f32) -> Option<[f32; 4]> {
        if !(pixels_per_point.is_finite() && pixels_per_point > 0.0) {
            return None;
        }
        Some([
            self.left as f32 / pixels_per_point,
            self.right as f32 / pixels_per_point,
            self.top as f32 / pixels_per_point,
            self.bottom as f32 / pixels_per_point,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_inverted_rects_are_not_usable() {
        assert_eq!(ContentRect::default().usable(), None);
        assert_eq!(ContentRect { left: 10, top: 0, right: 10, bottom: 20 }.usable(), None);
        assert_eq!(ContentRect { left: 0, top: 10, right: 20, bottom: 10 }.usable(), None);
        assert!(ContentRect { left: 0, top: 0, right: 20, bottom: 10 }.usable().is_some());
    }

    #[test]
    fn insets_measure_the_bars_around_the_content_rect() {
        // A 1600x2560 surface whose content view starts below a 48 px status bar and ends above a
        // 64 px navigation bar, with a 24 px cutout on the left in landscape.
        let rect = ContentRect { left: 24, top: 48, right: 1600, bottom: 2496 };
        assert_eq!(rect.insets(1600, 2560), Some(Insets { left: 24, top: 48, right: 0, bottom: 64 }));
    }

    #[test]
    fn a_rect_outside_the_surface_never_gives_negative_insets() {
        let rect = ContentRect { left: -8, top: -8, right: 1640, bottom: 2600 };
        assert_eq!(rect.insets(1600, 2560), Some(Insets { left: 0, top: 0, right: 0, bottom: 0 }));
    }

    #[test]
    fn a_huge_rect_is_clamped_to_the_surface() {
        let rect = ContentRect { left: i32::MAX - 10, top: i32::MAX - 10, right: i32::MAX, bottom: i32::MAX };
        assert_eq!(rect.insets(1600, 2560), Some(Insets { left: 1600, top: 2560, right: 0, bottom: 0 }));
    }

    #[test]
    fn an_unknown_surface_size_is_rejected() {
        let rect = ContentRect { left: 0, top: 48, right: 1600, bottom: 2496 };
        assert_eq!(rect.insets(0, 2560), None);
        assert_eq!(rect.insets(1600, 0), None);
        assert_eq!(rect.insets(-1, -1), None);
    }

    #[test]
    fn points_divide_physical_pixels_by_the_density() {
        let insets = Insets { left: 12, top: 48, right: 6, bottom: 64 };
        assert_eq!(insets.to_points(2.0), Some([6.0, 3.0, 24.0, 32.0]));
        assert_eq!(insets.to_points(1.0), Some([12.0, 6.0, 48.0, 64.0]));
    }

    #[test]
    fn a_bad_density_is_rejected() {
        let insets = Insets { left: 12, top: 48, right: 6, bottom: 64 };
        assert_eq!(insets.to_points(0.0), None);
        assert_eq!(insets.to_points(-2.0), None);
        assert_eq!(insets.to_points(f32::NAN), None);
        assert_eq!(insets.to_points(f32::INFINITY), None);
    }
}
