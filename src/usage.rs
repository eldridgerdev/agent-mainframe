use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::Instant;

// --- stats-cache.json models ---

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

// --- OAuth usage response models ---

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

// --- credentials.json models ---

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

// --- Aggregated data for the UI ---

#[derive(Debug, Clone, Default)]
pub struct UsageData {
    pub today_messages: u64,
    pub today_sessions: u64,
    pub today_tool_calls: u64,
    pub five_hour_pct: Option<f64>,
    pub seven_day_pct: Option<f64>,
    pub five_hour_resets: Option<String>,
    pub seven_day_resets: Option<String>,
    pub subscription_type: Option<String>,
    pub last_error: Option<String>,
}

pub struct UsageManager {
    data: Arc<Mutex<UsageData>>,
    last_stats_refresh: Option<Instant>,
    last_oauth_refresh: Option<Instant>,
}

impl UsageManager {
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(UsageData::default())),
            last_stats_refresh: None,
            last_oauth_refresh: None,
        }
    }

    pub fn get_data(&self) -> UsageData {
        self.data.lock().unwrap().clone()
    }

    pub fn refresh(&mut self) {
        let now = Instant::now();

        // Refresh local stats every 30s
        let should_refresh_stats = self
            .last_stats_refresh
            .map(|t| now.duration_since(t).as_secs() >= 30)
            .unwrap_or(true);

        if should_refresh_stats {
            self.refresh_stats_cache();
            self.last_stats_refresh = Some(now);
        }

        // Refresh OAuth rate limits every 60s (in background)
        let should_refresh_oauth = self
            .last_oauth_refresh
            .map(|t| now.duration_since(t).as_secs() >= 60)
            .unwrap_or(true);

        if should_refresh_oauth {
            self.last_oauth_refresh = Some(now);
            let data = Arc::clone(&self.data);
            std::thread::spawn(move || {
                fetch_rate_limits(&data);
            });
        }
    }

    fn refresh_stats_cache(&self) {
        let Some(claude_dir) = dirs::home_dir()
            .map(|h| h.join(".claude"))
        else {
            return;
        };

        let stats_path = claude_dir.join("stats-cache.json");
        let Ok(contents) =
            std::fs::read_to_string(&stats_path)
        else {
            return;
        };

        let Ok(cache) =
            serde_json::from_str::<StatsCache>(&contents)
        else {
            return;
        };

        let today = chrono::Local::now()
            .format("%Y-%m-%d")
            .to_string();

        let today_stats = cache
            .daily_activity
            .iter()
            .find(|d| d.date == today);

        let mut data = self.data.lock().unwrap();
        if let Some(stats) = today_stats {
            data.today_messages = stats.message_count;
            data.today_sessions = stats.session_count;
            data.today_tool_calls = stats.tool_call_count;
        } else {
            data.today_messages = 0;
            data.today_sessions = 0;
            data.today_tool_calls = 0;
        }
    }
}

fn fetch_rate_limits(data: &Arc<Mutex<UsageData>>) {
    let Some(claude_dir) =
        dirs::home_dir().map(|h| h.join(".claude"))
    else {
        return;
    };

    let creds_path = claude_dir.join(".credentials.json");
    let Ok(contents) =
        std::fs::read_to_string(&creds_path)
    else {
        return;
    };

    let Ok(creds) =
        serde_json::from_str::<Credentials>(&contents)
    else {
        return;
    };

    let Some(oauth) = creds.claude_ai_oauth else {
        return;
    };

    // Store subscription type
    {
        let mut d = data.lock().unwrap();
        d.subscription_type = oauth.subscription_type;
    }

    let result = ureq::get(
        "https://api.anthropic.com/api/oauth/usage",
    )
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
                    d.last_error =
                        Some(format!("Read error: {}", e));
                    return;
                }
            };

            match serde_json::from_str::<RateLimitResponse>(
                &body,
            ) {
                Ok(resp) => {
                    let mut d = data.lock().unwrap();
                    d.five_hour_pct = resp
                        .five_hour
                        .as_ref()
                        .map(|w| w.utilization);
                    d.five_hour_resets = resp
                        .five_hour
                        .as_ref()
                        .and_then(|w| w.resets_at.clone());
                    d.seven_day_pct = resp
                        .seven_day
                        .as_ref()
                        .map(|w| w.utilization);
                    d.seven_day_resets = resp
                        .seven_day
                        .as_ref()
                        .and_then(|w| w.resets_at.clone());
                    d.last_error = None;
                }
                Err(e) => {
                    let mut d = data.lock().unwrap();
                    d.last_error = Some(format!(
                        "Parse error: {}",
                        e
                    ));
                }
            }
        }
        Err(e) => {
            let mut d = data.lock().unwrap();
            d.last_error =
                Some(format!("HTTP error: {}", e));
        }
    }
}
