//! Per-machine host resource snapshot carried by the daemon heartbeat.
//!
//! The webui pins a CPU / memory / disk gauge for a machine in its header off
//! this. Every field is serde-defaulted so an older daemon (missing block) and
//! an older server (unknown block) keep interoperating.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Last-known host resource usage of one machine. Percentages are 0..=100.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MachineResources {
    /// CPU busy share over the last heartbeat interval, 0..=100, across all
    /// cores (a 4-core box with one saturated core reads 25).
    #[serde(default)]
    pub cpu_pct: f32,
    /// Memory in use (total minus the kernel's `MemAvailable`), 0..=100.
    #[serde(default)]
    pub mem_pct: f32,
    #[serde(default)]
    pub mem_used_bytes: u64,
    #[serde(default)]
    pub mem_total_bytes: u64,
    /// Fill of the filesystem holding the daemon's home directory, 0..=100.
    #[serde(default)]
    pub disk_pct: f32,
    #[serde(default)]
    pub disk_used_bytes: u64,
    #[serde(default)]
    pub disk_total_bytes: u64,
    /// The path the disk figures were sampled at (the daemon's home dir).
    #[serde(default)]
    pub disk_path: String,
    /// 1-minute load average when the host reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load1: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_object_deserializes_to_default() {
        let r: MachineResources = serde_json::from_str("{}").unwrap();
        assert_eq!(r, MachineResources::default());
    }

    #[test]
    fn round_trips() {
        let r = MachineResources {
            cpu_pct: 12.5,
            mem_pct: 41.0,
            mem_used_bytes: 4,
            mem_total_bytes: 10,
            disk_pct: 78.0,
            disk_used_bytes: 78,
            disk_total_bytes: 100,
            disk_path: "/home/x".into(),
            load1: Some(0.5),
        };
        let back: MachineResources =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }
}
