use std::time::{Duration, Instant};

use crate::app::App;
use crate::app::PendingInput;

/// Most toasts shown at once.
pub const MAX_TOASTS: usize = 3;

pub enum ToastKind {
    Success,
    Info,
    Warning,
    Error,
}

impl ToastKind {
    fn severity(&self) -> u8 {
        match self {
            ToastKind::Success => 0,
            ToastKind::Info => 1,
            ToastKind::Warning => 2,
            ToastKind::Error => 3,
        }
    }

    pub fn default_duration(&self) -> Duration {
        match self {
            ToastKind::Success => Duration::from_secs(3),
            ToastKind::Info => Duration::from_secs(4),
            ToastKind::Warning => Duration::from_secs(6),
            ToastKind::Error => Duration::from_secs(8),
        }
    }
}

pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    created_at: Instant,
    duration: Duration,
}

impl Toast {
    pub fn new(message: impl Into<String>, kind: ToastKind) -> Self {
        let duration = kind.default_duration();
        Self {
            message: message.into(),
            kind,
            created_at: Instant::now(),
            duration,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= self.duration
    }

    /// 0.0 = just created, 1.0 = expired. Use to dim text near expiry.
    pub fn age_fraction(&self) -> f32 {
        let elapsed = self.created_at.elapsed().as_secs_f32();
        let total = self.duration.as_secs_f32();
        (elapsed / total).clamp(0.0, 1.0)
    }
}

impl App {
    pub fn push_toast(&mut self, msg: impl Into<String>, kind: ToastKind) {
        // Cap the stack so a burst cannot grow past what the renderer can show
        // above the status bar. Expired toasts go first, then the oldest of the
        // lowest severity, so a burst of success/info toasts cannot push out a
        // live error.
        self.toasts.retain(|t| !t.is_expired());
        while self.toasts.len() >= MAX_TOASTS {
            let victim = self
                .toasts
                .iter()
                .enumerate()
                .min_by_key(|(i, t)| (t.kind.severity(), *i))
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.toasts.remove(victim);
        }
        self.toasts.push(Toast::new(msg, kind));
    }

    pub fn push_toast_success(&mut self, msg: impl Into<String>) {
        self.push_toast(msg, ToastKind::Success);
    }

    pub fn push_toast_info(&mut self, msg: impl Into<String>) {
        self.push_toast(msg, ToastKind::Info);
    }

    pub fn push_toast_warning(&mut self, msg: impl Into<String>) {
        self.push_toast(msg, ToastKind::Warning);
    }

    /// Error toasts carry their own marker (red border and the ✕ title), so a
    /// leading "Error:" is dropped rather than doubled up with it.
    pub fn push_toast_error(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        let msg = crate::app::strip_error_prefix(&msg).to_string();
        self.push_toast(msg, ToastKind::Error);
    }

    /// Move the shared status message into an expiring toast.
    ///
    /// Most modes render `message` in their own status or validation area,
    /// but pane view has no persistent AMF status line. Consuming it here
    /// keeps pane feedback visible without leaving text over the harness.
    pub fn promote_message_to_toast(&mut self) {
        let Some(message) = self.message.take() else {
            return;
        };

        let kind = if message.starts_with("Error:") || message.starts_with("Clipboard error:") {
            ToastKind::Error
        } else {
            ToastKind::Success
        };
        self.push_toast(message, kind);
    }

    /// Remove expired toasts. Returns true if any were removed (caller sets force_redraw).
    pub fn tick_toasts(&mut self) -> bool {
        let before = self.toasts.len();
        self.toasts.retain(|t| !t.is_expired());
        self.toasts.len() != before
    }

    pub fn has_active_toasts(&self) -> bool {
        !self.toasts.is_empty()
    }
}

pub(crate) fn input_request_toast_message(input: &PendingInput) -> String {
    let target = input
        .feature_name
        .as_deref()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            input
                .project_name
                .as_deref()
                .filter(|value| !value.is_empty())
        })
        .map(str::to_string)
        .unwrap_or_else(|| {
            if input.session_id.is_empty() {
                "unknown session".to_string()
            } else {
                input.session_id.clone()
            }
        });

    let label = match input.notification_type.as_str() {
        "diff-review" | "change-reason" => "diff review",
        "review-ready" => "fixes ready",
        _ => "input request",
    };
    format!("New {label} from {target}")
}

#[cfg(test)]
mod eviction_tests {
    use super::*;

    #[test]
    fn a_burst_of_low_severity_toasts_keeps_the_live_error() {
        let mut app = App::new_for_test(
            crate::project::ProjectStore::empty(),
            Box::new(crate::traits::MockTmuxOps::new()),
            Box::new(crate::traits::MockWorktreeOps::new()),
        );
        app.push_toast_error("kept");
        for i in 0..5 {
            app.push_toast_success(format!("ok {i}"));
        }
        assert!(app.toasts.len() <= MAX_TOASTS);
        assert!(app.toasts.iter().any(|t| t.message == "kept"));
    }

    #[test]
    fn strip_error_prefix_handles_multibyte_openings() {
        assert_eq!(crate::app::strip_error_prefix("✕é… boom"), "✕é… boom");
        assert_eq!(crate::app::strip_error_prefix("→ x"), "→ x");
        assert_eq!(crate::app::strip_error_prefix("Error: Error: x"), "x");
    }

    #[test]
    fn actionable_match_uses_word_boundaries() {
        assert!(crate::app::App::logged_error_message("compress failed").contains("Press D"));
        assert!(
            !crate::app::App::logged_error_message("Could not save; retry").contains("Press D")
        );
    }
}
