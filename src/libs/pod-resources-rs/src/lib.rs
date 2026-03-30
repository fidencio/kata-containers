// Copyright (c) 2026 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

pub mod pod_resources;

use anyhow::{Result, anyhow};
use cdi::cache::{CdiOption, new_cache, with_auto_refresh};
use cdi::spec_dirs::with_spec_dirs;
use container_device_interface as cdi;

use slog::info;
use std::sync::Arc;
use tokio::time;

const DEFAULT_DYNAMIC_CDI_SPEC_PATH: &str = "/var/run/cdi";
const DEFAULT_STATIC_CDI_SPEC_PATH: &str = "/etc/cdi";

#[macro_export]
macro_rules! sl {
    () => {
        slog_scope::logger()
    };
}

/// Resolve CDI fully-qualified device names into host device paths.
///
/// Scans the CDI spec directories, looks up each FQN in the cache, and
/// returns the device node paths (e.g. "/dev/vfio/42") for injection.
pub async fn handle_cdi_devices(
    devices: &[String],
    _cdi_timeout: time::Duration,
) -> Result<Vec<String>> {
    if devices.is_empty() {
        info!(sl!(), "no pod CDI devices requested.");
        return Ok(vec![]);
    }

    let options: Vec<CdiOption> = vec![
        with_auto_refresh(false),
        with_spec_dirs(&[DEFAULT_DYNAMIC_CDI_SPEC_PATH, DEFAULT_STATIC_CDI_SPEC_PATH]),
    ];
    let cache: Arc<std::sync::Mutex<cdi::cache::Cache>> = new_cache(options);

    let paths = {
        let mut paths = vec![];
        let mut cache = cache.lock().unwrap();
        cache
            .refresh()
            .map_err(|e| anyhow!("Refreshing cache failed: {:?}", e))?;

        for dev in devices.iter() {
            info!(sl!(), "Requested CDI device with FQN: {}", dev);
            match cache.get_device(dev) {
                Some(device) => {
                    info!(
                        sl!(),
                        "Target CDI device: {}",
                        device.get_qualified_name()
                    );
                    if let Some(devnodes) = device.edits().container_edits.device_nodes {
                        for dn in &devnodes {
                            let json = serde_json::to_value(dn)
                                .map_err(|e| anyhow!("failed to serialize DeviceNode: {e}"))?;
                            if let Some(p) = json.get("path").and_then(|v| v.as_str()) {
                                paths.push(p.to_owned());
                            }
                        }
                    }
                }
                None => {
                    return Err(anyhow!(
                        "Failed to get device node for CDI device: {} in cache",
                        dev
                    ));
                }
            }
        }

        paths
    };
    info!(sl!(), "target CDI device paths to inject: {:?}", paths);

    Ok(paths)
}
