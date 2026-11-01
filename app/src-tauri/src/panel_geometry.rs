//! Panel geometry in logical desktop coordinates, independent of AppKit.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub fn below_tray(tray: Bounds, preferred: (f64, f64), work_area: Bounds) -> Bounds {
    let margin = 4.0_f64
        .min(work_area.width / 2.0)
        .min(work_area.height / 2.0);
    let width = preferred.0.min(work_area.width - 2.0 * margin);
    let height = preferred.1.min(work_area.height - 2.0 * margin);
    let left = work_area.x + margin;
    let top = work_area.y + margin;
    Bounds {
        x: (tray.x + tray.width / 2.0 - width / 2.0).clamp(
            left,
            (work_area.x + work_area.width - margin - width).max(left),
        ),
        y: (tray.y + tray.height + margin).clamp(
            top,
            (work_area.y + work_area.height - margin - height).max(top),
        ),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: f64, y: f64, width: f64, height: f64) -> Bounds {
        Bounds {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_panel_with_room_remains_centered_under_its_tray_icon() {
        assert_eq!(
            below_tray(
                bounds(900.0, 0.0, 24.0, 24.0),
                (400.0, 720.0),
                bounds(0.0, 24.0, 1440.0, 856.0)
            ),
            bounds(712.0, 28.0, 400.0, 720.0),
        );
    }

    #[test]
    fn screen_edges_leave_the_entire_panel_accessible() {
        let work = bounds(0.0, 24.0, 1440.0, 856.0);
        assert_eq!(
            below_tray(bounds(2.0, 0.0, 24.0, 24.0), (400.0, 720.0), work).x,
            4.0
        );
        assert_eq!(
            below_tray(bounds(1414.0, 0.0, 24.0, 24.0), (400.0, 720.0), work).x,
            1036.0
        );
    }

    #[test]
    fn a_short_display_keeps_the_footer_above_the_dock() {
        assert_eq!(
            below_tray(
                bounds(900.0, 0.0, 24.0, 24.0),
                (400.0, 720.0),
                bounds(0.0, 24.0, 1440.0, 680.0)
            ),
            bounds(712.0, 28.0, 400.0, 672.0),
        );
    }

    #[test]
    fn a_left_hand_display_uses_its_negative_origin_and_reserved_dock() {
        assert_eq!(
            below_tray(
                bounds(-70.0, 0.0, 24.0, 24.0),
                (400.0, 720.0),
                bounds(-1230.0, 24.0, 1230.0, 776.0)
            ),
            bounds(-404.0, 28.0, 400.0, 720.0),
        );
    }

    #[test]
    fn a_display_above_the_main_screen_retains_its_own_top_edge() {
        assert_eq!(
            below_tray(
                bounds(900.0, -900.0, 24.0, 24.0),
                (400.0, 720.0),
                bounds(0.0, -876.0, 1440.0, 856.0)
            ),
            bounds(712.0, -872.0, 400.0, 720.0),
        );
    }

    #[test]
    fn a_narrow_work_area_fits_the_panel_without_invalid_clamp_bounds() {
        assert_eq!(
            below_tray(
                bounds(100.0, 0.0, 24.0, 24.0),
                (400.0, 720.0),
                bounds(0.0, 24.0, 350.0, 680.0)
            ),
            bounds(4.0, 28.0, 342.0, 672.0),
        );
    }
}
