// Copyright (c) 2019 Kata Containers community
// Copyright (c) 2025 NVIDIA Corporation
//
// SPDX-License-Identifier: Apache-2.0

use super::client as k8s;
use crate::config::Config;
use anyhow::Result;
use log::{debug, info, warn};
use std::collections::BTreeMap;

pub async fn update_existing_runtimeclasses_for_nfd(config: &Config) -> Result<()> {
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;

    info!("Checking existing runtime classes for NFD updates");

    let existing_runtimeclasses = k8s::list_runtimeclasses(config).await?;

    for rc in existing_runtimeclasses {
        let name = rc
            .metadata
            .name
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("RuntimeClass missing name"))?;

        if !name.starts_with("kata") {
            continue;
        }

        let nfd_key = if name.contains("tdx") {
            Some("tdx.intel.com/keys")
        } else if name.contains("snp") {
            Some("sev-snp.amd.com/esids")
        } else {
            None
        };

        if nfd_key.is_none() {
            continue;
        }

        let nfd_key = nfd_key.unwrap();

        // Only update if the RuntimeClass is missing the NFD field
        // Check if NFD key already exists in overhead.podFixed
        let needs_update = if let Some(ref overhead) = rc.overhead {
            if let Some(ref pod_fixed) = overhead.pod_fixed {
                // Field exists, check if the key is missing
                !pod_fixed.contains_key(nfd_key)
            } else {
                // overhead exists but podFixed is missing, needs update
                true
            }
        } else {
            // overhead is missing, needs update
            true
        };

        if !needs_update {
            info!("RuntimeClass {name} already has NFD key {nfd_key}, skipping");
            continue;
        }

        info!("Updating existing RuntimeClass {name} with missing NFD key {nfd_key}");

        let mut patched_rc = rc.clone();

        if patched_rc.overhead.is_none() {
            patched_rc.overhead = Some(Default::default());
        }

        if let Some(ref mut overhead) = patched_rc.overhead {
            if overhead.pod_fixed.is_none() {
                overhead.pod_fixed = Some(Default::default());
            }

            if let Some(ref mut pod_fixed) = overhead.pod_fixed {
                let quantity = Quantity("1".to_string());
                pod_fixed.insert(nfd_key.to_string(), quantity);
            }
        }

        k8s::update_runtimeclass(config, &patched_rc).await?;
        info!("Successfully updated RuntimeClass {name} with NFD key {nfd_key}");
    }

    Ok(())
}

/// Record each RuntimeClass' guest kernel command line on the RuntimeClass
/// itself, under `annotation_key`.
///
/// RuntimeClasses are created by the Helm chart, which cannot know a command
/// line that only exists once the configurations have been rendered onto a node;
/// hence annotating them here instead. Ones that are absent are left alone
/// rather than created: whether a RuntimeClass should exist is the chart's
/// decision, not this one's.
pub async fn annotate_guest_kernel_cmdlines(
    config: &Config,
    annotation_key: &str,
    cmdline_by_runtime_class: &BTreeMap<String, String>,
) -> Result<()> {
    let existing = k8s::list_runtimeclasses(config).await?;

    let mut annotated = 0;
    let mut unchanged = 0;
    let mut found = Vec::new();

    for rc in existing {
        let Some(name) = rc.metadata.name.as_deref() else {
            continue;
        };
        let Some(encoded_cmdline) = cmdline_by_runtime_class.get(name) else {
            continue;
        };
        found.push(name.to_string());

        if rc
            .metadata
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.get(annotation_key))
            == Some(encoded_cmdline)
        {
            unchanged += 1;
            continue;
        }

        k8s::patch_runtimeclass_annotation(config, name, annotation_key, encoded_cmdline).await?;
        debug!("Annotated RuntimeClass {name} with its guest kernel command line");
        annotated += 1;
    }

    info!(
        "Guest kernel command line annotation: {annotated} RuntimeClass(es) updated, \
         {unchanged} already current"
    );

    // A RuntimeClass this node installed but which does not exist in the cluster
    // means the node and the chart disagree about what should exist. Not this
    // step's to fix, but worth saying out loud.
    let absent: Vec<&str> = cmdline_by_runtime_class
        .keys()
        .map(String::as_str)
        .filter(|name| !found.iter().any(|f| f == name))
        .collect();
    if !absent.is_empty() {
        warn!(
            "Installed configurations for RuntimeClass(es) that do not exist in the cluster, so \
             their guest kernel command line was not published: {}",
            absent.join(", ")
        );
    }

    Ok(())
}
