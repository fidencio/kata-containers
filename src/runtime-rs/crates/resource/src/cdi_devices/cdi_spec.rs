// Copyright (c) 2025 NVIDIA CORPORATION
//
// SPDX-License-Identifier: Apache-2.0
//

//! Minimal CDI (Container Device Interface) spec reader and OCI spec injector.
//!
//! Reads CDI spec files from the standard directories (`/etc/cdi` and
//! `/var/run/cdi`) and injects the device nodes for requested devices into
//! an OCI spec's `linux.devices` list.
//!
//! This is the Rust equivalent of the Go CDI library call
//! `config.InjectCDIDevices(ociSpec, devices)` used in the Go runtime.
//!
//! Only the fields needed for VFIO passthrough are parsed:
//! - `kind` (vendor.com/class, e.g. `nvidia.com/gpu`)
//! - `devices[].name`
//! - `devices[].containerEdits.deviceNodes[].{path, type, major, minor, fileMode}`

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use oci_spec::runtime::{LinuxDevice, LinuxDeviceBuilder, LinuxDeviceType, Spec};
use serde::Deserialize;

const CDI_SPEC_DIRS: &[&str] = &["/etc/cdi", "/var/run/cdi"];

// ---------------------------------------------------------------------------
// CDI spec data types (subset of the CDI specification)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdiSpec {
    kind: String,
    devices: Vec<CdiDevice>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdiDevice {
    name: String,
    container_edits: ContainerEdits,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContainerEdits {
    #[serde(default)]
    device_nodes: Vec<DeviceNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceNode {
    path: String,
    #[serde(rename = "type", default = "default_char_type")]
    typ: String,
    #[serde(default)]
    major: Option<i64>,
    #[serde(default)]
    minor: Option<i64>,
    #[serde(default)]
    file_mode: Option<u32>,
}

fn default_char_type() -> String {
    "c".to_string()
}

// ---------------------------------------------------------------------------
// CDI registry (lazy load of all spec files)
// ---------------------------------------------------------------------------

/// Load and parse all CDI spec files from the standard directories.
fn load_specs() -> Vec<CdiSpec> {
    let mut specs = Vec::new();
    for dir in CDI_SPEC_DIRS {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_cdi_file(&path) {
                continue;
            }
            match parse_spec_file(&path) {
                Ok(spec) => specs.push(spec),
                Err(e) => {
                    // Log and skip malformed files; don't abort the whole scan.
                    eprintln!("cdi_spec: skipping {:?}: {e}", path);
                }
            }
        }
    }
    specs
}

fn is_cdi_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("yaml") | Some("yml") | Some("json")
    )
}

fn parse_spec_file(path: &Path) -> Result<CdiSpec> {
    let data = fs::read_to_string(path)
        .with_context(|| format!("read CDI spec {}", path.display()))?;
    let spec: CdiSpec = if path.extension().and_then(|e| e.to_str()) == Some("json") {
        serde_json::from_str(&data)
            .with_context(|| format!("parse JSON CDI spec {}", path.display()))?
    } else {
        serde_yaml::from_str(&data)
            .with_context(|| format!("parse YAML CDI spec {}", path.display()))?
    };
    Ok(spec)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Inject device nodes from CDI specs into an OCI spec's `linux.devices`.
///
/// `devices` is a slice of fully-qualified CDI device names in the form
/// `"vendor.com/class=name"` (e.g. `"nvidia.com/gpu=0"`).
///
/// Device nodes found in the CDI spec files are appended to the existing
/// `linux.devices` list in `spec`.  If a device node path already appears in
/// the list it is skipped to avoid duplicates.
pub fn inject_cdi_devices(spec: &mut Spec, devices: &[String]) -> Result<()> {
    if devices.is_empty() {
        return Ok(());
    }

    let cdi_specs = load_specs();

    // Collect (kind, name) pairs we need to resolve.
    let requests: Vec<(String, String)> = devices
        .iter()
        .filter_map(|d| {
            let (kind, name) = d.split_once('=')?;
            Some((kind.to_owned(), name.to_owned()))
        })
        .collect();

    // Track paths already in the spec to avoid duplicates.
    let mut existing_paths: std::collections::HashSet<PathBuf> = spec
        .linux()
        .as_ref()
        .and_then(|l| l.devices().as_ref())
        .map(|devs| devs.iter().map(|d| PathBuf::from(d.path())).collect())
        .unwrap_or_default();

    let mut new_devices: Vec<LinuxDevice> = Vec::new();

    for (kind, name) in &requests {
        let mut found = false;
        'spec_loop: for cdi_spec in &cdi_specs {
            if &cdi_spec.kind != kind {
                continue;
            }
            for cdi_dev in &cdi_spec.devices {
                if &cdi_dev.name != name {
                    continue;
                }
                found = true;
                for node in &cdi_dev.container_edits.device_nodes {
                    let node_path = PathBuf::from(&node.path);
                    if existing_paths.contains(&node_path) {
                        continue;
                    }
                    existing_paths.insert(node_path.clone());

                    let linux_dev = build_linux_device(node)
                        .with_context(|| format!("build device for {kind}={name}"))?;
                    new_devices.push(linux_dev);
                }
                break 'spec_loop;
            }
        }
        if !found {
            return Err(anyhow::anyhow!(
                "CDI device {kind}={name} not found in spec files under {:?}",
                CDI_SPEC_DIRS
            ));
        }
    }

    if !new_devices.is_empty() {
        let linux = spec.linux_mut().get_or_insert_with(Default::default);
        let mut all_devices = linux
            .devices()
            .clone()
            .unwrap_or_default();
        all_devices.extend(new_devices);
        linux.set_devices(Some(all_devices));
    }

    Ok(())
}

/// Collect CDI device names from CDI annotations already on the OCI spec.
///
/// CDI annotations have the form `cdi.k8s.io/<qualifier>=<vendor/class=name>`.
/// This mirrors the Go runtime's `cdi.ParseAnnotations()` call.
pub fn devices_from_annotations(
    annotations: &std::collections::HashMap<String, String>,
) -> Vec<String> {
    const CDI_ANNOTATION_PREFIX: &str = "cdi.k8s.io/";
    annotations
        .iter()
        .filter(|(k, _)| k.starts_with(CDI_ANNOTATION_PREFIX))
        .map(|(_, v)| v.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_linux_device(node: &DeviceNode) -> Result<LinuxDevice> {
    let dev_type = match node.typ.as_str() {
        "c" | "u" => LinuxDeviceType::C,
        "b" => LinuxDeviceType::B,
        "p" => LinuxDeviceType::P,
        other => {
            return Err(anyhow::anyhow!("unknown CDI device type '{other}'"));
        }
    };

    // If major/minor are provided in the spec use them; otherwise stat the host file.
    let (major, minor) = match (node.major, node.minor) {
        (Some(maj), Some(min)) => (maj, min),
        _ => stat_major_minor(&node.path)?,
    };

    let mut builder = LinuxDeviceBuilder::default()
        .path(PathBuf::from(&node.path))
        .typ(dev_type)
        .major(major)
        .minor(minor);

    if let Some(mode) = node.file_mode {
        builder = builder.file_mode(mode);
    }

    builder.build().context("build LinuxDevice")
}

fn stat_major_minor(path: &str) -> Result<(i64, i64)> {
    let meta = fs::metadata(path)
        .with_context(|| format!("stat CDI device node {path}"))?;
    let rdev = meta.rdev();
    // major/minor on Linux
    let major = ((rdev >> 8) & 0xfff) as i64 | (((rdev >> 32) & !0xfff) as i64);
    let minor = (rdev & 0xff) as i64 | (((rdev >> 12) & !0xff) as i64);
    Ok((major, minor))
}
