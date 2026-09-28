//! Typed configuration for synthetic GPU-loop acceptance input.
//!
//! Production NSEvent dispatch does not depend on this module. Keeping test
//! environment parsing out of the frame loop makes the owner-thread lifecycle
//! readable and lets acceptance syntax evolve without recompiling that logic.

#[derive(Debug, Default)]
pub(super) struct GpuTestConfig {
    pub(super) resize: Option<(u32, u32)>,
    pub(super) resize_after_ms: Option<u64>,
    pub(super) pointer: Option<(f32, f32)>,
    pub(super) drag: Option<Vec<(f32, f32)>>,
    pub(super) post_sequence_drag: Option<Vec<(f32, f32)>>,
    pub(super) tap_sequence: Option<Vec<(f32, f32, u64)>>,
    pub(super) pointer_sequence_post_delay_ms: u64,
    pub(super) post_drag_tap_sequence: Option<Vec<(f32, f32, u64)>>,
    pub(super) hold_ms: u64,
    pub(super) cancel: bool,
    pub(super) pointer_hz: Option<u32>,
    pub(super) key_sequence: Option<Vec<(u32, u32)>>,
    pub(super) post_pointer_key_sequence: Option<Vec<(u32, u32)>>,
    pub(super) post_pointer_key_delay_ms: u64,
}

impl GpuTestConfig {
    pub(super) fn from_env() -> Self {
        let resize = std::env::var("AIM_TEST_WINDOW_RESIZE")
            .ok()
            .and_then(|value| parse_resize(&value, 'x').or_else(|| parse_resize(&value, ',')));
        let resize_after_ms = parse_env("AIM_TEST_WINDOW_RESIZE_AFTER_MS");
        let explicit_pointer = std::env::var("AIM_TEST_POINTER_CLICK")
            .ok()
            .and_then(|value| parse_pair(&value, ','));
        let drag = std::env::var("AIM_TEST_POINTER_DRAG")
            .ok()
            .map(|value| parse_drag(&value))
            .filter(|points| points.len() >= 2);
        let post_sequence_drag = std::env::var("AIM_TEST_POINTER_AFTER_SEQUENCE_DRAG")
            .ok()
            .map(|value| parse_drag(&value))
            .filter(|points| points.len() >= 2);
        let tap_sequence = std::env::var("AIM_TEST_POINTER_SEQUENCE")
            .ok()
            .map(|value| parse_taps(&value))
            .filter(|samples| !samples.is_empty());
        let post_drag_tap_sequence = std::env::var("AIM_TEST_POINTER_AFTER_DRAG_SEQUENCE")
            .ok()
            .map(|value| parse_taps(&value))
            .filter(|samples| !samples.is_empty());
        let pointer = drag
            .as_ref()
            .and_then(|points| points.first().copied())
            .or_else(|| {
                tap_sequence
                    .as_ref()
                    .and_then(|samples| samples.first().map(|&(x, y, _)| (x, y)))
            })
            .or(explicit_pointer);
        let key_sequence = std::env::var("AIM_TEST_KEY_SEQUENCE")
            .ok()
            .map(|value| parse_keys(&value))
            .filter(|sequence| !sequence.is_empty())
            .or_else(|| parse_env("AIM_TEST_KEY_CODE").map(|code| vec![(code, 0)]));
        let post_pointer_key_sequence = std::env::var("AIM_TEST_KEY_AFTER_POINTER_SEQUENCE")
            .ok()
            .map(|value| parse_keys(&value))
            .filter(|sequence| !sequence.is_empty());
        Self {
            resize,
            resize_after_ms,
            pointer,
            drag,
            post_sequence_drag,
            tap_sequence,
            pointer_sequence_post_delay_ms: parse_env("AIM_TEST_POINTER_SEQUENCE_POST_DELAY_MS")
                .unwrap_or(0),
            post_drag_tap_sequence,
            hold_ms: parse_env("AIM_TEST_POINTER_HOLD_MS").unwrap_or(0),
            cancel: std::env::var("AIM_TEST_POINTER_CANCEL")
                .ok()
                .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
            pointer_hz: parse_env("AIM_TEST_POINTER_HZ").filter(|hz| *hz > 0),
            key_sequence,
            post_pointer_key_sequence,
            post_pointer_key_delay_ms: parse_env("AIM_TEST_KEY_AFTER_POINTER_DELAY_MS")
                .unwrap_or(0),
        }
    }

    pub(super) fn standalone_pointer_replay(&self) -> bool {
        self.drag.is_none()
            && self.tap_sequence.is_none()
            && self.post_sequence_drag.is_none()
            && self.post_drag_tap_sequence.is_none()
    }
}

fn parse_env<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok()?.parse().ok()
}

fn parse_pair(value: &str, separator: char) -> Option<(f32, f32)> {
    let (x, y) = value.split_once(separator)?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

fn parse_resize(value: &str, separator: char) -> Option<(u32, u32)> {
    let (width, height) = value.split_once(separator)?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

fn parse_drag(value: &str) -> Vec<(f32, f32)> {
    value
        .split(';')
        .filter_map(|sample| parse_pair(sample, ','))
        .collect()
}

fn parse_taps(value: &str) -> Vec<(f32, f32, u64)> {
    value
        .split(';')
        .filter_map(|sample| {
            let mut fields = sample.split(',');
            Some((
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
            ))
        })
        .collect()
}

fn parse_keys(value: &str) -> Vec<(u32, u32)> {
    value
        .split(',')
        .filter_map(|sample| {
            let (code, meta) = sample.split_once(':').unwrap_or((sample, "0"));
            Some((code.parse().ok()?, meta.parse().ok()?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceptance_input_syntax_is_typed_and_fail_closed() {
        assert_eq!(parse_pair("180,320", ','), Some((180.0, 320.0)));
        assert_eq!(parse_pair("bad,320", ','), None);
        assert_eq!(
            parse_taps("0,0,0;180,140,10000"),
            vec![(0.0, 0.0, 0), (180.0, 140.0, 10_000)]
        );
        assert_eq!(parse_keys("36,33:1"), vec![(36, 0), (33, 1)]);
    }
}
