//! In-flight PR work. Closing a pane and cancelling a worker are separate actions.

use std::sync::mpsc::{Receiver, TryRecvError};

use super::{InvestigationOutcome, PrReview};
use crate::app::ai_review::AiReviewProgress;
use crate::app::{AiReviewRunProgress, AiReviewState};

/// One fetch and one investigation slot, preserving the original independent
/// cancellation and receiver-drop behavior. AppMode remains the result target.
#[derive(Default)]
pub(crate) struct PrReviewWork {
    fetch: Option<Receiver<anyhow::Result<PrReview>>>,
    investigation: Option<Receiver<InvestigationOutcome>>,
}

impl PrReviewWork {
    pub(crate) fn begin_fetch(&mut self, receiver: Receiver<anyhow::Result<PrReview>>) {
        self.fetch = Some(receiver);
    }

    pub(crate) fn fetch_pending(&self) -> bool {
        self.fetch.is_some()
    }

    pub(crate) fn poll_fetch(&self) -> Option<Result<anyhow::Result<PrReview>, TryRecvError>> {
        self.fetch.as_ref().map(Receiver::try_recv)
    }

    pub(crate) fn cancel_fetch(&mut self) {
        self.fetch = None;
    }

    pub(crate) fn begin_investigation(&mut self, receiver: Receiver<InvestigationOutcome>) {
        self.investigation = Some(receiver);
    }

    pub(crate) fn investigation_pending(&self) -> bool {
        self.investigation.is_some()
    }

    pub(crate) fn poll_investigation(&self) -> Option<Result<InvestigationOutcome, TryRecvError>> {
        self.investigation.as_ref().map(Receiver::try_recv)
    }

    pub(crate) fn cancel_investigation(&mut self) {
        self.investigation = None;
    }
}

/// The pending origin and live progress outlive the running dialog. Finishing
/// or invalidating a run clears all three together; merely closing a pane does not.
#[derive(Default)]
pub(crate) struct AiReviewRun {
    receiver: Option<Receiver<AiReviewProgress>>,
    origin: Option<AiReviewState>,
    progress: Option<AiReviewRunProgress>,
}

impl AiReviewRun {
    pub(crate) fn begin(&mut self, receiver: Receiver<AiReviewProgress>, origin: AiReviewState) {
        self.receiver = Some(receiver);
        self.origin = Some(origin);
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.receiver.is_some()
    }

    pub(crate) fn poll(&self) -> Result<AiReviewProgress, TryRecvError> {
        self.receiver
            .as_ref()
            .map_or(Err(TryRecvError::Disconnected), Receiver::try_recv)
    }

    pub(crate) fn origin(&self) -> &Option<AiReviewState> {
        &self.origin
    }

    pub(crate) fn progress(&self) -> &Option<AiReviewRunProgress> {
        &self.progress
    }

    pub(crate) fn progress_mut(&mut self) -> Option<&mut AiReviewRunProgress> {
        self.progress.as_mut()
    }

    pub(crate) fn show_progress(&mut self, progress: AiReviewRunProgress) {
        self.progress = Some(progress);
    }

    pub(crate) fn finish(&mut self) -> Option<AiReviewState> {
        self.receiver = None;
        self.progress = None;
        self.origin.take()
    }

    // A few existing regressions intentionally seed incomplete/disconnected
    // runs. Keep that injection test-only, outside the production lifecycle API.
    #[cfg(test)]
    pub(crate) fn set_receiver_for_test(&mut self, receiver: Option<Receiver<AiReviewProgress>>) {
        self.receiver = receiver;
    }

    #[cfg(test)]
    pub(crate) fn set_origin_for_test(&mut self, origin: Option<AiReviewState>) {
        self.origin = origin;
    }

    #[cfg(test)]
    pub(crate) fn set_progress_for_test(&mut self, progress: Option<AiReviewRunProgress>) {
        self.progress = progress;
    }
}
