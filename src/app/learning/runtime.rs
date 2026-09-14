//! Answer delivery survives overlay navigation; displayed session data stays in AppMode.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, channel};

use super::LearningAnswer;

pub(crate) struct LearningRuns {
    answers: Receiver<LearningAnswer>,
    sender: Sender<LearningAnswer>,
    in_flight: HashSet<String>,
}

impl Default for LearningRuns {
    fn default() -> Self {
        let (sender, answers) = channel();
        Self {
            answers,
            sender,
            in_flight: HashSet::new(),
        }
    }
}

impl LearningRuns {
    pub(crate) fn begin(&mut self, qa_id: String) -> Sender<LearningAnswer> {
        self.in_flight.insert(qa_id);
        self.sender.clone()
    }

    pub(crate) fn is_in_flight(&self, qa_id: &str) -> bool {
        self.in_flight.contains(qa_id)
    }

    pub(crate) fn next_answer(&mut self) -> Option<LearningAnswer> {
        let answer = self.answers.try_recv().ok()?;
        self.in_flight.remove(&answer.qa_id);
        Some(answer)
    }

    #[cfg(test)]
    pub(crate) fn sender(&self) -> Sender<LearningAnswer> {
        self.sender.clone()
    }

    #[cfg(test)]
    pub(crate) fn clear_in_flight_for_test(&mut self) {
        self.in_flight.clear();
    }
}
