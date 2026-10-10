//! Printed circuit boards: footprints made from parameters, boards linked to their
//! schematics, copper geometry, connectivity and ratsnest, copper pours, design rule
//! checks, and fabrication outputs (Gerber, Excellon, positions, bill of materials).

pub mod board3d;
pub mod connect;
pub mod drc;
pub mod enclosure;
pub mod fab;
pub mod font;
pub mod footprints;
pub mod geometry;
pub mod kicad;
pub mod layout;
pub mod svg;
pub mod sync;
pub mod zones;

#[cfg(test)]
mod tests {
    #[test]
    fn polygon_booleans_work() {
        use i_overlay::core::fill_rule::FillRule;
        use i_overlay::core::overlay_rule::OverlayRule;
        use i_overlay::float::single::SingleFloatOverlay;
        let a = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        let b = [[2.0, 2.0], [6.0, 2.0], [6.0, 6.0], [2.0, 6.0]];
        let r = a.overlay(&b, OverlayRule::Intersect, FillRule::EvenOdd);
        assert_eq!(r.len(), 1);
    }
}
