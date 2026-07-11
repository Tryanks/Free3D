//! Adaptive 1-2-5 grid spacing.

/// Selects a minor pitch whose apparent spacing remains useful at `distance`.
pub fn adaptive_pitch(distance: f32) -> f32 {
    let desired = (distance.max(0.001) / 18.0).max(0.001);
    let decade = 10.0_f32.powf(desired.log10().floor());
    let normalized = desired / decade;
    let step = if normalized < 2.0 {
        1.0
    } else if normalized < 5.0 {
        2.0
    } else {
        5.0
    };
    step * decade
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_uses_one_two_five_progression() {
        let samples = [18.0, 36.0, 90.0, 180.0, 360.0, 900.0];
        let pitches: Vec<_> = samples.into_iter().map(adaptive_pitch).collect();
        assert_eq!(pitches, vec![1.0, 2.0, 5.0, 10.0, 20.0, 50.0]);
    }
}
