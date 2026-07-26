// Copyright (c) 2026 Kata Contributors
//
// SPDX-License-Identifier: Apache-2.0

//! Reader for the RuntimeClass manifest `kata-deploy render-configs` writes
//! next to the configurations it installs.
//!
//! Without it, the RuntimeClasses present in an installation have to be inferred
//! by walking the tree and reading meaning into directory names - which cannot
//! distinguish, say, the Go runtime's `kata-qemu-debug` from runtime-rs'
//! `kata-qemu-runtime-rs-debug`, since both are just directories with "qemu" in
//! the name. kata-deploy knows the answer, so let it say so.
//!
//! These structs mirror the ones kata-deploy serializes in
//! `tools/packaging/kata-deploy/binary/src/artifacts/manifest.rs`; keep the two
//! in step and bump the version there on any incompatible change.

use std::{fs, path::Path};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Highest schema version this tool understands.
const SUPPORTED_VERSION: u32 = 1;

pub(crate) const MANIFEST_FILE_NAME: &str = "kata-deploy-runtimes.json";

#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeManifest {
    pub(crate) version: u32,
    pub(crate) runtimes: Vec<RuntimeEntry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeEntry {
    pub(crate) runtime_class: String,
    pub(crate) shim: String,
    pub(crate) hypervisor: String,
    pub(crate) runtime_rs: bool,
    /// Configuration file, relative to the installation root.
    pub(crate) config: String,
}

impl RuntimeManifest {
    /// Read the manifest of the installation rooted at `kata_root`, if it has
    /// one. `Ok(None)` means the installation predates the manifest, not that
    /// something went wrong.
    pub(crate) fn load(kata_root: &Path) -> Result<Option<Self>> {
        let path = kata_root
            .join("share/defaults/kata-containers")
            .join(MANIFEST_FILE_NAME);

        if !path.is_file() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read manifest {}", path.display()))?;
        let manifest: Self = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse manifest {}", path.display()))?;

        if manifest.version > SUPPORTED_VERSION {
            bail!(
                "manifest {} has version {}, which this build does not understand (supports up \
                 to {SUPPORTED_VERSION})",
                path.display(),
                manifest.version
            );
        }

        Ok(Some(manifest))
    }
}
