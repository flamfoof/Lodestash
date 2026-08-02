//! Audio helpers with no renderer in them.
//!
//! Playback (`AudioPlayer`) stays in the UI crate: its rodio sink is `!Send` and belongs to
//! whatever owns the window. Only the pure parts live here.

/// `mm:ss` for the transport readout.
pub fn fmt_time(secs: f32) -> String {
    let secs = secs.max(0.0) as u32;
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_time_is_mmss() {
        assert_eq!(fmt_time(0.0), "0:00");
        assert_eq!(fmt_time(9.7), "0:09");
        assert_eq!(fmt_time(75.0), "1:15");
        assert_eq!(fmt_time(-3.0), "0:00");
    }
}
