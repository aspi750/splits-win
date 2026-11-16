/// Catch-up window & notification deduplication (`CONTRACT.md §6.2`).
pub fn is_within_catch_up_window(hour: u32) -> bool {
    (4..18).contains(&hour)
}

pub fn should_catch_up_briefing(
    hour: u32,
    has_briefing_text: bool,
    had_sleep: bool,
    sleep_synced: bool,
) -> bool {
    if !is_within_catch_up_window(hour) || !sleep_synced {
        return false;
    }
    !has_briefing_text || !had_sleep
}

pub fn should_send_morning_notification(
    hour: u32,
    has_briefing_text: bool,
    had_sleep: bool,
    notify_enabled: bool,
    last_notified_date: Option<&str>,
    today_date: &str,
) -> bool {
    is_within_catch_up_window(hour)
        && has_briefing_text
        && had_sleep
        && notify_enabled
        && last_notified_date != Some(today_date)
}
