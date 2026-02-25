use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Claude,
    Zai,
}

impl Model {
    pub fn label(&self) -> &'static str {
        match self {
            Model::Claude => "claude",
            Model::Zai => "zai",
        }
    }

    pub fn all() -> &'static [Model] {
        &[Model::Claude, Model::Zai]
    }

    pub fn next(&self) -> Model {
        match self {
            Model::Claude => Model::Zai,
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

#[derive(Debug, Clone)]
pub struct UsageData {
    pub visible_model: Model,
    pub claude: ClaudeUsageData,
    pub zai: ZaiUsageData,
}

impl Default for UsageData {
    fn default() -> Self {
        Self {
            visible_model: Model::Claude,
            claude: ClaudeUsageData::default(),
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

pub struct UsageManager {
    data: Arc<Mutex<UsageData>>,
    last_stats_refresh: Option<Instant>,
    last_oauth_refresh: Option<Instant>,
    last_cycle: Instant,
    cycle_interval_secs: u64,
    zai_monthly_limit: Option<u64>,
    zai_weekly_limit: Option<u64>,
    zai_five_hour_limit: Option<u64>,
}

impl UsageManager {
    pub fn new(
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
        data.visible_model = data.visible_model.next();
    }

    pub fn should_cycle(&self) -> bool {
        self.last_cycle.elapsed().as_secs() >= self.cycle_interval_secs
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
            self.last_stats_refresh = Some(now);
        }

        let should_refresh_oauth = self
            .last_oauth_refresh
            .map(|t| now.duration_since(t).as_secs() >= 60)
            .unwrap_or(true);

        if should_refresh_oauth {
            self.last_oauth_refresh = Some(now);
            let data = Arc::clone(&self.data);
            let monthly = self.zai_monthly_limit;
            let weekly = self.zai_weekly_limit;
            let five_hour = self.zai_five_hour_limit;
            std::thread::spawn(move || {
                fetch_rate_limits(&data);
                fetch_zai_usage(&data, monthly, weekly, five_hour);
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
}

fn fetch_rate_limits(data: &Arc<Mutex<UsageData>>) {
    let Some(claude_dir) = dirs::home_dir().map(|h| h.join(".claude")) else {
        return;
    };

    let creds_path = claude_dir.join(".credentials.json");
    let Ok(contents) = std::fs::read_to_string(&creds_path) else {
        return;
    };

    let Ok(creds) = serde_json::from_str::<Credentials>(&contents) else {
        return;
    };

    let Some(oauth) = creds.claude_ai_oauth else {
        return;
    };

    {
        let mut d = data.lock().unwrap();
        d.claude.subscription_type = oauth.subscription_type;
    }

    let result = ureq::get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", &format!("Bearer {}", oauth.access_token))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "claude-code/2.1.42")
        .header("Content-Type", "application/json")
        .call();

    match result {
        Ok(mut response) => {
            let body = match response.body_mut().read_to_string() {
                Ok(b) => b,
                Err(e) => {
                    let mut d = data.lock().unwrap();
                    d.claude.last_error = Some(format!("Read error: {}", e));
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
                }
                Err(e) => {
                    let mut d = data.lock().unwrap();
                    d.claude.last_error = Some(format!("Parse error: {}", e));
                }
            }
        }
        Err(e) => {
            let mut d = data.lock().unwrap();
            d.claude.last_error = Some(format!("HTTP error: {}", e));
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
