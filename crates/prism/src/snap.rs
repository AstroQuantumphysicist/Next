//! Bounded snapping geometry and reversible previews. This module does not
//! imply a multi-monitor compositor or persistent layout service.
use crate::desktop::Bounds;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Maximize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Minimum {
    pub width: i32,
    pub height: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidGeometry,
    MinimumDoesNotFit,
}
/// Move through the bounded snap layout from an active keyboard preview.
/// Missing neighbors intentionally leave the current preview untouched.
pub const fn adjacent(zone: Zone, direction: Direction) -> Option<Zone> {
    match (zone, direction) {
        (Zone::Left, Direction::Up) => Some(Zone::TopLeft),
        (Zone::Left, Direction::Down) => Some(Zone::BottomLeft),
        (Zone::Left, Direction::Right) => Some(Zone::Right),
        (Zone::Right, Direction::Up) => Some(Zone::TopRight),
        (Zone::Right, Direction::Down) => Some(Zone::BottomRight),
        (Zone::Right, Direction::Left) => Some(Zone::Left),
        (Zone::TopLeft, Direction::Up) => Some(Zone::Maximize),
        (Zone::TopLeft, Direction::Right) => Some(Zone::TopRight),
        (Zone::TopLeft, Direction::Down) => Some(Zone::BottomLeft),
        (Zone::TopLeft, Direction::Left) => Some(Zone::Left),
        (Zone::TopRight, Direction::Up) => Some(Zone::Maximize),
        (Zone::TopRight, Direction::Right) => Some(Zone::Right),
        (Zone::TopRight, Direction::Down) => Some(Zone::BottomRight),
        (Zone::TopRight, Direction::Left) => Some(Zone::TopLeft),
        (Zone::BottomLeft, Direction::Up) => Some(Zone::TopLeft),
        (Zone::BottomLeft, Direction::Right) => Some(Zone::BottomRight),
        (Zone::BottomLeft, Direction::Left) => Some(Zone::Left),
        (Zone::BottomRight, Direction::Up) => Some(Zone::TopRight),
        (Zone::BottomRight, Direction::Right) => Some(Zone::Right),
        (Zone::BottomRight, Direction::Left) => Some(Zone::BottomLeft),
        (Zone::Maximize, Direction::Left) => Some(Zone::Left),
        (Zone::Maximize, Direction::Right) => Some(Zone::Right),
        _ => None,
    }
}
fn valid(area: Bounds) -> bool {
    area.width > 0
        && area.height > 0
        && area.x.checked_add(area.width).is_some()
        && area.y.checked_add(area.height).is_some()
}
/// Split odd widths without gaps or overlap, keeping each result entirely in
/// the supplied work area. An impossible minimum is reported rather than
/// shrinking a window below its hint or positioning it beyond the display.
pub fn target(area: Bounds, minimum: Minimum, zone: Zone) -> Result<Bounds, Error> {
    if !valid(area) || minimum.width <= 0 || minimum.height <= 0 {
        return Err(Error::InvalidGeometry);
    }
    let split = area.width / 2;
    let split_y = area.height / 2;
    let result = match zone {
        Zone::Left => Bounds {
            width: split,
            ..area
        },
        Zone::Right => Bounds {
            x: area.x + split,
            width: area.width - split,
            ..area
        },
        Zone::TopLeft => Bounds {
            width: split,
            height: split_y,
            ..area
        },
        Zone::TopRight => Bounds {
            x: area.x + split,
            width: area.width - split,
            height: split_y,
            ..area
        },
        Zone::BottomLeft => Bounds {
            y: area.y + split_y,
            width: split,
            height: area.height - split_y,
            ..area
        },
        Zone::BottomRight => Bounds {
            x: area.x + split,
            y: area.y + split_y,
            width: area.width - split,
            height: area.height - split_y,
        },
        Zone::Maximize => area,
    };
    if result.width < minimum.width || result.height < minimum.height {
        return Err(Error::MinimumDoesNotFit);
    }
    Ok(result)
}
/// Pointer zones are finite and inside this work area; unrelated monitor
/// positions cannot accidentally trigger a snap. Corners take priority, then
/// side edges and the top-center maximize zone.
pub fn pointer_zone(area: Bounds, x: i32, y: i32, edge: i32) -> Option<Zone> {
    if !valid(area)
        || edge <= 0
        || x < area.x
        || y < area.y
        || x >= area.x + area.width
        || y >= area.y + area.height
    {
        return None;
    }
    let edge_x = edge.min((area.width / 2).max(1));
    let edge_y = edge.min((area.height / 2).max(1));
    let left = x - area.x < edge_x;
    let right = area.x + area.width - x <= edge_x;
    let top = y - area.y < edge_y;
    let bottom = area.y + area.height - y <= edge_y;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(Zone::TopLeft),
        (_, true, true, _) => Some(Zone::TopRight),
        (true, _, _, true) => Some(Zone::BottomLeft),
        (_, true, _, true) => Some(Zone::BottomRight),
        (true, _, _, _) => Some(Zone::Left),
        (_, true, _, _) => Some(Zone::Right),
        (_, _, true, _) => Some(Zone::Maximize),
        _ => None,
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Preview {
    original: Bounds,
    pending: Bounds,
    area: Bounds,
    minimum: Minimum,
    zone: Zone,
}
impl Preview {
    pub fn new(
        area: Bounds,
        original: Bounds,
        minimum: Minimum,
        zone: Zone,
    ) -> Result<Self, Error> {
        if !valid(original) {
            return Err(Error::InvalidGeometry);
        }
        Ok(Self {
            original,
            pending: target(area, minimum, zone)?,
            area,
            minimum,
            zone,
        })
    }
    pub fn bounds(&self) -> Bounds {
        self.pending
    }
    pub fn zone(&self) -> Zone {
        self.zone
    }
    /// A failed preview leaves the previously valid choice unchanged.
    pub fn select(&mut self, zone: Zone) -> Result<(), Error> {
        let pending = target(self.area, self.minimum, zone)?;
        self.pending = pending;
        self.zone = zone;
        Ok(())
    }
    pub fn cancel(self) -> Bounds {
        self.original
    }
    pub fn commit(self) -> Bounds {
        self.pending
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const MIN: Minimum = Minimum {
        width: 240,
        height: 144,
    };
    #[test]
    fn odd_width_split_is_complete_disjoint_and_inside_work_area() {
        let area = Bounds {
            x: 8,
            y: 68,
            width: 1009,
            height: 652,
        };
        let left = target(area, MIN, Zone::Left).unwrap();
        let right = target(area, MIN, Zone::Right).unwrap();
        assert_eq!(left.width + right.width, area.width);
        assert_eq!(left.x + left.width, right.x);
        assert_eq!(right.x + right.width, area.x + area.width);
        assert_eq!(left.y, area.y);
        assert_eq!(left.height, area.height);
        assert_eq!(target(area, MIN, Zone::Maximize), Ok(area));
    }
    #[test]
    fn odd_quadrants_tile_the_work_area_without_gaps_or_overlap() {
        let area = Bounds {
            x: 13,
            y: 29,
            width: 1009,
            height: 653,
        };
        let zones = [
            Zone::TopLeft,
            Zone::TopRight,
            Zone::BottomLeft,
            Zone::BottomRight,
        ];
        let [tl, tr, bl, br] = zones.map(|zone| target(area, MIN, zone).unwrap());
        assert_eq!(tl.x, area.x);
        assert_eq!(tl.y, area.y);
        assert_eq!(tl.x + tl.width, tr.x);
        assert_eq!(tr.y, tl.y);
        assert_eq!(bl.x, tl.x);
        assert_eq!(bl.y, tl.y + tl.height);
        assert_eq!(br.x, tr.x);
        assert_eq!(br.y, bl.y);
        assert_eq!(tr.x + tr.width, area.x + area.width);
        assert_eq!(bl.y + bl.height, area.y + area.height);
        assert_eq!(tl.width + tr.width, area.width);
        assert_eq!(tl.height + bl.height, area.height);
    }
    #[test]
    fn minimum_and_overflow_fail_without_manufacturing_offscreen_bounds() {
        assert_eq!(
            target(
                Bounds {
                    x: 0,
                    y: 0,
                    width: 479,
                    height: 144
                },
                MIN,
                Zone::Left
            ),
            Err(Error::MinimumDoesNotFit)
        );
        assert!(target(
            Bounds {
                x: 0,
                y: 0,
                width: 479,
                height: 144
            },
            MIN,
            Zone::Right
        )
        .is_ok());
        let quadrant_area = Bounds {
            x: 0,
            y: 0,
            width: 480,
            height: 288,
        };
        for zone in [
            Zone::TopLeft,
            Zone::TopRight,
            Zone::BottomLeft,
            Zone::BottomRight,
        ] {
            assert!(target(quadrant_area, MIN, zone).is_ok());
        }
        assert_eq!(
            target(
                Bounds {
                    width: 480,
                    height: 287,
                    ..quadrant_area
                },
                MIN,
                Zone::TopLeft
            ),
            Err(Error::MinimumDoesNotFit)
        );
        assert!(target(
            Bounds {
                width: 480,
                height: 287,
                ..quadrant_area
            },
            MIN,
            Zone::BottomLeft
        )
        .is_ok());
        assert_eq!(
            target(
                Bounds {
                    width: 479,
                    ..quadrant_area
                },
                MIN,
                Zone::TopLeft
            ),
            Err(Error::MinimumDoesNotFit)
        );
        assert!(target(
            Bounds {
                width: 479,
                ..quadrant_area
            },
            MIN,
            Zone::TopRight
        )
        .is_ok());
        for area in [
            Bounds {
                x: i32::MAX,
                y: 0,
                width: 1,
                height: 144,
            },
            Bounds {
                x: 0,
                y: 0,
                width: 640,
                height: 0,
            },
        ] {
            assert_eq!(
                target(area, MIN, Zone::Maximize),
                Err(Error::InvalidGeometry)
            );
        }
    }
    #[test]
    fn preview_cancel_preserves_restore_and_failed_selection_keeps_last_preview() {
        let area = Bounds {
            x: 8,
            y: 68,
            width: 479,
            height: 400,
        };
        let original = Bounds {
            x: 32,
            y: 96,
            width: 240,
            height: 240,
        };
        let mut preview = Preview::new(area, original, MIN, Zone::Maximize).unwrap();
        assert_eq!(preview.select(Zone::Left), Err(Error::MinimumDoesNotFit));
        assert_eq!(preview.bounds(), area);
        assert_eq!(preview.zone(), Zone::Maximize);
        assert_eq!(preview.cancel(), original);
        preview.select(Zone::Right).unwrap();
        assert_eq!(preview.commit().width, 240);
    }
    #[test]
    fn pointer_edges_never_trigger_outside_or_in_center() {
        let area = Bounds {
            x: 8,
            y: 68,
            width: 1000,
            height: 600,
        };
        assert_eq!(pointer_zone(area, 8, 100, 24), Some(Zone::Left));
        assert_eq!(pointer_zone(area, 1007, 100, 24), Some(Zone::Right));
        assert_eq!(pointer_zone(area, 8, 68, 24), Some(Zone::TopLeft));
        assert_eq!(pointer_zone(area, 1007, 68, 24), Some(Zone::TopRight));
        assert_eq!(pointer_zone(area, 8, 667, 24), Some(Zone::BottomLeft));
        assert_eq!(pointer_zone(area, 1007, 667, 24), Some(Zone::BottomRight));
        assert_eq!(pointer_zone(area, 500, 68, 24), Some(Zone::Maximize));
        for (x, y) in [(7, 100), (1008, 100), (500, 67), (500, 668), (500, 200)] {
            assert_eq!(pointer_zone(area, x, y, 24), None);
        }
    }
    #[test]
    fn keyboard_neighbors_cover_quadrants_without_wrapping() {
        assert_eq!(adjacent(Zone::Left, Direction::Up), Some(Zone::TopLeft));
        assert_eq!(
            adjacent(Zone::TopLeft, Direction::Right),
            Some(Zone::TopRight)
        );
        assert_eq!(
            adjacent(Zone::TopRight, Direction::Down),
            Some(Zone::BottomRight)
        );
        assert_eq!(
            adjacent(Zone::BottomRight, Direction::Left),
            Some(Zone::BottomLeft)
        );
        assert_eq!(adjacent(Zone::BottomLeft, Direction::Down), None);
    }
    #[test]
    fn successful_geometries_obey_minimum_and_containment_across_sizes() {
        for width in [1, 239, 240, 479, 480, 481, 800, 1921] {
            for height in [1, 143, 144, 400, 1080] {
                let area = Bounds {
                    x: -40,
                    y: 20,
                    width,
                    height,
                };
                for zone in [
                    Zone::Left,
                    Zone::Right,
                    Zone::TopLeft,
                    Zone::TopRight,
                    Zone::BottomLeft,
                    Zone::BottomRight,
                    Zone::Maximize,
                ] {
                    if let Ok(b) = target(area, MIN, zone) {
                        assert!(b.width >= MIN.width && b.height >= MIN.height);
                        assert!(b.x >= area.x && b.y >= area.y);
                        assert!(
                            b.x + b.width <= area.x + width && b.y + b.height <= area.y + height
                        );
                    }
                }
            }
        }
    }
}
