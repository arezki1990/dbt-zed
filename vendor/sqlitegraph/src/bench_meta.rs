#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchRun {
    pub name: String,
    pub mean_ns: u64,
    pub samples: u32,
}

impl BenchRun {
    pub fn summary(&self) -> String {
        format!(
            "{}: mean={}ns samples={}",
            self.name, self.mean_ns, self.samples
        )
    }

    pub fn within_threshold(&self, max_ns: u64) -> bool {
        self.mean_ns <= max_ns
    }

    pub fn within_regression(&self, baseline: &BenchRun, tolerance: f64) -> bool {
        if self.name != baseline.name {
            return false;
        }
        let allowed = (baseline.mean_ns as f64) * (1.0 + tolerance);
        (self.mean_ns as f64) <= allowed
    }
}
