use std::collections::BTreeSet;

use anyhow::Result;
use dpp_core::{LevelInfo, LockFile, WorldPaths};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrderEntry {
    pub id: String,
    pub install_name: String,
    pub load_weight: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrderReport {
    pub expected: Vec<OrderEntry>,
    pub observed: Option<Vec<String>>,
    pub status: String,
    pub missing: Vec<String>,
    pub disabled: Vec<String>,
    pub warnings: Vec<String>,
    pub actions: Vec<String>,
}

/// Report the advisory load order without changing `level.dat` or running commands.
///
/// Observed order is read from `DataPacks.Enabled` only when the world does not
/// look open. A `session.lock` is treated as "may be running"; the command then
/// reports `unknown` instead of guessing an order from `/datapack list`.
pub fn order_world(world: &WorldPaths) -> Result<OrderReport> {
    let packages = LockFile::load_optional(&world.lock())?
        .map(|lock| lock.packages)
        .unwrap_or_default();
    let mut expected: Vec<OrderEntry> = packages
        .into_iter()
        .map(|package| OrderEntry {
            id: package.id,
            install_name: package.install_name,
            load_weight: package.load_weight,
        })
        .collect();
    expected.sort_by(|left, right| {
        left.load_weight
            .cmp(&right.load_weight)
            .then_with(|| left.id.cmp(&right.id))
            .then_with(|| left.install_name.cmp(&right.install_name))
    });

    let expected_names: Vec<String> = expected
        .iter()
        .map(|package| package.install_name.clone())
        .collect();
    let mut warnings = Vec::new();
    let mut seen = BTreeSet::new();
    for name in &expected_names {
        if !seen.insert(name.clone()) {
            warnings.push(format!("duplicate install_name in lock: {name}"));
        }
    }

    let mut observed: Option<Vec<String>> = None;
    let mut disabled = Vec::new();
    if world.session_lock().is_file() {
        warnings.push(
            "session.lock present; world may be running, so actual load order is unknown. \
             Run `/datapack list enabled` in game; `order` does not guess an order."
                .into(),
        );
    } else {
        let level = LevelInfo::load(&world.level_dat())?;
        match level.enabled_packs {
            Some(enabled) => {
                let managed: BTreeSet<&str> = expected_names.iter().map(String::as_str).collect();
                observed = Some(
                    enabled
                        .iter()
                        .filter_map(|name| name.strip_prefix("file/"))
                        .filter(|name| managed.contains(name))
                        .map(ToOwned::to_owned)
                        .collect(),
                );
                let expected: BTreeSet<&str> = expected_names.iter().map(String::as_str).collect();
                disabled = level
                    .disabled_packs
                    .iter()
                    .filter_map(|name| name.strip_prefix("file/"))
                    .filter(|name| expected.contains(name))
                    .map(ToOwned::to_owned)
                    .collect();
            }
            None => warnings.push(
                "level.dat has no DataPacks.Enabled list; actual load order is unknown".into(),
            ),
        }
    }

    let missing: Vec<String> = observed
        .as_ref()
        .map(|observed| {
            expected_names
                .iter()
                .filter(|name| !observed.contains(name))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let matches = observed
        .as_ref()
        .is_some_and(|observed| observed == &expected_names);
    let status = if observed.is_none() {
        "unknown"
    } else if matches {
        "match"
    } else {
        "drift"
    };

    let mut actions = Vec::new();
    let mut action_set = BTreeSet::new();
    if let Some(observed) = &observed {
        for name in &missing {
            let action = format!("/datapack enable \"file/{name}\"");
            if action_set.insert(action.clone()) {
                actions.push(action);
            }
        }
        for pair in expected_names.windows(2) {
            let (Some(left), Some(right)) = (
                observed.iter().position(|name| name == &pair[0]),
                observed.iter().position(|name| name == &pair[1]),
            ) else {
                continue;
            };
            if left > right {
                let action = format!(
                    "/datapack enable \"file/{}\" before \"file/{}\"",
                    pair[0], pair[1]
                );
                if action_set.insert(action.clone()) {
                    actions.push(action);
                }
            }
        }
    }
    if !disabled.is_empty() {
        warnings.push(
            "disabled managed packs are reported but never enabled automatically; review them first"
                .into(),
        );
    }

    Ok(OrderReport {
        expected,
        observed,
        status: status.into(),
        missing,
        disabled,
        warnings,
        actions,
    })
}
