use serde::Serialize;
use std::hash::{Hash, Hasher};
use std::time::Instant;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize)]
pub enum EnvironmentKind {
    #[serde(rename = "windows")]
    Windows,
    #[serde(rename = "wsl")]
    Wsl,
    #[serde(rename = "wslc")]
    WslContainer,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceKind {
    Process,
    Container,
}

#[derive(Debug, Clone, Eq)]
pub struct ProcessKey {
    pub environment: EnvironmentKind,
    pub pid: u32,
    pub start_id: u64,
}

impl PartialEq for ProcessKey {
    fn eq(&self, other: &Self) -> bool {
        self.environment == other.environment
            && self.pid == other.pid
            && self.start_id == other.start_id
    }
}

impl Hash for ProcessKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.environment.hash(state);
        self.pid.hash(state);
        self.start_id.hash(state);
    }
}

#[derive(Debug, Clone)]
pub struct ProcessSample {
    pub key: ProcessKey,
    pub name: String,
    pub cpu_time_secs: f64,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub captured_at: Instant,
    pub processes: Vec<ProcessSample>,
}

#[derive(Debug, Clone)]
pub struct WindowsSnapshot {
    pub snapshot: Snapshot,
    pub host_logical_cpu_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceUsage {
    pub environment: EnvironmentKind,
    pub kind: ResourceKind,
    /// Stable identifier in the resource's native namespace.
    /// Processes use their decimal PID; containers use their full container ID.
    pub id: String,
    /// Present for process rows and null for non-process resources.
    pub pid: Option<u32>,
    pub name: String,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
}
