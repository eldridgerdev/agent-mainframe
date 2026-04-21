use std::time::{Duration, Instant};

use crate::app::App;

pub enum ToastKind {
    Success,
    Info,
    Warning,
    Error,
}

impl ToastKind {
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

    pub fn push_toast_error(&mut self, msg: impl Into<String>) {
        self.push_toast(msg, ToastKind::Error);
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
