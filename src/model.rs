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
    #[serde(rename = "docker")]
    Docker,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceKind {
    Process,
    Application,
    Container,
    Infra,
    Host,
}

impl ResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::Application => "application",
            Self::Container => "container",
            Self::Infra => "infra",
            Self::Host => "host",
        }
    }
}

#[derive(Debug, Clone, Eq)]
pub struct ProcessKey {
    pub environment: EnvironmentKind,
    pub source: Option<String>,
    pub pid: u32,
    pub start_id: u64,
}

impl PartialEq for ProcessKey {
    fn eq(&self, other: &Self) -> bool {
        self.environment == other.environment
            && self.source == other.source
            && self.pid == other.pid
            && self.start_id == other.start_id
    }
}

impl Hash for ProcessKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.environment.hash(state);
        self.source.hash(state);
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub kind: ResourceKind,
    /// Stable identifier in the resource's native namespace.
    /// Processes use their decimal PID; containers use their full container ID.
    pub id: String,
    /// Present for process rows and null for non-process resources.
    pub pid: Option<u32>,
    /// Process creation identity used internally to reject PID reuse.
    #[serde(skip)]
    pub start_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ppid: Option<u32>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    pub cpu_percent: f64,
    /// Cumulative CPU time consumed by this resource, in seconds, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_time_seconds: Option<f64>,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ContainerProcessUsage {
    pub resource: ResourceUsage,
    /// Process observations collected inside the Docker daemon's PID namespace.
    pub processes: Vec<ResourceUsage>,
    /// Host PIDs are only populated after the daemon has been proven to share
    /// the current WSL host PID namespace.
    pub host_pids: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowsApplicationUsage {
    pub resource: ResourceUsage,
    pub processes: Vec<ResourceUsage>,
}

#[cfg(test)]
mod tests {
    use super::{EnvironmentKind, ResourceKind, ResourceUsage};

    #[test]
    fn serializes_infra_kind_for_json_output() {
        assert_eq!(
            serde_json::to_string(&ResourceKind::Infra).unwrap(),
            "\"infra\""
        );
    }

    #[test]
    fn process_generation_is_not_added_to_json_compatibility_rows() {
        let row = ResourceUsage {
            environment: EnvironmentKind::Windows,
            source: None,
            kind: ResourceKind::Process,
            id: "42".into(),
            pid: Some(42),
            start_id: Some(123),
            ppid: None,
            name: "Code".into(),
            args: None,
            cpu_percent: 1.0,
            cpu_time_seconds: None,
            memory_bytes: 1,
        };
        let value = serde_json::to_value(row).unwrap();
        assert!(value.get("start_id").is_none());
        assert!(value.get("cpu_time_seconds").is_none());
    }
}
