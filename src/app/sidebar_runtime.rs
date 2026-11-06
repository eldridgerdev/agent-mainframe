use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar as StdCondvar, Mutex as StdMutex};

use super::SidebarLoadResult;

type SidebarLoadTask = Box<dyn FnOnce() + Send + 'static>;

pub(super) struct SidebarRuntime {
    pub(super) sidebar_load_tx: Sender<SidebarLoadResult>,
    pub(super) sidebar_load_rx: Receiver<SidebarLoadResult>,
    pub(super) sidebar_load_executor: Option<SidebarLoadExecutor>,
    pub(super) sidebar_load_signatures: HashMap<String, u64>,
    pub(super) pending_sidebar_loads: HashSet<String>,
}

impl SidebarRuntime {
    pub(super) fn new() -> Self {
        let (sidebar_load_tx, sidebar_load_rx) = std::sync::mpsc::channel();
        Self {
            sidebar_load_tx,
            sidebar_load_rx,
            sidebar_load_executor: None,
            sidebar_load_signatures: HashMap::new(),
            pending_sidebar_loads: HashSet::new(),
        }
    }
}

struct SidebarLoadQueue {
    tasks: VecDeque<SidebarLoadTask>,
    stopping: bool,
}

pub(super) struct SidebarLoadExecutor {
    queue: Arc<(StdMutex<SidebarLoadQueue>, StdCondvar)>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl SidebarLoadExecutor {
    const WORKER_COUNT: usize = 4;

    pub(super) fn new() -> Self {
        Self::with_worker_count(Self::WORKER_COUNT)
    }

    pub(super) fn with_worker_count(worker_count: usize) -> Self {
        assert!(worker_count > 0);
        let queue = Arc::new((
            StdMutex::new(SidebarLoadQueue {
                tasks: VecDeque::new(),
                stopping: false,
            }),
            StdCondvar::new(),
        ));
        let workers = (0..worker_count)
            .map(|index| {
                let queue = Arc::clone(&queue);
                std::thread::Builder::new()
                    .name(format!("sidebar-loader-{index}"))
                    .spawn(move || {
                        loop {
                            let task = {
                                let (lock, ready) = &*queue;
                                let mut state = lock.lock().unwrap();
                                while state.tasks.is_empty() && !state.stopping {
                                    state = ready.wait(state).unwrap();
                                }
                                if state.tasks.is_empty() && state.stopping {
                                    return;
                                }
                                state.tasks.pop_front()
                            };
                            if let Some(task) = task {
                                task();
                            }
                        }
                    })
                    .expect("failed to start sidebar loader worker")
            })
            .collect();
        Self { queue, workers }
    }

    pub(super) fn submit(&self, task: SidebarLoadTask) {
        let (lock, ready) = &*self.queue;
        let mut state = lock.lock().unwrap();
        state.tasks.push_back(task);
        ready.notify_one();
    }
}

impl Drop for SidebarLoadExecutor {
    fn drop(&mut self) {
        let (lock, ready) = &*self.queue;
        let mut state = lock.lock().unwrap();
        state.stopping = true;
        state.tasks.clear();
        drop(state);
        ready.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
