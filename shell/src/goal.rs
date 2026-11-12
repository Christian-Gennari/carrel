//! The daily goal as a fraction, for the hairline along the bottom edge
//! (spec/03-ui.md). Pure arithmetic so it can be tested without GTK.

/// How far today's words have filled the line, from 0.0 to 1.0.
///
/// The line is full but never over-full at the goal, and a goal of zero
/// (which `Settings::normalize` prevents) simply draws nothing.
pub fn hairline_fraction(today_words: u64, daily_goal: u32) -> f64 {
    if daily_goal == 0 {
        return 0.0;
    }
    (today_words as f64 / f64::from(daily_goal)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_follows_the_goal() {
        assert_eq!(hairline_fraction(0, 500), 0.0);
        assert_eq!(hairline_fraction(250, 500), 0.5);
        assert_eq!(hairline_fraction(500, 500), 1.0);
        assert_eq!(hairline_fraction(999, 500), 1.0);
        assert_eq!(hairline_fraction(0, 0), 0.0);
    }

    #[test]
    fn one_word_over_the_goal_is_still_full_not_over_full() {
        assert!((hairline_fraction(501, 500) - 1.0).abs() < f64::EPSILON);
    }
}
