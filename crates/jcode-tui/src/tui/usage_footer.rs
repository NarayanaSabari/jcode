use super::info_widget::{AuthMethod, InfoWidgetData, UsageInfo, UsageProvider};
use crate::usage::{ProviderUsage, ProviderUsageSnapshot, UsageLimit};
use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

const FOOTER_DEFAULT_WIDTH: usize = 1;

/// Render the compact usage footer below the composer.
///
/// The provider snapshot is deliberately read through the nonblocking cache
/// accessor. A frame must never wait for credentials or a provider request.
pub(super) fn footer_lines(
    data: &super::info_widget::InfoWidgetData,
    width: u16,
) -> Vec<Line<'static>> {
    let snapshot = provider_usage_snapshot();
    render_footer(data, &snapshot, width)
}

fn provider_usage_snapshot() -> ProviderUsageSnapshot {
    // TUI tests should remain hermetic. The production accessor may schedule a
    // refresh when its cache is stale, which would make unrelated widget
    // tests depend on the machine's configured credentials.
    #[cfg(test)]
    {
        ProviderUsageSnapshot::default()
    }
    #[cfg(not(test))]
    {
        crate::usage::provider_usage_snapshot()
    }
}

fn render_footer(
    data: &InfoWidgetData,
    snapshot: &ProviderUsageSnapshot,
    width: u16,
) -> Vec<Line<'static>> {
    let width = usize::from(width).max(FOOTER_DEFAULT_WIDTH);
    let mut lines = wrap_line(&route_summary(data), width);

    let mut has_anthropic_report = false;
    let mut has_openai_report = false;
    for report in &snapshot.reports {
        let Some(kind) = provider_kind(&report.provider_name) else {
            continue;
        };
        match kind {
            ProviderKind::Anthropic => has_anthropic_report = true,
            ProviderKind::OpenAi => has_openai_report = true,
        }
        lines.extend(wrap_line(
            &format_provider_report(report, kind, data),
            width,
        ));
    }

    // The all-account cache is populated in the background. Until its first
    // result arrives, keep the active provider's status visible from the
    // existing widget cache rather than displaying a misleading zero.
    if let Some(info) = data.usage_info.as_ref() {
        let kind = match info.provider {
            UsageProvider::Anthropic if !has_anthropic_report => Some(ProviderKind::Anthropic),
            UsageProvider::OpenAI if !has_openai_report => Some(ProviderKind::OpenAi),
            _ => None,
        };
        if let Some(kind) = kind {
            lines.extend(wrap_line(&format_usage_info(info, kind, data), width));
        }
    }

    // Keep cache state observable even when the route summary itself wraps.
    if snapshot.refreshing {
        let status = if snapshot.stale {
            "Usage refreshing · cached data may be stale"
        } else {
            "Usage refreshing…"
        };
        lines.extend(wrap_line(status, width));
    } else if snapshot.stale {
        lines.extend(wrap_line("Usage cached · stale", width));
    }

    lines
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderKind {
    Anthropic,
    OpenAi,
}

fn provider_kind(provider_name: &str) -> Option<ProviderKind> {
    let name = provider_name.trim().to_ascii_lowercase();
    if name.starts_with("anthropic") || name.contains("claude") {
        Some(ProviderKind::Anthropic)
    } else if name.starts_with("openai") || name.contains("chatgpt") || name.contains("codex") {
        Some(ProviderKind::OpenAi)
    } else {
        None
    }
}

fn route_summary(data: &InfoWidgetData) -> String {
    let mut parts = Vec::new();
    parts.push(format_context(data));

    parts.push(format!(
        "Model {}",
        data.model
            .as_deref()
            .map(sanitize_inline)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "unavailable".to_string())
    ));

    if let Some(provider) = data
        .provider_name
        .as_deref()
        .map(sanitize_inline)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("provider {}", provider));
    }
    if let Some(auth) = auth_label(data.auth_method) {
        parts.push(format!("auth {}", auth));
    }
    if let Some(effort) = data
        .reasoning_effort
        .as_deref()
        .map(sanitize_inline)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("effort {}", effort));
    }

    parts.push(format!(
        "tier {}",
        service_tier_label(data.service_tier.as_deref())
    ));
    parts.join(" · ")
}

fn format_context(data: &InfoWidgetData) -> String {
    if data.context_info_stale {
        return "Context updating".to_string();
    }

    let Some(info) = data.context_info.as_ref() else {
        return data
            .observed_context_tokens
            .map(|used| format_context_counts(used, data.context_limit, false))
            .unwrap_or_else(|| "Context unavailable".to_string());
    };
    if info.total_chars == 0 && data.observed_context_tokens.is_none() {
        return "Context unavailable".to_string();
    }

    let used = data
        .observed_context_tokens
        .map(|tokens| tokens as u64)
        .unwrap_or_else(|| info.estimated_tokens() as u64);
    format_context_counts(
        used,
        data.context_limit,
        data.observed_context_tokens.is_none(),
    )
}

fn format_context_counts(used: u64, limit: Option<usize>, estimated: bool) -> String {
    let used_text = format!(
        "{}{}",
        if estimated { "~" } else { "" },
        compact_count(used)
    );
    let Some(limit) = limit.map(|limit| limit.max(1) as u64) else {
        return format!("Context {} used (capacity unavailable)", used_text);
    };
    let limit = limit.max(1);
    let percent = used.saturating_mul(100).saturating_add(limit / 2) / limit;
    format!(
        "Context {}/{} ({}%)",
        used_text,
        compact_count(limit),
        percent.min(100)
    )
}

fn format_provider_report(
    report: &ProviderUsage,
    kind: ProviderKind,
    data: &InfoWidgetData,
) -> String {
    let identity = report_identity(report, kind);
    if report.error.is_some() {
        return format!("{} · unavailable", identity);
    }
    if report.hard_limit_reached {
        return format!("{} · hard limit", identity);
    }
    if report.limits.is_empty() {
        return format!("{} · unavailable", identity);
    }

    let limits = report
        .limits
        .iter()
        .filter_map(|limit| format_limit(limit, data.usage_display_used))
        .collect::<Vec<_>>();
    if limits.is_empty() {
        format!("{} · unavailable", identity)
    } else {
        format!("{} · {}", identity, limits.join(" · "))
    }
}

fn format_usage_info(info: &UsageInfo, kind: ProviderKind, data: &InfoWidgetData) -> String {
    let identity = match kind {
        ProviderKind::Anthropic => "Claude",
        ProviderKind::OpenAi => "OpenAI",
    };
    if !info.available {
        return format!("{} · unavailable", identity);
    }

    let mut limits = Vec::new();
    if let Some(label) = info.primary_limit_label.as_deref()
        && let Some(limit) = format_ratio_limit(
            label,
            info.five_hour,
            info.five_hour_resets_at.as_deref(),
            data.usage_display_used,
        )
    {
        limits.push(limit);
    }
    if let Some(label) = info.secondary_limit_label.as_deref()
        && let Some(limit) = format_ratio_limit(
            label,
            info.seven_day,
            info.seven_day_resets_at.as_deref(),
            data.usage_display_used,
        )
    {
        limits.push(limit);
    }
    if let Some(ratio) = info.spark
        && let Some(limit) = format_ratio_limit(
            "Spark",
            ratio,
            info.spark_resets_at.as_deref(),
            data.usage_display_used,
        )
    {
        limits.push(limit);
    }
    if limits.is_empty() {
        format!("{} · unavailable", identity)
    } else {
        format!("{} · {}", identity, limits.join(" · "))
    }
}

fn report_identity(report: &ProviderUsage, kind: ProviderKind) -> String {
    let provider = match kind {
        ProviderKind::Anthropic => "Claude",
        ProviderKind::OpenAi => "OpenAI",
    };
    let label = report_extra(report, "Account label");
    let email = report_extra(report, "Account email");
    match (label, email) {
        (Some(label), Some(email)) => format!(
            "{} {} <{}>",
            provider,
            sanitize_inline(label),
            sanitize_inline(email)
        ),
        (Some(label), None) => format!("{} {}", provider, sanitize_inline(label)),
        (None, Some(email)) => format!("{} <{}>", provider, sanitize_inline(email)),
        (None, None) => format!("{} {}", provider, sanitize_inline(&report.provider_name)),
    }
}

fn report_extra<'a>(report: &'a ProviderUsage, key: &str) -> Option<&'a str> {
    report
        .extra_info
        .iter()
        .rev()
        .find_map(|(name, value)| (name == key).then_some(value.as_str()))
        .filter(|value| !value.trim().is_empty())
}

fn format_limit(limit: &UsageLimit, usage_display_used: bool) -> Option<String> {
    if !limit.usage_percent.is_finite() || limit.usage_percent < 0.0 {
        return None;
    }
    let name = compact_limit_name(&limit.name);
    let ratio = (limit.usage_percent / 100.0).clamp(0.0, 1.0);
    let mut result = format_ratio(&name, ratio, usage_display_used)?;
    if let Some(reset) = limit.resets_at.as_deref() {
        result.push_str(" · resets ");
        result.push_str(&sanitize_inline(&crate::usage::format_reset_time(reset)));
    }
    Some(result)
}

fn format_ratio_limit(
    name: &str,
    ratio: f32,
    reset: Option<&str>,
    usage_display_used: bool,
) -> Option<String> {
    if !ratio.is_finite() || ratio < 0.0 {
        return None;
    }
    let mut result = format_ratio(&compact_limit_name(name), ratio, usage_display_used)?;
    if let Some(reset) = reset {
        result.push_str(" · resets ");
        result.push_str(&sanitize_inline(&crate::usage::format_reset_time(reset)));
    }
    Some(result)
}

fn format_ratio(name: &str, ratio: f32, usage_display_used: bool) -> Option<String> {
    if !ratio.is_finite() || ratio < 0.0 {
        return None;
    }
    let used = (ratio * 100.0).round().clamp(0.0, 100.0) as u8;
    let value = if usage_display_used {
        format!("{}% used", used)
    } else {
        format!("{}% left", 100u8.saturating_sub(used))
    };
    Some(format!("{} {}", name, value))
}

fn compact_limit_name(name: &str) -> String {
    // Keep model qualifiers and each quota window distinct.
    sanitize_inline(name)
        .replace("5-hour", "5h")
        .replace("5 hour", "5h")
        .replace("7-day", "7d")
        .replace("7 day", "7d")
}

fn auth_label(method: AuthMethod) -> Option<&'static str> {
    match method {
        AuthMethod::AnthropicOAuth => Some("Claude OAuth"),
        AuthMethod::AnthropicApiKey => Some("Claude API"),
        AuthMethod::OpenAIOAuth => Some("OpenAI OAuth"),
        AuthMethod::OpenAIApiKey => Some("OpenAI API"),
        AuthMethod::OpenRouterApiKey => Some("OpenRouter API"),
        AuthMethod::OpenCodeApiKey => Some("OpenCode API"),
        AuthMethod::CopilotOAuth => Some("Copilot OAuth"),
        AuthMethod::GeminiOAuth => Some("Gemini OAuth"),
        AuthMethod::ApiKey => Some("API key"),
        AuthMethod::Unknown => None,
    }
}

fn service_tier_label(service_tier: Option<&str>) -> String {
    let Some(tier) = service_tier.map(str::trim).filter(|tier| !tier.is_empty()) else {
        return "Standard".to_string();
    };
    match tier.to_ascii_lowercase().as_str() {
        "priority" | "fast" => "Fast".to_string(),
        "flex" => "Flex".to_string(),
        "off" | "default" | "auto" | "none" => "Standard".to_string(),
        _ => sanitize_inline(tier),
    }
}

fn compact_count(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn sanitize_inline(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn wrap_line(text: &str, width: usize) -> Vec<Line<'static>> {
    let width = width.max(FOOTER_DEFAULT_WIDTH);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for ch in text.chars() {
        if ch == '\n' {
            lines.push(Line::from(std::mem::take(&mut current)));
            current_width = 0;
            continue;
        }

        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if ch_width > 0 && current_width > 0 && current_width + ch_width > width {
            lines.push(Line::from(std::mem::take(&mut current)));
            current_width = 0;
            if ch.is_whitespace() {
                continue;
            }
        }
        current.push(ch);
        current_width = current_width.saturating_add(ch_width);
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(Line::from(current));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::ContextInfo;
    use unicode_width::UnicodeWidthStr;

    fn sample_data() -> InfoWidgetData {
        InfoWidgetData {
            context_info: Some(ContextInfo {
                total_chars: 50_000,
                ..Default::default()
            }),
            context_limit: Some(200_000),
            observed_context_tokens: Some(12_345),
            model: Some("gpt-6-astra".to_string()),
            reasoning_effort: Some("low".to_string()),
            service_tier: Some("priority".to_string()),
            provider_name: Some("OpenAI".to_string()),
            auth_method: AuthMethod::OpenAIOAuth,
            usage_display_used: true,
            ..Default::default()
        }
    }

    fn claude_report(label: &str, email: &str, usage: f32) -> ProviderUsage {
        ProviderUsage {
            provider_name: format!("Anthropic - {}", label),
            limits: vec![
                UsageLimit {
                    name: "5-hour window".to_string(),
                    usage_percent: usage,
                    resets_at: Some("2099-01-01T00:00:00Z".to_string()),
                },
                UsageLimit {
                    name: "7-day window".to_string(),
                    usage_percent: usage + 10.0,
                    resets_at: None,
                },
            ],
            extra_info: vec![
                ("Account label".to_string(), label.to_string()),
                ("Account email".to_string(), email.to_string()),
            ],
            ..Default::default()
        }
    }

    fn plain(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn renders_all_claude_accounts_with_email_and_reset_countdown() {
        let snapshot = ProviderUsageSnapshot {
            reports: vec![
                claude_report("work", "alice@example.com", 12.0),
                claude_report("personal", "bob@example.com", 0.0),
            ],
            refreshing: false,
            stale: false,
        };
        let text = render_footer(&sample_data(), &snapshot, 120)
            .iter()
            .map(plain)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Context 12.3k/200.0k (6%)"), "{text}");
        assert!(text.contains("Model gpt-6-astra"), "{text}");
        assert!(text.contains("tier Fast"), "{text}");
        assert!(text.contains("alice@example.com"), "{text}");
        assert!(text.contains("bob@example.com"), "{text}");
        assert!(text.contains("5h window 12% used"), "{text}");
        assert!(text.contains("resets"), "{text}");
        assert!(text.contains("5h window 0% used"), "{text}");
    }

    #[test]
    fn renders_unavailable_without_turning_it_into_zero() {
        let mut data = sample_data();
        data.context_info = None;
        data.observed_context_tokens = None;
        data.usage_info = Some(UsageInfo {
            provider: UsageProvider::OpenAI,
            available: false,
            ..Default::default()
        });
        let snapshot = ProviderUsageSnapshot::default();
        let text = render_footer(&data, &snapshot, 120)
            .iter()
            .map(plain)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Context unavailable"), "{text}");
        assert!(text.contains("OpenAI · unavailable"), "{text}");
        assert!(!text.contains("OpenAI · 0%"), "{text}");
    }

    #[test]
    fn labels_stale_cache_and_unknown_context_capacity() {
        let mut data = sample_data();
        data.context_limit = None;
        let snapshot = ProviderUsageSnapshot {
            refreshing: false,
            stale: true,
            ..ProviderUsageSnapshot::default()
        };
        let text = render_footer(&data, &snapshot, 120)
            .iter()
            .map(plain)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            text.contains("Context 12.3k used (capacity unavailable)"),
            "{text}"
        );
        assert!(text.contains("Usage cached · stale"), "{text}");
    }

    #[test]
    fn quota_labels_keep_model_and_window_distinctions() {
        assert_ne!(
            compact_limit_name("7-day Sonnet"),
            compact_limit_name("7-day")
        );
        assert_ne!(
            compact_limit_name("Spark 5-hour"),
            compact_limit_name("Spark 7-day")
        );
    }

    #[test]
    fn wraps_unicode_and_long_emails_to_the_requested_width() {
        let snapshot = ProviderUsageSnapshot {
            reports: vec![claude_report("☃", "équipe@example.com", 34.0)],
            refreshing: false,
            stale: false,
        };
        let lines = render_footer(&sample_data(), &snapshot, 24);
        assert!(lines.len() > 3);
        for line in &lines {
            assert!(
                UnicodeWidthStr::width(plain(line).as_str()) <= 24,
                "{}",
                plain(line)
            );
        }

        let text = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
        assert!(text.contains("@"), "{text}");
        assert!(text.contains("☃"), "{text}");
    }
}
