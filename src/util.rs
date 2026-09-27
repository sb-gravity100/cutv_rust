pub fn fmt_tc(s: f64) -> String {
    let ms = ((s.fract()) * 1000.0).round() as u32;
    let si = s as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        si / 3600,
        (si % 3600) / 60,
        si % 60,
        ms
    )
}

/// Formats a playback/export speed multiplier for display, e.g. `1.0 ->
/// "1"`, `1.5 -> "1.5"`, `0.25 -> "0.25"` — no trailing zeros.
pub fn fmt_speed(s: f64) -> String {
    if s.fract() == 0.0 { format!("{}", s as i64) } else { format!("{s}") }
}
