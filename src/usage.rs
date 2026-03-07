use crate::debug::{LogLevel, log_to_file};
use crate::http_client;
use chrono::{Datelike, TimeZone};
use serde::Deserialize;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Claude,
    Codex,
    Zai,
}

impl Model {
    pub fn label(&self) -> &'static str {
        match self {
            Model::Claude => "claude",
            Model::Codex => "codex",
            Model::Zai => "zai",
        }
    }

    pub fn all() -> &'static [Model] {
        &[Model::Claude, Model::Codex, Model::Zai]
    }

    pub fn next(&self) -> Model {
        match self {
            Model::Claude => Model::Codex,
            Model::Codex => Model::Zai,
            Model::Zai => Model::Claude,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeUsageData {
    pub today_messages: u64,
    pub today_sessions: u64,
    pub today_tool_calls: u64,
    pub today_tokens: u64,
    pub five_hour_pct: Option<f64>,
    pub seven_day_pct: Option<f64>,
    pub five_hour_resets: Option<String>,
    pub seven_day_resets: Option<String>,
    pub subscription_type: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ZaiUsageData {
    pub today_tokens: u64,
    pub today_calls: u64,
    pub monthly_tokens: u64,
    pub weekly_tokens: u64,
    pub five_hour_tokens: u64,
    pub monthly_token_limit: Option<u64>,
    pub monthly_usage_pct: Option<f64>,
    pub weekly_token_limit: Option<u64>,
    pub weekly_usage_pct: Option<f64>,
    pub five_hour_token_limit: Option<u64>,
    pub five_hour_usage_pct: Option<f64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CodexUsageData {
    pub today_tokens: u64,
    pub today_calls: u64,
    pub five_hour_tokens: u64,
    pub five_hour_usage_pct: Option<f64>,
    pub weekly_usage_pct: Option<f64>,
    pub five_hour_resets: Option<String>,
    pub weekly_resets: Option<String>,
    pub plan_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UsageData {
    pub visible_model: Model,
    pub claude: ClaudeUsageData,
    pub codex: CodexUsageData,
    pub zai: ZaiUsageData,
}

impl Default for UsageData {
    fn default() -> Self {
        Self {
            visible_model: Model::Claude,
            claude: ClaudeUsageData::default(),
            codex: CodexUsageData::default(),
            zai: ZaiUsageData::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatsCache {
    daily_activity: Vec<DailyActivity>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DailyActivity {
    date: String,
    message_count: u64,
    session_count: u64,
    tool_call_count: u64,
}

#[derive(Debug, Deserialize)]
struct ConversationEntry {
    timestamp: Option<String>,
    message: Option<ConversationMessage>,
}

#[derive(Debug, Deserialize)]
struct ConversationMessage {
    usage: Option<ConversationUsage>,
}

#[derive(Debug, Deserialize)]
struct ConversationUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct RateLimitResponse {
    five_hour: Option<RateLimitWindow>,
    seven_day: Option<RateLimitWindow>,
}

#[derive(Debug, Clone, Deserialize)]
struct RateLimitWindow {
    utilization: f64,
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Credentials {
    claude_ai_oauth: Option<OAuthCreds>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuthCreds {
    access_token: String,
    subscription_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpencodeMessage {
    #[serde(default)]
    time: Option<OpencodeTime>,
}

#[derive(Debug, Deserialize)]
struct OpencodeTime {
    created: i64,
}

#[derive(Debug, Deserialize)]
struct OpencodePart {
    #[serde(rename = "type")]
    part_type: String,
    tokens: Option<OpencodeTokens>,
}

#[derive(Debug, Deserialize)]
struct OpencodeTokens {
    input: u64,
    output: u64,
    #[serde(default)]
    cache: Option<OpencodeCache>,
}

#[derive(Debug, Deserialize)]
struct OpencodeCache {
    #[serde(default)]
    read: u64,
    #[serde(default)]
    write: u64,
}

#[derive(Debug, Deserialize)]
struct ZaiAuth {
    #[serde(rename = "zai-coding-plan")]
    zai_coding_plan: Option<ZaiApiKey>,
}

#[derive(Debug, Deserialize)]
struct ZaiApiKey {
    key: String,
}

#[derive(Debug, Deserialize)]
struct ZaiUsageResponse {
    data: ZaiApiResponseData,
}

#[derive(Debug, Deserialize)]
struct ZaiApiResponseData {
    #[serde(rename = "totalUsage")]
    total_usage: ZaiTotalUsage,
}

#[derive(Debug, Deserialize)]
struct ZaiTotalUsage {
    #[serde(rename = "totalModelCallCount")]
    total_model_call_count: u64,
    #[serde(rename = "totalTokensUsage")]
    total_tokens_usage: u64,
}

#[derive(Debug, Deserialize)]
struct CodexSessionEvent {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    event_type: String,
    payload: Option<CodexEventPayload>,
}

#[derive(Debug, Deserialize)]
struct CodexEventPayload {
    #[serde(rename = "type")]
    payload_type: Option<String>,
    info: Option<CodexEventInfo>,
    rate_limits: Option<CodexRateLimits>,
}

#[derive(Debug, Deserialize)]
struct CodexEventInfo {
    total_token_usage: Option<CodexTokenUsage>,
    last_token_usage: Option<CodexTokenUsage>,
}

#[derive(Debug, Deserialize)]
struct CodexTokenUsage {
    total_tokens: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
struct CodexRateLimits {
    primary: Option<CodexRateLimitWindow>,
    secondary: Option<CodexRateLimitWindow>,
    plan_type: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct CodexRateLimitWindow {
    used_percent: Option<f64>,
    window_minutes: Option<u64>,
    resets_at: Option<i64>,
}

pub struct UsageManager {
    data: Arc<Mutex<UsageData>>,
    last_stats_refresh: Option<Instant>,
    last_oauth_refresh: Option<Instant>,
    last_cycle: Instant,
    cycle_interval_secs: u64,
    zai_enabled: bool,
    codex_enabled: bool,
    zai_monthly_limit: Option<u64>,
    zai_weekly_limit: Option<u64>,
    zai_five_hour_limit: Option<u64>,
}

impl UsageManager {
    pub fn new(
        zai_enabled: bool,
        zai_monthly_limit: Option<u64>,
        zai_weekly_limit: Option<u64>,
        zai_five_hour_limit: Option<u64>,
    ) -> Self {
        let mut data = UsageData::default();
        data.zai.monthly_token_limit = zai_monthly_limit;
        data.zai.weekly_token_limit = zai_weekly_limit;
        data.zai.five_hour_token_limit = zai_five_hour_limit;
        Self {
            data: Arc::new(Mutex::new(data)),
            last_stats_refresh: None,
            last_oauth_refresh: None,
            last_cycle: Instant::now(),
            cycle_interval_secs: 5,
            zai_enabled,
            codex_enabled: true,
            zai_monthly_limit,
            zai_weekly_limit,
            zai_five_hour_limit,
        }
    }

    pub fn get_data(&self) -> UsageData {
        self.data.lock().unwrap().clone()
    }

    pub fn cycle_visible_model(&mut self) {
        let mut data = self.data.lock().unwrap();
        let mut next = data.visible_model;
        for _ in 0..Model::all().len() {
            next = next.next();
            if self.model_enabled(next) {
                data.visible_model = next;
                return;
            }
        }
    }

    pub fn should_cycle(&self) -> bool {
        let enabled_models = 1 + usize::from(self.codex_enabled) + usize::from(self.zai_enabled);
        enabled_models > 1 && self.last_cycle.elapsed().as_secs() >= self.cycle_interval_secs
    }

    pub fn refresh(&mut self) {
        let now = Instant::now();

        if self.should_cycle() {
            self.cycle_visible_model();
            self.last_cycle = now;
        }

        let should_refresh_stats = self
            .last_stats_refresh
            .map(|t| now.duration_since(t).as_secs() >= 30)
            .unwrap_or(true);

        if should_refresh_stats {
            self.refresh_claude_stats();
            self.refresh_codex_stats();
            self.last_stats_refresh = Some(now);
        }

        let should_refresh_oauth = self
            .last_oauth_refresh
            .map(|t| now.duration_since(t).as_secs() >= 60)
            .unwrap_or(true);

        if should_refresh_oauth {
            self.last_oauth_refresh = Some(now);
            let data = Arc::clone(&self.data);
            let zai_enabled = self.zai_enabled;
            let monthly = self.zai_monthly_limit;
            let weekly = self.zai_weekly_limit;
            let five_hour = self.zai_five_hour_limit;
            std::thread::spawn(move || {
                fetch_rate_limits(&data);
                if zai_enabled {
                    fetch_zai_usage(&data, monthly, weekly, five_hour);
                }
            });
        }
    }

    fn refresh_claude_stats(&self) {
        let Some(claude_dir) = dirs::home_dir().map(|h| h.join(".claude")) else {
            return;
        };

        let stats_path = claude_dir.join("stats-cache.json");
        let Ok(contents) = std::fs::read_to_string(&stats_path) else {
            return;
        };

        let Ok(cache) = serde_json::from_str::<StatsCache>(&contents) else {
            return;
        };

        let today = chrono::Local::now().format("%Y-%m-%d").to_string();

        let today_stats = cache.daily_activity.iter().find(|d| d.date == today);

        let today_tokens = calculate_claude_today_tokens(&today);

        let mut data = self.data.lock().unwrap();
        if let Some(stats) = today_stats {
            data.claude.today_messages = stats.message_count;
            data.claude.today_sessions = stats.session_count;
            data.claude.today_tool_calls = stats.tool_call_count;
        } else {
            data.claude.today_messages = 0;
            data.claude.today_sessions = 0;
            data.claude.today_tool_calls = 0;
        }
        data.claude.today_tokens = today_tokens;
    }

    fn refresh_codex_stats(&self) {
        let mut data = self.data.lock().unwrap();
        let stats = calculate_codex_usage();
        data.codex.today_tokens = stats.today_tokens;
        data.codex.today_calls = stats.today_calls;
        data.codex.five_hour_tokens = stats.five_hour_tokens;
        data.codex.five_hour_usage_pct = stats.five_hour_usage_pct;
        data.codex.weekly_usage_pct = stats.weekly_usage_pct;
        data.codex.five_hour_resets = stats.five_hour_resets.clone();
        data.codex.weekly_resets = stats.weekly_resets.clone();
        data.codex.plan_type = stats.plan_type.clone();

        let sessions_path = codex_sessions_root()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<none>".to_string());
        let summary = format!(
            "codex refresh path={} 5h_pct={:?} 7d_pct={:?} 5h_reset={:?} 7d_reset={:?} plan={:?} today_tokens={} five_hour_tokens={} calls={}",
            sessions_path,
            stats.five_hour_usage_pct,
            stats.weekly_usage_pct,
            stats.five_hour_resets,
            stats.weekly_resets,
            stats.plan_type,
            stats.today_tokens,
            stats.five_hour_tokens,
            stats.today_calls
        );
        log_to_file(LogLevel::Debug, "usage", &summary);

        if stats.five_hour_usage_pct.is_none() && stats.weekly_usage_pct.is_none() {
            log_to_file(
                LogLevel::Debug,
                "usage",
                "codex refresh: no rate-limit percentages found, using token fallback display",
            );
        }
    }

    fn model_enabled(&self, model: Model) -> bool {
        match model {
            Model::Claude => true,
            Model::Codex => self.codex_enabled,
            Model::Zai => self.zai_enabled,
        }
    }
}

fn fetch_rate_limits(data: &Arc<Mutex<UsageData>>) {
    let Some(claude_dir) = dirs::home_dir().map(|h| h.join(".claude")) else {
        log_to_file(
            LogLevel::Debug,
            "usage",
            "claude oauth usage: home_dir unavailable",
        );
        return;
    };

    let creds_path = claude_dir.join(".credentials.json");
    let Ok(contents) = std::fs::read_to_string(&creds_path) else {
        log_to_file(
            LogLevel::Debug,
            "usage",
            &format!(
                "claude oauth usage: credentials file missing/unreadable at {}",
                creds_path.display()
            ),
        );
        return;
    };

    let Ok(creds) = serde_json::from_str::<Credentials>(&contents) else {
        log_to_file(
            LogLevel::Warn,
            "usage",
            "claude oauth usage: failed to parse credentials JSON",
        );
        return;
    };

    let Some(oauth) = creds.claude_ai_oauth else {
        log_to_file(
            LogLevel::Debug,
            "usage",
            "claude oauth usage: claude_ai_oauth not present in credentials",
        );
        return;
    };

    log_to_file(
        LogLevel::Debug,
        "usage",
        &format!(
            "claude oauth usage: requesting oauth usage endpoint (sub={:?})",
            oauth.subscription_type
        ),
    );

    {
        let mut d = data.lock().unwrap();
        d.claude.subscription_type = oauth.subscription_type;
    }

    let result = http_client::https_agent()
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", &format!("Bearer {}", oauth.access_token))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "claude-code/2.1.42")
        .header("Content-Type", "application/json")
        .call();

    match result {
        Ok(mut response) => {
            log_to_file(
                LogLevel::Debug,
                "usage",
                "claude oauth usage: HTTP request succeeded; parsing body",
            );
            let body = match response.body_mut().read_to_string() {
                Ok(b) => b,
                Err(e) => {
                    let mut d = data.lock().unwrap();
                    d.claude.last_error = Some(format!("Read error: {}", e));
                    log_to_file(
                        LogLevel::Warn,
                        "usage",
                        &format!("claude oauth usage: body read error: {e}"),
                    );
                    return;
                }
            };

            match serde_json::from_str::<RateLimitResponse>(&body) {
                Ok(resp) => {
                    let mut d = data.lock().unwrap();
                    d.claude.five_hour_pct = resp.five_hour.as_ref().map(|w| w.utilization);
                    d.claude.five_hour_resets =
                        resp.five_hour.as_ref().and_then(|w| w.resets_at.clone());
                    d.claude.seven_day_pct = resp.seven_day.as_ref().map(|w| w.utilization);
                    d.claude.seven_day_resets =
                        resp.seven_day.as_ref().and_then(|w| w.resets_at.clone());
                    d.claude.last_error = None;
                    log_to_file(
                        LogLevel::Debug,
                        "usage",
                        &format!(
                            "claude oauth usage: parsed windows 5h={:?} 7d={:?}",
                            d.claude.five_hour_pct, d.claude.seven_day_pct
                        ),
                    );
                }
                Err(e) => {
                    let mut d = data.lock().unwrap();
                    d.claude.last_error = Some(format!("Parse error: {}", e));
                    let snippet: String = body.chars().take(240).collect();
                    log_to_file(
                        LogLevel::Warn,
                        "usage",
                        &format!(
                            "claude oauth usage: parse error: {e}; body_prefix={}",
                            snippet.replace('\n', "\\n")
                        ),
                    );
                }
            }
        }
        Err(e) => {
            let mut d = data.lock().unwrap();
            d.claude.last_error = Some(format!("HTTP error: {}", e));
            log_to_file(
                LogLevel::Warn,
                "usage",
                &format!("claude oauth usage: HTTP error: {e}"),
            );
        }
    }
}

fn fetch_zai_usage(
    _data: &Arc<Mutex<UsageData>>,
    _monthly_limit: Option<u64>,
    _weekly_limit: Option<u64>,
    _five_hour_limit: Option<u64>,
) {
}

fn calculate_claude_today_tokens(today: &str) -> u64 {
    let Some(projects_dir) = dirs::home_dir().map(|h| h.join(".claude").join("projects")) else {
        return 0;
    };
    if !projects_dir.exists() {
        return 0;
    }

    let Ok(proj_entries) = std::fs::read_dir(&projects_dir) else {
        return 0;
    };

    let mut total: u64 = 0;

    for proj_entry in proj_entries.flatten() {
        let proj_path = proj_entry.path();
        if !proj_path.is_dir() {
            continue;
        }

        let Ok(files) = std::fs::read_dir(&proj_path) else {
            continue;
        };

        for file_entry in files.flatten() {
            let file_path = file_entry.path();
            if file_path.extension().map(|e| e != "jsonl").unwrap_or(true) {
                continue;
            }

            // Only read files modified today as a quick filter
            let Ok(meta) = file_entry.metadata() else {
                continue;
            };
            let Ok(mtime) = meta.modified() else {
                continue;
            };
            let mdate = chrono::DateTime::<chrono::Local>::from(mtime)
                .format("%Y-%m-%d")
                .to_string();
            if mdate != today {
                continue;
            }

            let Ok(contents) = std::fs::read_to_string(&file_path) else {
                continue;
            };

            for line in contents.lines() {
                let Ok(entry) = serde_json::from_str::<ConversationEntry>(line) else {
                    continue;
                };
                let Some(ref ts) = entry.timestamp else {
                    continue;
                };
                if !ts.starts_with(today) {
                    continue;
                }
                if let Some(msg) = entry.message
                    && let Some(usage) = msg.usage
                {
                    total += usage.input_tokens
                        + usage.output_tokens
                        + usage.cache_read_input_tokens
                        + usage.cache_creation_input_tokens;
                }
            }
        }
    }

    total
}

fn calculate_five_hour_usage(_data: &std::sync::MutexGuard<UsageData>) -> u64 {
    let five_hours_ago = chrono::Local::now() - chrono::Duration::hours(5);
    let five_hours_ago_ts = five_hours_ago.timestamp_millis();

    let Some(data_dir) = dirs::data_dir().map(|d| d.join("opencode").join("storage")) else {
        return 0;
    };

    let message_path = data_dir.join("message");
    let part_path = data_dir.join("part");

    if !message_path.exists() {
        return 0;
    }

    let mut total_tokens: u64 = 0;

    let Ok(session_dirs) = std::fs::read_dir(&message_path) else {
        return 0;
    };

    for session_entry in session_dirs.flatten() {
        if !session_entry
            .file_type()
            .map(|t| t.is_dir())
            .unwrap_or(false)
        {
            continue;
        }

        let session_path = session_entry.path();
        let Ok(message_files) = std::fs::read_dir(&session_path) else {
            continue;
        };

        for msg_entry in message_files.flatten() {
            let msg_path = msg_entry.path();
            if msg_path.extension().map(|e| e != "json").unwrap_or(true) {
                continue;
            }

            let Ok(contents) = std::fs::read_to_string(&msg_path) else {
                continue;
            };

            let Ok(msg) = serde_json::from_str::<OpencodeMessage>(&contents) else {
                continue;
            };

            let Some(time) = msg.time else {
                continue;
            };

            if time.created < five_hours_ago_ts {
                continue;
            }

            let msg_id = msg_path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
            let part_dir = part_path.join(msg_id);
            if !part_dir.exists() {
                continue;
            }

            let Ok(part_files) = std::fs::read_dir(&part_dir) else {
                continue;
            };

            for part_entry in part_files.flatten() {
                let part_file = part_entry.path();
                if part_file.extension().map(|e| e != "json").unwrap_or(true) {
                    continue;
                }

                let Ok(part_contents) = std::fs::read_to_string(&part_file) else {
                    continue;
                };

                let Ok(part) = serde_json::from_str::<OpencodePart>(&part_contents) else {
                    continue;
                };

                if part.part_type == "step-finish"
                    && let Some(tokens) = part.tokens
                {
                    total_tokens += tokens.input + tokens.output;
                }
            }
        }
    }

    total_tokens
}

fn calculate_codex_usage() -> CodexUsageData {
    let Some(sessions_root) = codex_sessions_root() else {
        return CodexUsageData::default();
    };
    if !sessions_root.exists() {
        return CodexUsageData::default();
    }

    let now_local = chrono::Local::now();
    let today = now_local.date_naive();
    let yesterday = (now_local - chrono::Duration::days(1)).date_naive();
    let five_hours_ago_utc = chrono::Utc::now() - chrono::Duration::hours(5);

    let mut candidates = Vec::with_capacity(2);
    for day in [today, yesterday] {
        candidates.push(
            sessions_root
                .join(format!("{:04}", day.year()))
                .join(format!("{:02}", day.month()))
                .join(format!("{:02}", day.day())),
        );
    }

    let mut stats = CodexUsageData::default();
    let mut latest_rate_limits_ts: Option<chrono::DateTime<chrono::Utc>> = None;

    for day_dir in candidates {
        if !day_dir.exists() {
            continue;
        }

        let Ok(entries) = std::fs::read_dir(day_dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let Ok(contents) = std::fs::read_to_string(path) else {
                continue;
            };

            for line in contents.lines() {
                if let Some((dt_utc, rate_limits)) = extract_rate_limits_from_json_line(line) {
                    if latest_rate_limits_ts.is_none_or(|latest| dt_utc > latest) {
                        apply_codex_rate_limits(&mut stats, &rate_limits);
                        latest_rate_limits_ts = Some(dt_utc);
                    }
                }

                let Ok(event) = serde_json::from_str::<CodexSessionEvent>(line) else {
                    continue;
                };
                if event.event_type != "event_msg" {
                    continue;
                }

                let Some(payload) = event.payload else {
                    continue;
                };
                if payload.payload_type.as_deref() != Some("token_count") {
                    continue;
                }

                let Some(ts) = event.timestamp else {
                    continue;
                };
                let Ok(dt_fixed) = chrono::DateTime::parse_from_rfc3339(&ts) else {
                    continue;
                };
                let dt_local = dt_fixed.with_timezone(&chrono::Local);
                let dt_utc = dt_fixed.with_timezone(&chrono::Utc);
                // Structured path kept for compatibility with known schema.
                if let Some(rate_limits) = payload.rate_limits.clone()
                    && latest_rate_limits_ts.is_none_or(|latest| dt_utc > latest)
                {
                    apply_codex_rate_limits(&mut stats, &rate_limits);
                    latest_rate_limits_ts = Some(dt_utc);
                }

                let tokens = payload
                    .info
                    .and_then(|info| info.last_token_usage.or(info.total_token_usage))
                    .and_then(|usage| usage.total_tokens)
                    .unwrap_or(0);

                if dt_local.date_naive() == today {
                    stats.today_tokens = stats.today_tokens.saturating_add(tokens);
                    stats.today_calls = stats.today_calls.saturating_add(1);
                }

                if dt_utc >= five_hours_ago_utc {
                    stats.five_hour_tokens = stats.five_hour_tokens.saturating_add(tokens);
                }
            }
        }
    }

    if stats.five_hour_usage_pct.is_none() && stats.weekly_usage_pct.is_none() {
        if let Some(limits) = find_latest_codex_rate_limits(&sessions_root) {
            apply_codex_rate_limits(&mut stats, &limits);
        }
    }

    stats
}

fn apply_codex_rate_limits(stats: &mut CodexUsageData, limits: &CodexRateLimits) {
    for window in [&limits.primary, &limits.secondary].into_iter().flatten() {
        match window.window_minutes {
            Some(300) => {
                stats.five_hour_usage_pct = window.used_percent;
                stats.five_hour_resets = window.resets_at.and_then(format_unix_reset);
            }
            Some(10_080) => {
                stats.weekly_usage_pct = window.used_percent;
                stats.weekly_resets = window.resets_at.and_then(format_unix_reset);
            }
            None => {
                // Some plans only return one unnamed window; treat it as 5h.
                if stats.five_hour_usage_pct.is_none() {
                    stats.five_hour_usage_pct = window.used_percent;
                    stats.five_hour_resets = window.resets_at.and_then(format_unix_reset);
                }
            }
            _ => {}
        }
    }

    stats.plan_type = limits.plan_type.clone();
}

fn format_unix_reset(epoch_seconds: i64) -> Option<String> {
    let dt = chrono::Local.timestamp_opt(epoch_seconds, 0).single()?;
    Some(dt.to_rfc3339())
}

fn find_latest_codex_rate_limits(sessions_root: &std::path::Path) -> Option<CodexRateLimits> {
    let mut newest: Option<(chrono::DateTime<chrono::Utc>, CodexRateLimits)> = None;

    let Ok(year_dirs) = std::fs::read_dir(sessions_root) else {
        return None;
    };

    for year in year_dirs.flatten() {
        let year_path = year.path();
        if !year_path.is_dir() {
            continue;
        }
        let Ok(month_dirs) = std::fs::read_dir(&year_path) else {
            continue;
        };
        for month in month_dirs.flatten() {
            let month_path = month.path();
            if !month_path.is_dir() {
                continue;
            }
            let Ok(day_dirs) = std::fs::read_dir(&month_path) else {
                continue;
            };
            for day in day_dirs.flatten() {
                let day_path = day.path();
                if !day_path.is_dir() {
                    continue;
                }
                let Ok(files) = std::fs::read_dir(&day_path) else {
                    continue;
                };
                for file in files.flatten() {
                    let path = file.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let Ok(contents) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    for line in contents.lines() {
                        if let Some((dt_utc, rate_limits)) =
                            extract_rate_limits_from_json_line(line)
                        {
                            if newest.as_ref().is_none_or(|(prev, _)| dt_utc > *prev) {
                                newest = Some((dt_utc, rate_limits));
                            }
                            continue;
                        }

                        let Ok(event) = serde_json::from_str::<CodexSessionEvent>(line) else {
                            continue;
                        };
                        if event.event_type != "event_msg" {
                            continue;
                        }
                        let Some(payload) = event.payload else {
                            continue;
                        };
                        if payload.payload_type.as_deref() != Some("token_count") {
                            continue;
                        }
                        let Some(rate_limits) = payload.rate_limits else {
                            continue;
                        };
                        let Some(ts) = event.timestamp else {
                            continue;
                        };
                        let Ok(dt_fixed) = chrono::DateTime::parse_from_rfc3339(&ts) else {
                            continue;
                        };
                        let dt_utc = dt_fixed.with_timezone(&chrono::Utc);
                        if newest.as_ref().is_none_or(|(prev, _)| dt_utc > *prev) {
                            newest = Some((dt_utc, rate_limits));
                        }
                    }
                }
            }
        }
    }

    newest.map(|(_, limits)| limits)
}

fn codex_sessions_root() -> Option<std::path::PathBuf> {
    let from_dirs = dirs::home_dir().map(|h| h.join(".codex").join("sessions"));
    if from_dirs.as_ref().is_some_and(|p| p.exists()) {
        return from_dirs;
    }

    let from_env = std::env::var("HOME")
        .ok()
        .map(std::path::PathBuf::from)
        .map(|h| h.join(".codex").join("sessions"));
    if from_env.as_ref().is_some_and(|p| p.exists()) {
        return from_env;
    }

    let from_user_home = std::env::var("USER")
        .ok()
        .map(|u| std::path::PathBuf::from("/home").join(u))
        .map(|h| h.join(".codex").join("sessions"));
    if from_user_home.as_ref().is_some_and(|p| p.exists()) {
        return from_user_home;
    }

    from_dirs.or(from_env).or(from_user_home)
}

fn extract_rate_limits_from_json_line(
    line: &str,
) -> Option<(chrono::DateTime<chrono::Utc>, CodexRateLimits)> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("type")?.as_str()? != "event_msg" {
        return None;
    }

    let payload = value.get("payload")?;
    if payload.get("type")?.as_str()? != "token_count" {
        return None;
    }

    let rl = payload.get("rate_limits")?;
    if rl.is_null() {
        return None;
    }

    let parse_window = |w: &Value| -> Option<CodexRateLimitWindow> {
        Some(CodexRateLimitWindow {
            used_percent: w.get("used_percent").and_then(|v| v.as_f64()),
            window_minutes: w.get("window_minutes").and_then(|v| v.as_u64()),
            resets_at: w.get("resets_at").and_then(|v| v.as_i64()),
        })
    };

    let limits = CodexRateLimits {
        primary: rl.get("primary").and_then(parse_window),
        secondary: rl.get("secondary").and_then(parse_window),
        plan_type: rl
            .get("plan_type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    };

    let ts = value.get("timestamp")?.as_str()?;
    let dt_utc = chrono::DateTime::parse_from_rfc3339(ts)
        .ok()?
        .with_timezone(&chrono::Utc);
    Some((dt_utc, limits))
}
