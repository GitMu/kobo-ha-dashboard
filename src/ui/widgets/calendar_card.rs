//! Upcoming events of several calendars in one list, grouped by day.

use crate::config::SensorItemConfig;
use crate::display::{Canvas, TextAlign};
use crate::ha::client::CALENDAR_ATTR;
use crate::ha::models::EntityState;
use crate::i18n;
use crate::ui::layout::{InteractiveTarget, Rect, UiAction};
use crate::ui::theme::Theme;
use crate::ui::widgets::{draw_card, type_scale, PAD};
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, Timelike};
use std::collections::HashMap;

pub const TITLE_H: f32 = 40.0;
const DAY_H: f32 = 42.0;
const ROW_H: f32 = 36.0;
/// Space between one day's last event and the next day
const DAY_GAP: f32 = 10.0;

/// Height (without the title) that lists `days` days of three events each
pub fn natural_height(days: u32) -> f32 {
    let days = days.clamp(1, crate::config::MAX_CALENDAR_DAYS).min(5) as f32;
    2.0 * PAD + days * (DAY_H + 3.0 * ROW_H) + (days - 1.0) * DAY_GAP + ROW_H
}

/// When an event happens on a given day
#[derive(Debug, Clone, PartialEq)]
pub enum When {
    AllDay,
    /// Starts at this time
    At(DateTime<FixedOffset>),
    /// Started before this day and ends at this time
    Until(DateTime<FixedOffset>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub when: When,
    pub summary: String,
    /// Index of the calendar in the card's list
    pub calendar: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Day {
    pub date: NaiveDate,
    pub entries: Vec<Entry>,
}

fn parse_time(s: &str, offset: FixedOffset) -> Option<DateTime<FixedOffset>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&offset));
    }
    // Without an offset: local time of Home Assistant, taken as ours
    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()?;
    naive.and_local_timezone(offset).single()
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()
}

/// The days from today to `days` days ahead that have something on, with
/// their events: all-day ones first, then by time. A timed event shows on
/// the day it starts, and on today while it is still going on; one that is
/// over is left out. An all-day event shows on each day it covers.
pub fn agenda(calendars: &[Option<&serde_json::Value>], now: DateTime<FixedOffset>, days: u32) -> Vec<Day> {
    let offset = *now.offset();
    let today = now.date_naive();
    let today_start = today.and_hms_opt(0, 0, 0).and_then(|t| t.and_local_timezone(offset).single()).unwrap_or(now);
    let mut out: Vec<Day> = (0..days.max(1))
        .map(|i| Day {
            date: today + Duration::days(i as i64),
            entries: Vec::new(),
        })
        .collect();
    let last = out.last().map(|d| d.date).unwrap_or(today);

    for (calendar, events) in calendars.iter().enumerate() {
        let Some(events) = events.and_then(|v| v.as_array()) else { continue };
        for e in events {
            let summary = e.get("summary").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let (Some(start), Some(end)) = (e.get("start").and_then(|v| v.as_str()), e.get("end").and_then(|v| v.as_str())) else {
                continue;
            };
            let all_day = e.get("all_day").and_then(|v| v.as_bool()).unwrap_or(false);
            if all_day {
                let Some(first) = parse_date(start) else { continue };
                // The end date is the day after the event
                let end = parse_date(end).filter(|d| *d > first).unwrap_or(first + Duration::days(1));
                for day in out.iter_mut().filter(|d| d.date >= first && d.date < end) {
                    day.entries.push(Entry {
                        when: When::AllDay,
                        summary: summary.clone(),
                        calendar,
                    });
                }
                continue;
            }
            let Some(start) = parse_time(start, offset) else { continue };
            let end = parse_time(end, offset).unwrap_or(start);
            // Over (an event without a length is over once it started)
            let over = if end > start { end <= now } else { start < now };
            if over {
                continue;
            }
            let (date, when) = if start < today_start {
                (today, When::Until(end))
            } else {
                (start.date_naive(), When::At(start))
            };
            if date > last {
                continue;
            }
            if let Some(day) = out.iter_mut().find(|d| d.date == date) {
                day.entries.push(Entry { when, summary, calendar });
            }
        }
    }

    for day in out.iter_mut() {
        day.entries.sort_by(|a, b| {
            let key = |e: &Entry| match &e.when {
                When::AllDay => (0, 0, 0),
                When::Until(t) => (1, t.hour() * 60 + t.minute(), 0),
                When::At(t) => (2, t.hour() * 60 + t.minute(), 0),
            };
            key(a).cmp(&key(b)).then_with(|| a.summary.cmp(&b.summary))
        });
        // The same event in two calendars (shared ones) once
        day.entries.dedup_by(|a, b| a.when == b.when && a.summary == b.summary);
    }
    out.retain(|d| !d.entries.is_empty());
    out
}

/// "Today", "Tomorrow", or the weekday and date
fn day_label(date: NaiveDate, today: NaiveDate) -> String {
    if date == today {
        return i18n::t("date.today");
    }
    if date == today + Duration::days(1) {
        return i18n::t("calendar.tomorrow");
    }
    let weekday = i18n::nth("date.weekdays", date.weekday().num_days_from_monday() as usize);
    let long = i18n::tf(
        "date.long",
        &[("day", &date.day().to_string()), ("month", &i18n::nth("date.months", date.month0() as usize))],
    );
    format!("{} · {}", weekday, long)
}

fn clock(t: &DateTime<FixedOffset>, twelve_hour: bool) -> String {
    if twelve_hour {
        let (pm, h) = t.hour12();
        format!("{}:{:02} {}", h, t.minute(), if pm { "PM" } else { "AM" })
    } else {
        format!("{:02}:{:02}", t.hour(), t.minute())
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_calendar_card(
    canvas: &mut Canvas,
    theme: &Theme,
    rect: Rect,
    title: Option<&str>,
    items: &[SensorItemConfig],
    days: u32,
    twelve_hour: bool,
    states: &HashMap<String, EntityState>,
    targets: &mut Vec<InteractiveTarget>,
) {
    draw_card(canvas, theme, rect, None);
    let left = rect.x + PAD;
    let right = rect.x + rect.w - PAD;
    let bottom = rect.y + rect.h - PAD;
    let mut y = rect.y + PAD;
    if let Some(title) = title {
        canvas.draw_icon("calendar", left, y - 2.0, 28.0, theme.text_muted, 2.2);
        canvas.draw_text_fit(
            title,
            left + 38.0,
            y - 2.0,
            type_scale::BODY,
            true,
            theme.text_muted,
            TextAlign::Left,
            rect.w - 2.0 * PAD - 38.0,
        );
        y += TITLE_H;
    }
    targets.push(InteractiveTarget {
        rect,
        action: UiAction::Refresh,
    });

    let now = chrono::Utc::now().with_timezone(&theme.utc_offset);
    let events: Vec<Option<&serde_json::Value>> = items
        .iter()
        .map(|i| states.get(&i.entity_id).and_then(|s| s.attributes.get(CALENDAR_ATTR)))
        .collect();
    if events.iter().all(Option::is_none) {
        // Not fetched yet (or no calendar chosen)
        canvas.draw_text("--", left, y + 4.0, type_scale::BODY, false, theme.text_muted, TextAlign::Left);
        return;
    }
    let agenda = agenda(&events, now, days.clamp(1, crate::config::MAX_CALENDAR_DAYS));
    if agenda.is_empty() {
        canvas.draw_text_fit(
            &i18n::t("calendar.nothing"),
            left,
            y + 4.0,
            type_scale::BODY,
            false,
            theme.text_muted,
            TextAlign::Left,
            right - left,
        );
        return;
    }

    // Which calendar an event is from, when there are several
    let labels: Vec<String> = items
        .iter()
        .map(|i| {
            if !i.title.trim().is_empty() {
                i.title.clone()
            } else {
                states.get(&i.entity_id).and_then(|s| s.get_attr_str("friendly_name")).unwrap_or("").to_string()
            }
        })
        .collect();
    let show_labels = items.len() > 1 && rect.w >= 420.0;
    let label_w = if show_labels { ((right - left) * 0.28).min(200.0) } else { 0.0 };
    // The time column fits the widest time
    let sample = if twelve_hour { "12:00 PM" } else { "→ 00:00" };
    let time_w = canvas
        .measure_text(sample, type_scale::LABEL, true)
        .0
        .max(canvas.measure_text(&i18n::t("calendar.all_day"), type_scale::LABEL, true).0)
        .min((right - left) * 0.35)
        + 14.0;

    let total: usize = agenda.iter().map(|d| d.entries.len()).sum();
    let mut shown = 0usize;
    'days: for (n, day) in agenda.iter().enumerate() {
        if n > 0 {
            y += DAY_GAP;
        }
        // A day header needs room for at least one event under it
        if y + DAY_H + ROW_H > bottom {
            break;
        }
        canvas.draw_text_fit(
            &day_label(day.date, now.date_naive()),
            left,
            y + 4.0,
            type_scale::BODY + 2.0,
            true,
            theme.text,
            TextAlign::Left,
            right - left,
        );
        y += DAY_H;
        for (i, entry) in day.entries.iter().enumerate() {
            let remaining = total - shown;
            // Room for this row, and for the "more" line if others follow
            let need = if remaining > 1 { 2.0 * ROW_H } else { ROW_H };
            if y + need > bottom && !(remaining == 1 && y + ROW_H <= bottom) {
                break 'days;
            }
            let (time, color) = match &entry.when {
                When::AllDay => (i18n::t("calendar.all_day"), theme.accent.solid),
                When::At(t) => (clock(t, twelve_hour), theme.text_muted),
                When::Until(t) => (format!("→ {}", clock(t, twelve_hour)), theme.text_muted),
            };
            canvas.draw_text_fit(&time, left, y + 4.0, type_scale::LABEL, true, color, TextAlign::Left, time_w - 10.0);
            let mut text_right = right;
            if show_labels {
                let label = labels.get(entry.calendar).map(String::as_str).unwrap_or("");
                canvas.draw_text_fit(label, right, y + 4.0, type_scale::LABEL, false, theme.text_muted, TextAlign::Right, label_w);
                text_right -= label_w + 12.0;
            }
            let summary = if entry.summary.is_empty() { "—" } else { &entry.summary };
            canvas.draw_text_fit(
                summary,
                left + time_w,
                y + 2.0,
                type_scale::BODY,
                false,
                theme.text,
                TextAlign::Left,
                text_right - left - time_w,
            );
            // A thin rule between events of the same day
            if i + 1 < day.entries.len() {
                canvas.draw_line(left + time_w, y + ROW_H - 3.0, right, y + ROW_H - 3.0, 1.0, theme.border);
            }
            y += ROW_H;
            shown += 1;
        }
    }
    if shown < total {
        let more = i18n::tf("calendar.more", &[("count", &(total - shown).to_string())]);
        canvas.draw_text_fit(&more, left, y + 4.0, type_scale::LABEL, true, theme.text_muted, TextAlign::Left, right - left);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    #[test]
    fn merges_calendars_by_day() {
        let now = at("2026-11-22T10:00:00+01:00");
        let family = json!([
            {"summary": "Market", "start": "2026-11-22T11:00:00+01:00", "end": "2026-11-22T12:00:00+01:00", "all_day": false},
            {"summary": "Breakfast", "start": "2026-11-22T08:00:00+01:00", "end": "2026-11-22T09:00:00+01:00", "all_day": false},
            {"summary": "Holidays", "start": "2026-11-23", "end": "2026-11-25", "all_day": true},
            {"summary": "Too far", "start": "2026-11-25T09:00:00+01:00", "end": "2026-11-25T10:00:00+01:00", "all_day": false}
        ]);
        let work = json!([
            {"summary": "Call", "start": "2026-11-23T08:30:00Z", "end": "2026-11-23T09:00:00Z", "all_day": false},
            {"summary": "Night shift", "start": "2026-11-21T22:00:00+01:00", "end": "2026-11-22T14:00:00+01:00", "all_day": false}
        ]);
        let days = agenda(&[Some(&family), Some(&work), None], now, 3);
        let dates: Vec<String> = days.iter().map(|d| d.date.to_string()).collect();
        assert_eq!(dates, ["2026-11-22", "2026-11-23", "2026-11-24"]);

        // Today: the night shift still going on, then the market; breakfast is over
        let today: Vec<&str> = days[0].entries.iter().map(|e| e.summary.as_str()).collect();
        assert_eq!(today, ["Night shift", "Market"]);
        assert_eq!(days[0].entries[0].when, When::Until(at("2026-11-22T14:00:00+01:00")));
        assert_eq!(days[0].entries[1].calendar, 0);

        // Tomorrow: the all-day event first, the call in local time
        let tomorrow = &days[1].entries;
        assert_eq!(tomorrow[0].when, When::AllDay);
        assert_eq!(tomorrow[1].summary, "Call");
        assert_eq!(tomorrow[1].calendar, 1);
        assert_eq!(
            clock(
                &match &tomorrow[1].when {
                    When::At(t) => *t,
                    _ => panic!(),
                },
                false
            ),
            "09:30"
        );

        // The holidays end on the 25th: still on the 24th, which is the last day
        assert_eq!(days[2].entries.len(), 1);
    }

    #[test]
    fn nothing_planned_and_one_day() {
        let now = at("2026-11-22T10:00:00+01:00");
        assert!(agenda(&[Some(&json!([]))], now, 3).is_empty());
        let later = json!([{"summary": "Tomorrow", "start": "2026-11-23T09:00:00+01:00", "end": "2026-11-23T10:00:00+01:00", "all_day": false}]);
        assert!(agenda(&[Some(&later)], now, 1).is_empty());
        assert_eq!(agenda(&[Some(&later)], now, 2).len(), 1);
    }

    #[test]
    fn shared_events_show_once() {
        let now = at("2026-11-22T10:00:00+01:00");
        let ev = json!([{"summary": "Dinner", "start": "2026-11-22T19:00:00+01:00", "end": "2026-11-22T21:00:00+01:00", "all_day": false}]);
        let days = agenda(&[Some(&ev), Some(&ev)], now, 3);
        assert_eq!(days[0].entries.len(), 1);
    }
}
