//! Millisecond time input, while keeping persisted values compatible with f64 seconds.

pub const MAX_SECONDS: f64 = 864_000.0;
const MAX_MILLISECONDS: u64 = 864_000_000;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TimeFormat {
    Clock,
    Seconds,
}

/// Round to the nearest millisecond, carrying into the next second/minute/hour.
/// Invalid or negative values display as zero; finite values above 240 hours clamp.
pub fn format(seconds: f64, mode: TimeFormat) -> String {
    let seconds = if seconds.is_finite() {
        seconds.clamp(0.0, MAX_SECONDS)
    } else {
        0.0
    };
    let milliseconds = (seconds * 1000.0).round() as u64;
    let seconds = milliseconds / 1000;
    let fraction = milliseconds % 1000;
    match mode {
        TimeFormat::Clock => format!(
            "{:02}:{:02}:{:02}:{fraction:03}",
            seconds / 3600,
            (seconds / 60) % 60,
            seconds % 60,
        ),
        TimeFormat::Seconds => format!("{seconds:02}:{fraction:03}"),
    }
}

fn integer(field: &str) -> Option<u64> {
    if field.is_empty() || !field.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    field.parse().ok()
}

/// Parse HH:MM:SS:mmm or total-seconds:mmm, up to exactly 240:00:00:000.
/// One to three millisecond digits are an integer count: `:5` means 5 ms.
/// Surrounding whitespace is ignored; whitespace inside a field is rejected.
pub fn parse(text: &str, mode: TimeFormat) -> Option<f64> {
    let fields: Vec<_> = text.trim().split(':').collect();
    let (seconds, fraction) = match (mode, fields.as_slice()) {
        (TimeFormat::Clock, [hours, minutes, seconds, fraction]) => {
            let (hours, minutes, seconds) = (integer(hours)?, integer(minutes)?, integer(seconds)?);
            if minutes > 59 || seconds > 59 {
                return None;
            }
            let seconds = hours
                .checked_mul(3600)?
                .checked_add(minutes.checked_mul(60)?)?
                .checked_add(seconds)?;
            (seconds, *fraction)
        }
        (TimeFormat::Seconds, [seconds, fraction]) => (integer(seconds)?, *fraction),
        _ => return None,
    };
    if !(1..=3).contains(&fraction.len()) {
        return None;
    }
    let milliseconds = seconds.checked_mul(1000)?.checked_add(integer(fraction)?)?;
    if milliseconds > MAX_MILLISECONDS {
        return None;
    }
    Some(milliseconds as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_zero_one_millisecond_and_unbounded_total_seconds() {
        assert_eq!(format(0.0, TimeFormat::Clock), "00:00:00:000");
        assert_eq!(format(0.001, TimeFormat::Clock), "00:00:00:001");
        assert_eq!(format(0.001, TimeFormat::Seconds), "00:001");
        assert_eq!(format(61.005, TimeFormat::Seconds), "61:005");
        assert_eq!(format(3601.123, TimeFormat::Seconds), "3601:123");
        assert_eq!(format(360_001.123, TimeFormat::Clock), "100:00:01:123");
    }

    #[test]
    fn rounds_with_carry_and_without_float_tails() {
        assert_eq!(format(59.9996, TimeFormat::Clock), "00:01:00:000");
        assert_eq!(format(3599.9996, TimeFormat::Clock), "01:00:00:000");
        assert_eq!(format(59.9996, TimeFormat::Seconds), "60:000");
        assert_eq!(format(1.0004, TimeFormat::Seconds), "01:000");
        assert_eq!(format(1.0006, TimeFormat::Seconds), "01:001");
        assert_eq!(format(0.1 + 0.2, TimeFormat::Seconds), "00:300");
    }

    #[test]
    fn milliseconds_are_integer_counts_and_modes_round_trip() {
        assert_eq!(parse("00:00:00:1", TimeFormat::Clock), Some(0.001));
        assert_eq!(parse("00:00:00:10", TimeFormat::Clock), Some(0.010));
        assert_eq!(parse("00:00:00:100", TimeFormat::Clock), Some(0.100));
        assert_eq!(parse("61:5", TimeFormat::Seconds), Some(61.005));
        assert_eq!(parse(" 01:02:03:004 ", TimeFormat::Clock), Some(3723.004));
        for milliseconds in [
            0_u64,
            1,
            5,
            10,
            999,
            1000,
            60_001,
            3_599_999,
            360_000_001,
            MAX_MILLISECONDS,
        ] {
            let value = milliseconds as f64 / 1000.0;
            for mode in [TimeFormat::Clock, TimeFormat::Seconds] {
                assert_eq!(parse(&format(value, mode), mode), Some(value));
            }
        }
    }

    #[test]
    fn enforces_exact_maximum_and_safe_display_fallback() {
        assert_eq!(parse("240:00:00:000", TimeFormat::Clock), Some(MAX_SECONDS));
        assert_eq!(parse("864000:000", TimeFormat::Seconds), Some(MAX_SECONDS));
        assert_eq!(
            format(MAX_SECONDS - 0.0004, TimeFormat::Clock),
            "240:00:00:000"
        );
        assert!(parse("240:00:00:001", TimeFormat::Clock).is_none());
        assert!(parse("864000:001", TimeFormat::Seconds).is_none());
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            assert_eq!(format(invalid, TimeFormat::Clock), "00:00:00:000");
        }
        assert_eq!(format(MAX_SECONDS + 1.0, TimeFormat::Seconds), "864000:000");
    }

    #[test]
    fn rejects_invalid_fields_decimals_wrong_modes_and_overflow() {
        for invalid in [
            "",
            " ",
            "00:00:00",
            "00:00:00:000:0",
            "00:60:00:000",
            "00:00:60:000",
            "00:00:00:1000",
            "00:00:00:",
            "00:00:00:0000",
            "00:00:00:0.5",
            "00:00:00:-1",
            "-1:00:00:000",
            "+1:00:00:000",
            "NaN:00:00:000",
            "00: 01:00:000",
            "００:00:00:001",
            "18446744073709551615:00:00:000",
            "18446744073709551616:00:00:000",
            "241:00:00:000",
        ] {
            assert!(parse(invalid, TimeFormat::Clock).is_none(), "{invalid}");
        }
        for invalid in [
            "",
            "00",
            "00:00:000",
            "-1:000",
            "+1:000",
            "1.5:000",
            "1:0.5",
            "1:1000",
            "1:0000",
            "NaN:000",
            "inf:000",
            "18446744073709551615:000",
            "864001:000",
        ] {
            assert!(parse(invalid, TimeFormat::Seconds).is_none(), "{invalid}");
        }
        assert!(parse("00:00:00:001", TimeFormat::Seconds).is_none());
        assert!(parse("61:001", TimeFormat::Clock).is_none());
    }
}
