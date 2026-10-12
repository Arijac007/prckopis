//! The unit lengths are shown in: page sizes (Document Properties, Print) and the margins typed
//! in Header & Footer and Set Page Boxes. Preferences ▸ Documents and view ▸ Units.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    Inches,
    #[default]
    Millimetres,
    Centimetres,
    Points,
}

impl Unit {
    pub const ALL: [Unit; 4] = [Unit::Millimetres, Unit::Centimetres, Unit::Inches, Unit::Points];

    /// Units per PDF point.
    pub fn per_point(self) -> f64 {
        match self {
            Unit::Points => 1.0,
            Unit::Inches => 1.0 / 72.0,
            Unit::Millimetres => 25.4 / 72.0,
            Unit::Centimetres => 2.54 / 72.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Unit::Points => "pt",
            Unit::Inches => "in",
            Unit::Millimetres => "mm",
            Unit::Centimetres => "cm",
        }
    }

    /// Decimals worth showing: a tenth of a millimetre or point, a hundredth of an inch or centimetre.
    pub fn decimals(self) -> usize {
        match self {
            Unit::Millimetres | Unit::Points => 1,
            Unit::Inches | Unit::Centimetres => 2,
        }
    }

    /// Drag speed for a value field in this unit.
    pub fn speed(self) -> f64 {
        match self {
            Unit::Inches | Unit::Centimetres => 0.01,
            Unit::Millimetres | Unit::Points => 0.5,
        }
    }

    /// The unit on first start, before the user picks one in Preferences: millimetres, whatever
    /// the system's region (Prckopis is set up for Europe).
    pub fn system() -> Unit {
        Unit::Millimetres
    }

    /// A page size given in points, e.g. "210.0 × 297.0 mm".
    pub fn size(self, width_pt: f64, height_pt: f64) -> String {
        let (k, d) = (self.per_point(), self.decimals());
        format!("{:.d$} × {:.d$} {}", width_pt * k, height_pt * k, self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::Unit;

    #[test]
    fn millimetres_until_the_user_picks_a_unit() {
        assert_eq!(Unit::system(), Unit::Millimetres);
        assert_eq!(Unit::default(), Unit::Millimetres);
    }

    #[test]
    fn a4_in_every_unit() {
        assert_eq!(Unit::Millimetres.size(595.276, 841.89), "210.0 × 297.0 mm");
        assert_eq!(Unit::Centimetres.size(595.276, 841.89), "21.00 × 29.70 cm");
        assert_eq!(Unit::Inches.size(612.0, 792.0), "8.50 × 11.00 in");
        assert_eq!(Unit::Points.size(612.0, 792.0), "612.0 × 792.0 pt");
    }

    #[test]
    fn saved_names_round_trip() {
        for u in Unit::ALL {
            let v = serde_json::to_value(u).unwrap();
            assert_eq!(serde_json::from_value::<Unit>(v).unwrap(), u);
        }
        assert_eq!(serde_json::to_value(Unit::Millimetres).unwrap(), "millimetres");
    }
}
