//! Bounded snapping geometry and reversible previews. This module does not
//! imply a multi-monitor compositor or persistent layout service.
use crate::desktop::Bounds;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Left,
    Right,
    Maximize,
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
        Zone::Maximize => area,
    };
    if result.width < minimum.width || result.height < minimum.height {
        return Err(Error::MinimumDoesNotFit);
    }
    Ok(result)
}
/// Pointer zones are finite and inside this work area; unrelated monitor
/// positions cannot accidentally trigger a snap. Side edges precede the top.
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
    let edge = edge.min(area.width / 2).min(area.height);
    if x - area.x < edge {
        Some(Zone::Left)
    } else if area.x + area.width - x <= edge {
        Some(Zone::Right)
    } else if y - area.y < edge {
        Some(Zone::Maximize)
    } else {
        None
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
        assert_eq!(pointer_zone(area, 500, 68, 24), Some(Zone::Maximize));
        for (x, y) in [(7, 100), (1008, 100), (500, 67), (500, 668), (500, 200)] {
            assert_eq!(pointer_zone(area, x, y, 24), None);
        }
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
                for zone in [Zone::Left, Zone::Right, Zone::Maximize] {
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
