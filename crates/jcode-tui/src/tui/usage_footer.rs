use super::info_widget::InfoWidgetData;
use crate::usage::{ProviderUsage, ProviderUsageSnapshot, UsageLimit};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const PURPLE: Color = Color::Rgb(190, 155, 255);
const TEAL: Color = Color::Rgb(91, 211, 190);
const ORANGE: Color = Color::Rgb(235, 164, 125);
const MUTED: Color = Color::Rgb(157, 163, 175);

pub(super) fn footer_lines(data: &InfoWidgetData, width: u16) -> Vec<Line<'static>> {
    #[cfg(test)]
    let snapshot = ProviderUsageSnapshot::default();
    #[cfg(not(test))]
    let snapshot = crate::usage::provider_usage_snapshot();
    render_footer(data, &snapshot, width as usize)
}

fn render_footer(
    data: &InfoWidgetData,
    snapshot: &ProviderUsageSnapshot,
    width: usize,
) -> Vec<Line<'static>> {
    vec![
        route_line(data, width),
        account_line(snapshot, false, width),
        account_line(snapshot, true, width),
    ]
}

fn colored(text: impl Into<String>, color: Color) -> Span<'static> {
    let color = match color {
        Color::Rgb(r, g, b) => super::color_support::rgb(r, g, b),
        other => other,
    };
    Span::styled(text.into(), Style::default().fg(color))
}

fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// Truncate by terminal cells, never wrap a status row.
fn shorten(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let cells = ch.width().unwrap_or(0);
        if used + cells > width - 1 {
            break;
        }
        result.push(ch);
        used += cells;
    }
    result.push('…');
    result
}

fn bound(mut line: Line<'static>, width: usize) -> Line<'static> {
    let mut remaining = width;
    for span in &mut line.spans {
        span.content = shorten(&span.content, remaining).into();
        remaining = remaining.saturating_sub(span.content.width());
    }
    line
}

fn count(value: u64) -> String {
    let (number, suffix) = if value >= 1_000_000 {
        (value as f64 / 1_000_000.0, "M")
    } else if value >= 1000 {
        (value as f64 / 1000.0, "k")
    } else {
        return value.to_string();
    };
    format!(
        "{}{suffix}",
        format!("{number:.1}")
            .trim_end_matches('0')
            .trim_end_matches('.')
    )
}

fn route_line(data: &InfoWidgetData, width: usize) -> Line<'static> {
    let model = match data.model.as_deref() {
        Some("gpt-6-astra") => "Astra".into(),
        Some(model) => clean(model),
        None => "Model unavailable".into(),
    };
    let tier = match data.service_tier.as_deref() {
        Some("priority" | "fast") => "Fast",
        Some("flex") => "Flex",
        _ => "Standard",
    };
    let effort = data
        .reasoning_effort
        .as_deref()
        .map(clean)
        .unwrap_or_default();
    let route = if effort.is_empty() {
        format!("✦ {model} · {tier}")
    } else {
        format!("✦ {model} · {effort} · {tier}")
    };
    let used = data.observed_context_tokens.or_else(|| {
        data.context_info
            .as_ref()
            .map(|c| c.estimated_tokens() as u64)
    });
    let context = if data.context_info_stale {
        "◉ Context updating".into()
    } else if let Some(used) = used {
        let estimate = if data.observed_context_tokens.is_none() {
            "~"
        } else {
            ""
        };
        let limit = data
            .context_limit
            .filter(|n| *n > 0)
            .map(|n| count(n as u64))
            .unwrap_or_else(|| "?".into());
        format!("◉ Context {estimate}{}/{limit}", count(used))
    } else {
        "◉ Context unavailable".into()
    };
    let context = shorten(&context, width);
    let left = shorten(&route, width.saturating_sub(context.width() + 2));
    let gap = width.saturating_sub(left.width() + context.width());
    bound(
        Line::from(vec![
            colored(left, PURPLE),
            Span::raw(" ".repeat(gap)),
            colored(context, PURPLE),
        ]),
        width,
    )
}

fn extra<'a>(report: &'a ProviderUsage, key: &str) -> Option<&'a str> {
    report
        .extra_info
        .iter()
        .find_map(|(k, v)| (k == key && !v.trim().is_empty()).then_some(v.as_str()))
}

fn generic_limit(report: &ProviderUsage, session: bool) -> Option<&UsageLimit> {
    report.limits.iter().find(|limit| {
        let name = limit.name.to_ascii_lowercase();
        let names: &[&str] = if session {
            &["5-hour window", "5-hour", "5h", "session"]
        } else {
            &["7-day window", "7-day", "7d", "weekly"]
        };
        names.contains(&name.as_str())
    })
}

fn quota(limit: &UsageLimit, label: &str, accent: Color) -> Span<'static> {
    let used = limit.usage_percent;
    if !used.is_finite() || used < 0.0 {
        return colored(format!("{label} unavailable"), MUTED);
    }
    let left = 100u8.saturating_sub(used.clamp(0.0, 100.0).round() as u8);
    let color = if left <= 10 {
        Color::Rgb(255, 112, 112)
    } else if left <= 25 {
        Color::Rgb(240, 193, 96)
    } else {
        accent
    };
    colored(format!("{label} {left}%"), color)
}

fn account_line(snapshot: &ProviderUsageSnapshot, claude: bool, width: usize) -> Line<'static> {
    let (prefix, accent) = if claude {
        ("✳ Claude  ", ORANGE)
    } else {
        ("◉ OpenAI  ", TEAL)
    };
    let mut reports = snapshot
        .reports
        .iter()
        .filter(|r| {
            if claude {
                r.provider_name.starts_with("Anthropic")
            } else {
                r.provider_name.starts_with("OpenAI")
            }
        })
        .collect::<Vec<_>>();
    reports.sort_by_key(|r| {
        (
            std::cmp::Reverse(r.provider_name.contains('✦')),
            std::cmp::Reverse(r.last_used_unix_secs),
        )
    });
    let mut spans = vec![colored(prefix, accent)];
    let Some(report) = reports.first() else {
        spans.push(colored(
            if snapshot.refreshing {
                "loading…"
            } else {
                "unavailable"
            },
            MUTED,
        ));
        return bound(Line::from(spans), width);
    };
    let mut details = Vec::new();
    if snapshot.stale {
        details.push(colored("stale · ", MUTED));
    }
    if report.error.is_some() {
        details.push(colored("unavailable", MUTED));
    } else if report.hard_limit_reached {
        details.push(colored("limit reached", Color::Rgb(255, 112, 112)));
    } else {
        if claude {
            if let Some(limit) = generic_limit(report, true) {
                details.push(quota(limit, "Session", accent));
            }
        }
        if let Some(limit) = generic_limit(report, false) {
            if !details.is_empty() {
                details.push(colored(" · ", MUTED));
            }
            details.push(quota(limit, "Weekly", accent));
        }
        if details.is_empty() {
            details.push(colored("unavailable", MUTED));
        } else if details.iter().any(|s| s.content.contains('%')) {
            details.push(colored(" left", MUTED));
        }
    }
    let additional = if reports.len() > 1 {
        format!(" +{} accounts", reports.len() - 1)
    } else {
        String::new()
    };
    let details_width: usize = details.iter().map(|s| s.content.width()).sum();
    let email_width = width.saturating_sub(prefix.width() + details_width + additional.width() + 2);
    let identity = extra(report, "Account email")
        .or_else(|| extra(report, "Account label"))
        .unwrap_or("email unavailable");
    let email = shorten(&clean(identity), email_width);
    if !email.is_empty() {
        spans.push(colored(email, MUTED));
    }
    if !additional.is_empty() {
        spans.push(colored(additional, MUTED));
    }
    spans.push(Span::raw("  "));
    spans.extend(details);
    // Reset is optional; keeping both quota values and the email has priority.
    if !claude && report.error.is_none() && !report.hard_limit_reached {
        if let Some(reset) = generic_limit(report, false).and_then(|l| l.resets_at.as_deref()) {
            let text = format!(" · ↻ {}", clean(&crate::usage::format_reset_time(reset)));
            let used: usize = spans.iter().map(|s| s.content.width()).sum();
            if used + text.width() <= width {
                spans.push(colored(text, MUTED));
            }
        }
    }
    bound(Line::from(spans), width)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn report(name: &str, email: &str) -> ProviderUsage {
        ProviderUsage {
            provider_name: name.into(),
            extra_info: vec![("Account email".into(), email.into())],
            limits: vec![
                UsageLimit {
                    name: "5-hour window".into(),
                    usage_percent: 2.0,
                    resets_at: None,
                },
                UsageLimit {
                    name: "7-day window".into(),
                    usage_percent: 34.0,
                    resets_at: None,
                },
                UsageLimit {
                    name: "7-day Fable window".into(),
                    usage_percent: 10.0,
                    resets_at: None,
                },
            ],
            ..Default::default()
        }
    }
    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }
    #[test]
    fn three_rows_keep_active_email_counts_and_generic_quotas() {
        let snapshot = ProviderUsageSnapshot {
            reports: vec![
                report("Anthropic other", "other@example.com"),
                report("Anthropic active ✦", "active@example.com"),
                report("OpenAI", "open@example.com"),
            ],
            ..Default::default()
        };
        let rows = render_footer(&InfoWidgetData::default(), &snapshot, 120);
        assert_eq!(rows.len(), 3);
        let claude = text(&rows[2]);
        assert!(
            claude.contains("active@example.com") && claude.contains("+1 accounts"),
            "{claude}"
        );
        assert!(claude.contains("Session 98%") && claude.contains("Weekly 66%"));
        assert!(!claude.contains("Fable"));
        assert_eq!(rows[1].spans[0].style.fg, colored("", TEAL).style.fg);
        assert_eq!(rows[2].spans[0].style.fg, colored("", ORANGE).style.fg);
    }
    #[test]
    fn rows_never_wrap_even_with_unicode_and_narrow_widths() {
        let snapshot = ProviderUsageSnapshot {
            reports: vec![report("Anthropic", "界界☃\nlong@example.com")],
            ..Default::default()
        };
        for width in [0, 1, 2, 20, 40, 80, 120] {
            let rows = render_footer(&InfoWidgetData::default(), &snapshot, width);
            assert_eq!(rows.len(), 3);
            assert!(
                rows.iter()
                    .all(|r| r.width() <= width && !text(r).contains('\n'))
            );
        }
    }
    #[test]
    fn unavailable_stale_and_low_quota_are_explicit() {
        let mut account = report("Anthropic", "a@example.com");
        account.limits[0].usage_percent = 95.0;
        account.limits[1].usage_percent = f32::NAN;
        let snapshot = ProviderUsageSnapshot {
            reports: vec![account],
            stale: true,
            ..Default::default()
        };
        let rows = render_footer(&InfoWidgetData::default(), &snapshot, 120);
        assert!(text(&rows[2]).contains("stale"));
        assert!(text(&rows[2]).contains("Weekly unavailable"));
        assert!(
            rows[2]
                .spans
                .iter()
                .any(|s| s.style.fg == colored("", Color::Rgb(255, 112, 112)).style.fg)
        );
        assert!(text(&rows[1]).contains("unavailable"));
    }
}
