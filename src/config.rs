use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How the task list is ordered.
///
/// `Manual` is Google's own `position` ordering, which the API treats as an
/// opaque lexicographic string; drag-to-reorder in the UI drives the
/// `tasks.move` endpoint. Every other mode is a purely local sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortMode {
    #[default]
    Manual,
    DueDate,
    Alphabetical,
    Created,
    Updated,
    Priority,
}

impl SortMode {
    pub const ALL: [SortMode; 6] = [
        SortMode::Manual,
        SortMode::DueDate,
        SortMode::Alphabetical,
        SortMode::Created,
        SortMode::Updated,
        SortMode::Priority,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortMode::Manual => "Manual",
            SortMode::DueDate => "Due date",
            SortMode::Alphabetical => "Alphabetical",
            SortMode::Created => "Date created",
            SortMode::Updated => "Last updated",
            SortMode::Priority => "Priority",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            SortMode::Manual => "manual",
            SortMode::DueDate => "due_date",
            SortMode::Alphabetical => "alphabetical",
            SortMode::Created => "created",
            SortMode::Updated => "updated",
            SortMode::Priority => "priority",
        }
    }
}

/// How the task list is sectioned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupMode {
    #[default]
    None,
    ByList,
    ByDue,
    ByStatus,
}

impl GroupMode {
    pub const ALL: [GroupMode; 4] = [
        GroupMode::None,
        GroupMode::ByList,
        GroupMode::ByDue,
        GroupMode::ByStatus,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GroupMode::None => "None",
            GroupMode::ByList => "Task list",
            GroupMode::ByDue => "Due date",
            GroupMode::ByStatus => "Status",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            GroupMode::None => "none",
            GroupMode::ByList => "by_list",
            GroupMode::ByDue => "by_due",
            GroupMode::ByStatus => "by_status",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Config {
    pub sort_mode: SortMode,
    pub group_mode: GroupMode,
    /// Minutes between background syncs when the app is running.
    pub poll_interval_minutes: u32,
    /// Local hour of day (0-23) at which "due today" notifications fire.
    pub notify_hour: u32,
    /// Notify for tasks that have just become overdue.
    pub notify_overdue: bool,
    /// Keep the app running in the tray after the window is closed.
    pub close_to_tray: bool,
    /// Launch hidden to the tray on autostart.
    pub start_hidden: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sort_mode: SortMode::default(),
            group_mode: GroupMode::default(),
            poll_interval_minutes: 5,
            notify_hour: 9,
            notify_overdue: true,
            close_to_tray: true,
            start_hidden: false,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("gtaskbar")
            .join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::path();
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str(&raw) {
            Ok(config) => config,
            Err(err) => {
                log::warn!("{} is invalid ({err}); using defaults", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&path, body)
    }
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gtaskbar")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let config = Config::default();
        assert_eq!(config.poll_interval_minutes, 5);
        assert_eq!(config.notify_hour, 9);
        assert!(config.notify_overdue);
        assert!(config.close_to_tray);
    }

    #[test]
    fn round_trips_through_toml() {
        let config = Config {
            sort_mode: SortMode::Priority,
            group_mode: GroupMode::ByDue,
            notify_hour: 7,
            ..Default::default()
        };

        let encoded = toml::to_string_pretty(&config).expect("serialise");
        let decoded: Config = toml::from_str(&encoded).expect("deserialise");

        assert_eq!(decoded.sort_mode, SortMode::Priority);
        assert_eq!(decoded.group_mode, GroupMode::ByDue);
        assert_eq!(decoded.notify_hour, 7);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let decoded: Config = toml::from_str("notify_hour = 6").expect("deserialise");
        assert_eq!(decoded.notify_hour, 6);
        // Everything not present in the TOML must come from Default.
        let defaults = Config::default();
        assert_eq!(decoded.sort_mode, defaults.sort_mode);
        assert_eq!(decoded.group_mode, defaults.group_mode);
        assert_eq!(
            decoded.poll_interval_minutes,
            defaults.poll_interval_minutes
        );
        assert_eq!(decoded.notify_overdue, defaults.notify_overdue);
        assert_eq!(decoded.close_to_tray, defaults.close_to_tray);
    }

    #[test]
    fn sort_mode_ids_are_unique() {
        let mut ids: Vec<_> = SortMode::ALL.iter().map(|m| m.id()).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "sort mode ids must be unique");
    }

    #[test]
    fn group_mode_ids_are_unique() {
        let mut ids: Vec<_> = GroupMode::ALL.iter().map(|m| m.id()).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "group mode ids must be unique");
    }
}
