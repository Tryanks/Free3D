//! User-facing length units; geometry remains millimetre-based.

use serde::{Deserialize, Serialize};

/// Length unit used for readouts and numeric dimension entry.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum Units {
    /// Millimetres (the internal model unit).
    #[default]
    Millimeter,
    /// Centimetres.
    Centimeter,
    /// Metres.
    Meter,
    /// International inches.
    Inch,
}

impl Units {
    /// Every unit in settings-menu order.
    pub const ALL: [Self; 4] = [Self::Millimeter, Self::Centimeter, Self::Meter, Self::Inch];

    /// Number of internal millimetres represented by one displayed unit.
    pub const fn millimeters_per_unit(self) -> f64 {
        match self {
            Self::Millimeter => 1.0,
            Self::Centimeter => 10.0,
            Self::Meter => 1_000.0,
            Self::Inch => 25.4,
        }
    }

    /// Converts an internal millimetre value for display.
    pub fn display_value(self, millimeters: f64) -> f64 {
        millimeters / self.millimeters_per_unit()
    }

    /// Converts square millimetres to the current squared display unit.
    pub fn display_area(self, square_millimeters: f64) -> f64 {
        square_millimeters / self.millimeters_per_unit().powi(2)
    }

    /// Converts cubic millimetres to the current cubed display unit.
    pub fn display_volume(self, cubic_millimeters: f64) -> f64 {
        cubic_millimeters / self.millimeters_per_unit().powi(3)
    }

    /// Converts unit-density geometric inertia (mm⁵) to display-unit⁵.
    pub fn display_inertia(self, millimeters_fifth: f64) -> f64 {
        millimeters_fifth / self.millimeters_per_unit().powi(5)
    }

    /// Converts a value entered in this unit to internal millimetres.
    pub fn parse_value(self, displayed: f64) -> f64 {
        displayed * self.millimeters_per_unit()
    }

    /// Short label used beside compact values.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Millimeter => crate::i18n::t("Millimeter"),
            Self::Centimeter => crate::i18n::t("Centimeter"),
            Self::Meter => crate::i18n::t("Meter"),
            Self::Inch => crate::i18n::t("Inch"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inch_roundtrip_uses_25_4_millimeters() {
        let displayed = Units::Inch.display_value(25.4);
        assert!((displayed - 1.0).abs() < f64::EPSILON);
        assert!((Units::Inch.parse_value(displayed) - 25.4).abs() < f64::EPSILON);
    }
}
