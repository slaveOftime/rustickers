//! Multi-display placement memory for sticker windows.
//!
//! A sticker remembers one placement per monitor, keyed by that monitor's stable UUID, plus the
//! monitor the user last deliberately moved it to (its *preferred* display). Resolving those two
//! pieces of state against the currently connected monitors is enough to handle application
//! start-up, unplugging a monitor and plugging it back in with a single rule:
//!
//! - the target monitor is the preferred one when it is connected, otherwise the primary one;
//! - if the sticker has a record for the target monitor, that record is used verbatim (only
//!   clamped, so a monitor that changed resolution still shows the window);
//! - otherwise a placement is derived from the best available record by rescaling it to the
//!   target's DPI and clamping it into the target's work area. Derived placements are never
//!   written back, so the absent monitor's memory stays intact.
//!
//! Everything here is deliberately free of platform APIs so it can be unit tested anywhere.

use crate::model::sticker::StickerPlacement;

/// `(left, top, right, bottom)` of a monitor's work area, in native pixels.
pub type WorkArea = (i32, i32, i32, i32);

/// A window rectangle in native virtual-screen (physical) pixels.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct NativeRect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl NativeRect {
    /// Convert between monitors of differing DPI, keeping the window's apparent size.
    pub fn scaled(self, from_scale_factor: f32, to_scale_factor: f32) -> Self {
        if from_scale_factor <= 0.0 || to_scale_factor <= 0.0 {
            return self;
        }
        let ratio = (to_scale_factor / from_scale_factor) as f64;
        Self {
            width: (self.width as f64 * ratio).round() as i32,
            height: (self.height as f64 * ratio).round() as i32,
            ..self
        }
    }

    /// Move the rect back inside a work area, keeping it fully visible when it fits.
    pub fn clamped_into(self, work_area: WorkArea) -> Self {
        let (left, top) =
            clamp_into_work_area((self.left, self.top), (self.width, self.height), work_area);
        Self { left, top, ..self }
    }
}

/// Read the geometry of a stored placement as a rectangle.
pub fn placement_rect(placement: &StickerPlacement) -> NativeRect {
    NativeRect {
        left: placement.native_left,
        top: placement.native_top,
        width: placement.native_width,
        height: placement.native_height,
    }
}

/// A monitor that is connected right now.
#[derive(PartialEq, Clone, Debug)]
pub struct DisplayEntry {
    pub uuid: String,
    pub display_id: Option<i64>,
    pub scale_factor: f32,
    pub work_area: WorkArea,
    pub is_primary: bool,
}

/// Where a sticker should be placed given the monitors that exist right now.
#[derive(PartialEq, Clone, Debug)]
pub struct ResolvedPlacement {
    pub display_uuid: String,
    pub display_id: Option<i64>,
    pub rect: NativeRect,
    /// True when the rect came from a record saved on this very monitor, false when it had to be
    /// derived from another monitor's record.
    pub exact: bool,
}

/// Pick the monitor and rectangle a sticker should use.
///
/// Returns `None` when there is nothing to go on, in which case the caller keeps whatever bounds
/// it already had.
pub fn resolve_placement(
    placements: &[StickerPlacement],
    preferred_uuid: Option<&str>,
    displays: &[DisplayEntry],
) -> Option<ResolvedPlacement> {
    let primary = displays
        .iter()
        .find(|display| display.is_primary)
        .or_else(|| displays.first())?;
    let target = preferred_uuid
        .and_then(|uuid| displays.iter().find(|display| display.uuid == uuid))
        .unwrap_or(primary);

    if let Some(record) = placements
        .iter()
        .find(|placement| placement.display_uuid == target.uuid)
    {
        return Some(ResolvedPlacement {
            display_uuid: target.uuid.clone(),
            display_id: target.display_id,
            rect: placement_rect(record).clamped_into(target.work_area),
            exact: true,
        });
    }

    // No memory of this monitor: carry the sticker over from the monitor it belongs to, or from
    // wherever it was seen last.
    let source = preferred_uuid
        .and_then(|uuid| {
            placements
                .iter()
                .find(|placement| placement.display_uuid == uuid)
        })
        .or_else(|| newest(placements))?;

    let rect = placement_rect(source)
        .scaled(source.scale_factor, target.scale_factor)
        .clamped_into(target.work_area);

    Some(ResolvedPlacement {
        display_uuid: target.uuid.clone(),
        display_id: target.display_id,
        rect,
        exact: false,
    })
}

fn newest(placements: &[StickerPlacement]) -> Option<&StickerPlacement> {
    placements.iter().max_by(|a, b| {
        a.updated_at
            .cmp(&b.updated_at)
            .then_with(|| b.display_uuid.cmp(&a.display_uuid))
    })
}

/// Move a window rect back inside a work area, keeping it fully visible when it fits.
pub fn clamp_into_work_area(
    position: (i32, i32),
    size: (i32, i32),
    work_area: WorkArea,
) -> (i32, i32) {
    let (left, top) = position;
    let (width, height) = size;
    let (area_left, area_top, area_right, area_bottom) = work_area;
    let max_left = (area_right - width).max(area_left);
    let max_top = (area_bottom - height).max(area_top);
    (
        left.clamp(area_left, max_left),
        top.clamp(area_top, max_top),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY_AREA: WorkArea = (0, 0, 1920, 1040);
    const RIGHT_AREA: WorkArea = (1920, -1200, 4480, 240);

    fn rect(left: i32, top: i32) -> NativeRect {
        NativeRect {
            left,
            top,
            width: 210,
            height: 114,
        }
    }

    fn primary_display() -> DisplayEntry {
        DisplayEntry {
            uuid: "primary".into(),
            display_id: Some(1),
            scale_factor: 1.0,
            work_area: PRIMARY_AREA,
            is_primary: true,
        }
    }

    fn right_display() -> DisplayEntry {
        DisplayEntry {
            uuid: "right".into(),
            display_id: Some(2),
            scale_factor: 1.0,
            work_area: RIGHT_AREA,
            is_primary: false,
        }
    }

    fn placement(uuid: &str, rect: NativeRect, updated_at: i64) -> StickerPlacement {
        StickerPlacement {
            display_uuid: uuid.into(),
            display_id: None,
            native_left: rect.left,
            native_top: rect.top,
            native_width: rect.width,
            native_height: rect.height,
            scale_factor: 1.0,
            updated_at,
        }
    }

    #[test]
    fn the_preferred_display_wins_while_it_is_connected() {
        let placements = [
            placement("primary", rect(100, 100), 1),
            placement("right", rect(2307, -1000), 2),
        ];
        let resolved = resolve_placement(
            &placements,
            Some("right"),
            &[primary_display(), right_display()],
        )
        .expect("a placement");

        assert_eq!(resolved.display_uuid, "right");
        assert_eq!(resolved.rect, rect(2307, -1000));
        assert!(resolved.exact);
    }

    #[test]
    fn unplugging_the_preferred_display_returns_to_the_remembered_primary_spot() {
        let placements = [
            placement("primary", rect(100, 100), 1),
            placement("right", rect(2307, -1000), 2),
        ];
        let resolved = resolve_placement(&placements, Some("right"), &[primary_display()])
            .expect("a placement");

        assert_eq!(resolved.display_uuid, "primary");
        assert_eq!(resolved.rect, rect(100, 100));
        assert!(resolved.exact);
    }

    #[test]
    fn a_sticker_that_only_lived_on_a_gone_display_is_derived_onto_the_primary() {
        let mut record = placement(
            "right",
            NativeRect {
                left: 2307,
                top: -1000,
                width: 315,
                height: 171,
            },
            2,
        );
        record.scale_factor = 1.5;

        let resolved =
            resolve_placement(&[record], Some("right"), &[primary_display()]).expect("a placement");

        assert_eq!(resolved.display_uuid, "primary");
        assert!(!resolved.exact);
        // Rescaled from 150% to 100% and pulled back inside the primary work area.
        assert_eq!(
            resolved.rect,
            NativeRect {
                left: 1710,
                top: 0,
                width: 210,
                height: 114,
            }
        );
    }

    #[test]
    fn replugging_the_preferred_display_beats_a_newer_primary_record() {
        // The user nudged the sticker around on the primary monitor while `right` was unplugged,
        // so the primary record is newer, but the preference still points at `right`.
        let placements = [
            placement("primary", rect(400, 400), 9),
            placement("right", rect(2307, -1000), 2),
        ];
        let resolved = resolve_placement(
            &placements,
            Some("right"),
            &[primary_display(), right_display()],
        )
        .expect("a placement");

        assert_eq!(resolved.display_uuid, "right");
        assert_eq!(resolved.rect, rect(2307, -1000));
    }

    #[test]
    fn without_a_preference_the_primary_record_is_used() {
        let placements = [
            placement("primary", rect(400, 400), 9),
            placement("right", rect(2307, -1000), 2),
        ];
        let resolved = resolve_placement(&placements, None, &[primary_display(), right_display()])
            .expect("a placement");

        assert_eq!(resolved.display_uuid, "primary");
        assert_eq!(resolved.rect, rect(400, 400));
    }

    #[test]
    fn a_record_from_a_shrunken_monitor_is_clamped_back_into_view() {
        let placements = [placement("primary", rect(1800, 1000), 1)];
        let resolved =
            resolve_placement(&placements, None, &[primary_display()]).expect("a placement");

        assert_eq!(resolved.rect, rect(1710, 926));
        assert!(resolved.exact);
    }

    #[test]
    fn nothing_is_resolved_without_displays_or_records() {
        assert!(resolve_placement(&[], Some("right"), &[primary_display()]).is_none());
        assert!(resolve_placement(&[placement("primary", rect(0, 0), 1)], None, &[]).is_none());
    }

    #[test]
    fn moving_to_a_higher_dpi_monitor_grows_the_pixel_size() {
        assert_eq!(
            rect(100, 200).scaled(1.0, 1.5),
            NativeRect {
                left: 100,
                top: 200,
                width: 315,
                height: 171,
            }
        );
    }

    #[test]
    fn moving_to_a_lower_dpi_monitor_shrinks_the_pixel_size() {
        let big = NativeRect {
            left: 0,
            top: 0,
            width: 315,
            height: 171,
        };
        assert_eq!(big.scaled(1.5, 1.0), rect(0, 0));
    }

    #[test]
    fn equal_dpi_monitors_keep_the_pixel_size() {
        assert_eq!(rect(5, 6).scaled(1.25, 1.25), rect(5, 6));
    }

    #[test]
    fn invalid_scale_factors_leave_the_rect_untouched() {
        assert_eq!(rect(5, 6).scaled(0.0, 1.5), rect(5, 6));
        assert_eq!(rect(5, 6).scaled(1.5, 0.0), rect(5, 6));
    }

    #[test]
    fn position_on_an_unplugged_right_monitor_moves_to_the_primary_edge() {
        assert_eq!(
            clamp_into_work_area((2307, 300), (210, 114), PRIMARY_AREA),
            (1710, 300)
        );
    }

    #[test]
    fn position_above_the_primary_monitor_is_pulled_down() {
        assert_eq!(
            clamp_into_work_area((400, -1217), (210, 114), PRIMARY_AREA),
            (400, 0)
        );
    }

    #[test]
    fn windows_larger_than_the_work_area_align_to_its_origin() {
        assert_eq!(
            clamp_into_work_area((2307, -1217), (3000, 2000), PRIMARY_AREA),
            (0, 0)
        );
    }
}
