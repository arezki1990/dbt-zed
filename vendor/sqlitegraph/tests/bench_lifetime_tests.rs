#[path = "../benches/bench_utils.rs"]
mod bench_utils;

use bench_utils::BenchmarkState;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct DropMarker {
    name: &'static str,
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl DropMarker {
    fn new(name: &'static str, log: Arc<Mutex<Vec<&'static str>>>) -> Self {
        Self { name, log }
    }
}

impl Drop for DropMarker {
    fn drop(&mut self) {
        self.log.lock().unwrap().push(self.name);
    }
}

#[test]
fn benchmark_state_drops_backend_before_temp_dir() {
    let log = Arc::new(Mutex::new(Vec::new()));

    {
        let _state = BenchmarkState {
            backend: DropMarker::new("backend", Arc::clone(&log)),
            temp_dir: DropMarker::new("temp", Arc::clone(&log)),
        };
    }

    let observed = log.lock().unwrap().clone();
    assert_eq!(observed, vec!["backend", "temp"]);
}
