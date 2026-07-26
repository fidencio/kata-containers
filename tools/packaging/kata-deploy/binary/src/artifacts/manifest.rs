// Copyright (c) 2026 Kata Containers community
//
// SPDX-License-Identifier: Apache-2.0

//! Description of the RuntimeClasses a kata-deploy configuration installs.
//!
//! kata-deploy is the only component that knows which RuntimeClasses exist on a
//! node, which shim configuration each of them is based on, and where that
//! configuration ends up on disk. Tools that need to reason about the installed
//! configurations - `kata-kernel-whisperer`, which prints the guest kernel
//! command line per RuntimeClass - would otherwise have to rediscover all of
//! that by walking the installation tree and guessing from directory names.
//!
//! So write it down instead. The manifest is emitted by the `render-configs`
//! action next to the installed configurations.
//!
//! The schema is a file format shared with `kata-kernel-whisperer`, which
//! carries a matching set of `Deserialize` structs. Bump `MANIFEST_VERSION` on
//! any incompatible change so consumers can reject what they cannot read.

use crate::artifacts::install::{get_hypervisor_name, node_custom_runtimes_dir};
use crate::config::Config;
use crate::utils;
use anyhow::{Context, Result};
use log::info;
use serde::Serialize;
use std::fs;
use std::path::Path;

/// Incompatible-change counter for the manifest schema.
pub const MANIFEST_VERSION: u32 = 1;

/// Manifest file name, written under share/defaults/kata-containers/.
pub const MANIFEST_FILE_NAME: &str = "kata-deploy-runtimes.json";

#[derive(Debug, Serialize)]
pub struct RuntimeManifest {
    pub version: u32,
    pub architecture: String,
    /// Installation directory the `config` paths are relative to, as seen from
    /// the node (not from whatever prefix rendered the tree).
    pub install_dir: String,
    pub runtimes: Vec<RuntimeEntry>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeEntry {
    /// RuntimeClass name, which is also the CRI runtime handler.
    pub runtime_class: String,
    /// Shim whose configuration this RuntimeClass is based on. Variant classes
    /// share a shim with the RuntimeClass they were derived from.
    pub shim: String,
    pub hypervisor: String,
    /// Whether the shim is served by runtime-rs rather than the Go runtime. The
    /// two assemble the guest kernel command line differently, and a handler
    /// name alone does not say which one is in play.
    pub runtime_rs: bool,
    /// Configuration file, relative to `install_dir`.
    pub config: String,
}

impl RuntimeManifest {
    pub fn from_config(config: &Config) -> Result<Self> {
        let mut runtimes = Vec::new();

        for shim in &config.shims_for_arch {
            runtimes.push(RuntimeEntry {
                runtime_class: config.runtime_handler_for_shim(shim),
                shim: shim.clone(),
                hypervisor: get_hypervisor_name(shim)?.to_string(),
                runtime_rs: utils::is_rust_shim(shim),
                config: shim_config_path(shim),
            });
        }

        for runtime in &config.custom_runtimes {
            runtimes.push(RuntimeEntry {
                runtime_class: runtime.handler.clone(),
                shim: runtime.base_config.clone(),
                hypervisor: get_hypervisor_name(&runtime.base_config)?.to_string(),
                runtime_rs: utils::is_rust_shim(&runtime.base_config),
                config: custom_runtime_config_path(&runtime.handler, &runtime.base_config),
            });
        }

        runtimes.sort_by(|a, b| a.runtime_class.cmp(&b.runtime_class));

        Ok(Self {
            version: MANIFEST_VERSION,
            architecture: std::env::consts::ARCH.to_string(),
            install_dir: config.dest_dir.clone(),
            runtimes,
        })
    }

    /// Serialized form, as consumers read it.
    pub fn to_json(&self) -> Result<String> {
        let mut content = serde_json::to_string_pretty(self)?;
        content.push('\n');
        Ok(content)
    }

    /// Write the manifest next to the installed configurations.
    pub fn write(&self, config: &Config) -> Result<()> {
        let dir = config.host_path(&format!(
            "{}/share/defaults/kata-containers",
            config.dest_dir
        ));
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create manifest directory: {dir}"))?;

        let path = Path::new(&dir).join(MANIFEST_FILE_NAME);
        fs::write(&path, self.to_json()?)
            .with_context(|| format!("Failed to write manifest: {}", path.display()))?;

        info!(
            "Wrote runtime manifest describing {} RuntimeClass(es): {}",
            self.runtimes.len(),
            path.display()
        );
        Ok(())
    }
}

/// Configuration file of a standard shim, relative to the installation
/// directory. Derived from the same layout helper the installer uses, so the
/// manifest cannot drift from where the files are actually written.
fn shim_config_path(shim: &str) -> String {
    let dir = utils::get_kata_containers_config_path(shim, "");
    format!(
        "{}/configuration-{shim}.toml",
        dir.trim_start_matches('/').trim_end_matches('/')
    )
}

/// Configuration file of a custom runtime handler, relative to the installation
/// directory.
fn custom_runtime_config_path(handler: &str, base_config: &str) -> String {
    let dir = node_custom_runtimes_dir("");
    format!(
        "{}/{handler}/configuration-{base_config}.toml",
        dir.trim_start_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CustomRuntime;

    #[test]
    fn debug_variants_are_described_alongside_the_classes_they_derive_from() {
        let mut config = Config::for_tests(&["qemu-runtime-rs", "qemu"]);
        // What DEBUG=true synthesizes: one variant handler per enabled shim.
        config.custom_runtimes = ["qemu-runtime-rs", "qemu"]
            .iter()
            .map(|shim| CustomRuntime {
                handler: format!("kata-{shim}-debug"),
                base_config: shim.to_string(),
                drop_in_file: None,
                containerd_snapshotter: None,
                crio_pull_type: None,
                debug_variant: true,
                devkit: false,
            })
            .collect();

        let manifest = RuntimeManifest::from_config(&config).unwrap();

        assert_eq!(manifest.version, MANIFEST_VERSION);
        assert_eq!(manifest.install_dir, "/opt/kata");

        let described: Vec<_> = manifest
            .runtimes
            .iter()
            .map(|entry| (entry.runtime_class.as_str(), entry.runtime_rs))
            .collect();
        assert_eq!(
            described,
            vec![
                ("kata-qemu", false),
                ("kata-qemu-debug", false),
                ("kata-qemu-runtime-rs", true),
                ("kata-qemu-runtime-rs-debug", true),
            ]
        );

        // A variant class serves the same shim, from its own configuration copy.
        let variant = manifest
            .runtimes
            .iter()
            .find(|entry| entry.runtime_class == "kata-qemu-runtime-rs-debug")
            .unwrap();
        assert_eq!(variant.shim, "qemu-runtime-rs");
        assert_eq!(variant.hypervisor, "qemu");
        assert_eq!(
            variant.config,
            "share/defaults/kata-containers/custom-runtimes/kata-qemu-runtime-rs-debug/\
             configuration-qemu-runtime-rs.toml"
        );
    }

    #[test]
    fn rust_and_golang_shims_get_their_own_config_layout() {
        assert_eq!(
            shim_config_path("qemu-runtime-rs"),
            "share/defaults/kata-containers/runtime-rs/runtimes/qemu-runtime-rs/\
             configuration-qemu-runtime-rs.toml"
        );
        assert_eq!(
            shim_config_path("qemu"),
            "share/defaults/kata-containers/runtimes/qemu/configuration-qemu.toml"
        );
    }

    #[test]
    fn custom_runtimes_keep_the_base_configuration_filename() {
        // The handler names the directory; the file inside it keeps the name of
        // the shim configuration it was copied from.
        assert_eq!(
            custom_runtime_config_path("kata-qemu-runtime-rs-debug", "qemu-runtime-rs"),
            "share/defaults/kata-containers/custom-runtimes/kata-qemu-runtime-rs-debug/\
             configuration-qemu-runtime-rs.toml"
        );
    }
}
