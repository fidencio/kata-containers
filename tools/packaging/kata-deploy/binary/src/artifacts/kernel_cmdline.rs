// Copyright (c) 2026 Kata Containers community
//
// SPDX-License-Identifier: Apache-2.0

//! Guest kernel command lines for the confidential RuntimeClasses this node
//! installs.
//!
//! What a RuntimeClass boots with is decided entirely by its configuration and
//! drop-ins, so it can be worked out at install time - but only by code that
//! knows how the runtime assembles the command line. `kata-kernel-whisperer`
//! knows; it links the very same kata-types and hypervisor crates the shim does.
//! So run it and publish what it says (see
//! `annotate_runtimeclasses_with_guest_kernel_cmdline`), rather than
//! reimplementing the assembly here and letting the two drift.
//!
//! Only the confidential classes are published, for the reason given at
//! `CONFIDENTIAL_SHIM_MARKERS`, which is also why nothing here happens at all on
//! an ordinary install.
//!
//! The tool stays inside the kata-deploy container: it is an install-time
//! detail, not something the node is made to carry. Two things follow from that.
//!
//! Configuration files record artifact paths as the *node* sees them
//! (`/opt/kata/bin/qemu-system-x86_64`), and loading a configuration
//! canonicalizes them - which fails in a container that has no `/opt/kata` of
//! its own. Symlinking the install directory to its `/host` view for the
//! duration of the call makes those paths resolve without copying anything onto
//! the node. Artifact paths do not appear on the guest command line, so reaching
//! them by a different route cannot change the answer.
//!
//! And the list of RuntimeClasses is passed in rather than discovered. Left to
//! infer it from the installed tree, the whisperer can only go by directory
//! names, which do not say whether a class is served by runtime-rs or by the Go
//! runtime - and describing a Go RuntimeClass with runtime-rs' assembly would
//! annotate it with a command line it never boots. kata-deploy knows, so it
//! writes the manifest out (container-side) and points the whisperer at it.

use crate::artifacts::install::{extract_component_tarball, ComponentTarball};
use crate::artifacts::manifest::RuntimeManifest;
use crate::config::Config;
use anyhow::{Context, Result};
use base64::Engine;
use flate2::{write::GzEncoder, Compression};
use log::{debug, info, warn};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Component tarball carrying the whisperer, extracted into the container.
const WHISPERER_COMPONENT: &str = "kata-kernel-whisperer";

/// Where the whisperer is unpacked *inside the container*. Alongside the
/// tarballs it came from, so nothing suggests it is part of the installation.
const WHISPERER_DIR: &str = "/opt/kata-artifacts/kernel-whisperer";

/// Path of the binary within the extracted component.
const WHISPERER_BIN: &str = "bin/kata-kernel-whisperer";

/// Name under `WHISPERER_DIR` where the manifest describing this node's
/// RuntimeClasses is handed over. Container-side, so the node is not made to
/// carry it either.
const MANIFEST_FILE: &str = "kata-deploy-runtimes.json";

/// Highest report schema this code knows how to read.
const SUPPORTED_REPORT_VERSION: u32 = 1;

/// Annotation carrying a RuntimeClass' guest kernel command line, gzip'd and
/// then base64-encoded.
///
/// Encoded rather than plain because the command line runs well past a
/// screenful once guest extensions (devkit, GPU, CoCo) combine, at which point
/// it is unreadable either way and being compact is worth more. Recover it with
/// `base64 -d | gunzip`.
///
/// Needs no architecture in the name: only confidential RuntimeClasses carry it
/// (see `CONFIDENTIAL_SHIM_MARKERS`) and each of those exists on exactly one
/// architecture, so no two nodes can disagree about the value.
const CMDLINE_ANNOTATION_KEY: &str = "katacontainers.io/guest-kernel-cmdline.gz.base64";

/// Shim name segments that mark a confidential shim.
///
/// Only confidential RuntimeClasses get their command line published, because
/// only there does anyone need it up front: the guest command line is covered by
/// the launch measurement, so a relying party has to know what to expect of it
/// before it will trust the guest. Elsewhere it is a detail of a VM the host
/// already controls, and publishing it cluster-wide would raise questions about
/// which node's answer is on show without answering anything.
const CONFIDENTIAL_SHIM_MARKERS: &[&str] = &["snp", "tdx", "cca"];

/// Whether `shim` runs guests under a TEE whose launch measurement covers the
/// guest kernel command line.
///
/// Matched on hyphen-separated segments of the shim name, so a shim added later
/// is recognized by the naming kata-deploy already uses
/// (`qemu-nvidia-gpu-snp-runtime-rs`) without needing a list of every
/// combination. Secure Execution is deliberately absent: its command line lives
/// inside the signed boot image rather than being passed to the guest, so there
/// is nothing here to describe.
fn is_confidential_shim(shim: &str) -> bool {
    shim.split('-')
        .any(|segment| CONFIDENTIAL_SHIM_MARKERS.contains(&segment))
}

/// RuntimeClasses in `manifest` whose guest command line is worth publishing.
///
/// Decided from the shim each class is based on rather than from its own name,
/// which is what carries the debug and devkit variants along: they are separate
/// RuntimeClasses serving the shim they were derived from, and a variant of a
/// confidential class is every bit as confidential.
fn confidential_runtime_classes(manifest: &RuntimeManifest) -> BTreeSet<&str> {
    manifest
        .runtimes
        .iter()
        .filter(|entry| is_confidential_shim(&entry.shim))
        .map(|entry| entry.runtime_class.as_str())
        .collect()
}

/// The whisperer's `--json` report. Mirrors the structs it serializes in
/// `src/tools/kata-kernel-whisperer/src/main.rs`.
#[derive(Debug, Deserialize)]
struct Report {
    version: u32,
    architecture: String,
    runtime_classes: Vec<ReportedRuntimeClass>,
    #[serde(default)]
    skipped: Vec<SkippedRuntimeClass>,
}

#[derive(Debug, Deserialize)]
struct ReportedRuntimeClass {
    runtime_class: String,
    cmdline: String,
}

#[derive(Debug, Deserialize)]
struct SkippedRuntimeClass {
    runtime_class: String,
    reason: String,
}

/// Work out the guest kernel command line of the confidential RuntimeClasses
/// installed here, encoded for publication and keyed by RuntimeClass name.
///
/// `Ok(None)` means the answer is unavailable rather than empty: an image built
/// without the whisperer component, or an installation it cannot describe. That
/// is not a reason to fail an install, so callers carry on without the
/// annotation.
fn guest_kernel_cmdlines(config: &Config) -> Result<Option<BTreeMap<String, String>>> {
    // Which RuntimeClasses exist, and which shim each is based on, is something
    // only kata-deploy knows; hand it over rather than have the whisperer infer
    // it from directory names, which cannot tell a Go-runtime class from a
    // runtime-rs one.
    let manifest = RuntimeManifest::from_config(config)?;
    let confidential = confidential_runtime_classes(&manifest);

    // The overwhelmingly common case: nothing confidential installed, so there
    // is nothing worth publishing and no reason to go looking.
    if confidential.is_empty() {
        debug!("No confidential RuntimeClasses installed; no guest kernel command line to publish");
        return Ok(None);
    }

    let Some(binary) = stage_whisperer()? else {
        return Ok(None);
    };

    let manifest_path = Path::new(WHISPERER_DIR).join(MANIFEST_FILE);
    manifest.write_to(&manifest_path)?;

    // Keep the symlink alive only while the whisperer runs.
    let Some(kata_root) = InstallDirLink::create(config)? else {
        return Ok(None);
    };

    let report = run_whisperer(&binary, kata_root.kata_root(), &manifest_path)?;
    drop(kata_root);

    if report.version > SUPPORTED_REPORT_VERSION {
        warn!(
            "kata-kernel-whisperer reported schema version {}, which this kata-deploy does not \
             understand (supports up to {SUPPORTED_REPORT_VERSION}); not annotating RuntimeClasses",
            report.version
        );
        return Ok(None);
    }

    for skipped in &report.skipped {
        debug!(
            "No guest kernel command line for RuntimeClass {}: {}",
            skipped.runtime_class, skipped.reason
        );
    }

    let mut by_runtime_class = BTreeMap::new();
    for class in &report.runtime_classes {
        if !confidential.contains(class.runtime_class.as_str()) {
            continue;
        }
        by_runtime_class.insert(class.runtime_class.clone(), encode_cmdline(&class.cmdline)?);
    }

    debug!(
        "Described {} confidential RuntimeClass(es) on {}",
        by_runtime_class.len(),
        report.architecture
    );

    Ok(Some(by_runtime_class))
}

/// Unpack the whisperer into the container, returning its path. `Ok(None)` when
/// the image was built without the component - a partial build, typically, which
/// should not stop an install.
fn stage_whisperer() -> Result<Option<PathBuf>> {
    let binary = Path::new(WHISPERER_DIR).join(WHISPERER_BIN);
    if binary.is_file() {
        return Ok(Some(binary));
    }

    match extract_component_tarball(WHISPERER_COMPONENT, WHISPERER_DIR)? {
        ComponentTarball::Extracted => {}
        ComponentTarball::Missing => {
            warn!(
                "This kata-deploy image was built without the '{WHISPERER_COMPONENT}' component; \
                 RuntimeClasses will not be annotated with their guest kernel command line"
            );
            return Ok(None);
        }
    }

    if !binary.is_file() {
        warn!(
            "Component '{WHISPERER_COMPONENT}' does not contain {WHISPERER_BIN}; RuntimeClasses \
             will not be annotated with their guest kernel command line"
        );
        return Ok(None);
    }

    Ok(Some(binary))
}

/// Makes the install directory reachable in the container under the path the
/// configurations were written for, by symlinking it to its `/host` view.
///
/// A link this creates is removed when the guard drops: it is scaffolding for
/// one call, and leaving it behind would make the container look as though it
/// had an installation of its own. A link that was already there is left alone.
struct InstallDirLink {
    /// Path to hand the whisperer as `--kata-root`.
    kata_root: PathBuf,
    remove_on_drop: Option<PathBuf>,
}

impl InstallDirLink {
    /// `Ok(None)` when the container already has something of its own at the
    /// install path, which is not ours to replace.
    fn create(config: &Config) -> Result<Option<Self>> {
        let node_path = PathBuf::from(&config.dest_dir);
        let host_view = PathBuf::from(&config.host_install_dir);

        // Nothing to arrange when the two are the same path, as they are when
        // kata-deploy runs directly on a host rather than in a container.
        if node_path == host_view {
            return Ok(Some(Self {
                kata_root: node_path,
                remove_on_drop: None,
            }));
        }

        match fs::symlink_metadata(&node_path) {
            Ok(metadata) => {
                if metadata.is_symlink() && fs::read_link(&node_path)? == host_view {
                    return Ok(Some(Self {
                        kata_root: node_path,
                        remove_on_drop: None,
                    }));
                }
                warn!(
                    "{} already exists inside the kata-deploy container, so it cannot be resolved \
                     to {} to read the installed configurations; RuntimeClasses will not be \
                     annotated with their guest kernel command line",
                    node_path.display(),
                    host_view.display()
                );
                Ok(None)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = node_path.parent() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!("Failed to create {} in the container", parent.display())
                    })?;
                }
                symlink(&host_view, &node_path).with_context(|| {
                    format!(
                        "Failed to link {} to {} in the container",
                        node_path.display(),
                        host_view.display()
                    )
                })?;
                debug!(
                    "Linked {} to {} for the duration of the guest kernel command line lookup",
                    node_path.display(),
                    host_view.display()
                );
                Ok(Some(Self {
                    kata_root: node_path.clone(),
                    remove_on_drop: Some(node_path),
                }))
            }
            Err(e) => Err(e).with_context(|| format!("Failed to inspect {}", node_path.display())),
        }
    }

    fn kata_root(&self) -> &Path {
        &self.kata_root
    }
}

impl Drop for InstallDirLink {
    fn drop(&mut self) {
        if let Some(path) = &self.remove_on_drop {
            if let Err(e) = fs::remove_file(path) {
                debug!("Failed to remove {}: {e}", path.display());
            }
        }
    }
}

fn run_whisperer(binary: &Path, kata_root: &Path, manifest: &Path) -> Result<Report> {
    let output = Command::new(binary)
        .arg("--kata-root")
        .arg(kata_root)
        .arg("--manifest")
        .arg(manifest)
        .arg("--json")
        .output()
        .with_context(|| format!("Failed to run {}", binary.display()))?;

    if !output.status.success() {
        anyhow::bail!(
            "{} failed: {}",
            binary.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("Failed to parse the report from {}", binary.display()))
}

/// gzip, then base64. Deterministic (flate2 writes no timestamp), so an
/// unchanged command line encodes to an unchanged annotation and repeat installs
/// have nothing to patch.
fn encode_cmdline(cmdline: &str) -> Result<String> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(cmdline.as_bytes())
        .context("Failed to compress a guest kernel command line")?;
    let compressed = encoder
        .finish()
        .context("Failed to compress a guest kernel command line")?;

    Ok(base64::engine::general_purpose::STANDARD.encode(compressed))
}

/// Publish each RuntimeClass' guest kernel command line as an annotation on it.
pub(crate) async fn annotate_runtimeclasses_with_guest_kernel_cmdline(
    config: &Config,
) -> Result<()> {
    let Some(cmdlines) = guest_kernel_cmdlines(config)? else {
        return Ok(());
    };

    if cmdlines.is_empty() {
        debug!("No guest kernel command lines to publish");
        return Ok(());
    }

    info!(
        "Publishing the guest kernel command line of {} confidential RuntimeClass(es) as {}",
        cmdlines.len(),
        CMDLINE_ANNOTATION_KEY
    );

    crate::k8s::runtimeclasses::annotate_guest_kernel_cmdlines(
        config,
        CMDLINE_ANNOTATION_KEY,
        &cmdlines,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CustomRuntime;
    use flate2::read::GzDecoder;
    use std::io::Read;
    use tempfile::TempDir;

    /// A config whose install directory and `/host` view are both inside `dir`,
    /// so the linking can be exercised without writing to /opt.
    fn config_installing_into(dir: &TempDir) -> Config {
        let mut config = Config::for_tests(&["qemu-runtime-rs"]);
        config.dest_dir = dir.path().join("node/opt/kata").display().to_string();
        config.host_install_dir = dir.path().join("host/opt/kata").display().to_string();
        fs::create_dir_all(&config.host_install_dir).unwrap();
        fs::create_dir_all(dir.path().join("node/opt")).unwrap();
        config
    }

    fn decode(encoded: &str) -> String {
        let compressed = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let mut decoder = GzDecoder::new(&compressed[..]);
        let mut cmdline = String::new();
        decoder.read_to_string(&mut cmdline).unwrap();
        cmdline
    }

    /// The whole point of the annotation is that a consumer can get the command
    /// line back out with nothing but gzip and base64.
    #[test]
    fn an_encoded_cmdline_decodes_to_itself() {
        let cmdline = "reboot=k panic=1 systemd.unit=kata-containers.target root=/dev/vda1 \
                       rootfstype=ext4 agent.log=debug initcall_debug selinux=0 console=hvc0";

        assert_eq!(decode(&encode_cmdline(cmdline).unwrap()), cmdline);
    }

    /// Repeat installs must not churn the annotation, so the same command line
    /// has to encode byte-identically every time.
    #[test]
    fn encoding_is_deterministic() {
        let cmdline = "reboot=k panic=1 console=hvc0";

        assert_eq!(
            encode_cmdline(cmdline).unwrap(),
            encode_cmdline(cmdline).unwrap()
        );
    }

    /// Compression has to earn its keep on the sizes actually seen once guest
    /// extensions combine; below that it is noise either way.
    #[test]
    fn encoding_a_realistic_cmdline_is_smaller_than_the_cmdline() {
        let cmdline = "reboot=k panic=1 systemd.unit=kata-containers.target \
                       systemd.mask=systemd-networkd.service systemd.mask=systemd-networkd.socket \
                       systemd.mask=systemd-journald.service systemd.mask=systemd-journald.socket \
                       cgroup_no_v1=all pci=realloc pci=nocrs pci=assign-busses nvrc.smi.srs=1 \
                       root=/dev/vda1 rootflags=data=ordered,errors=remount-ro ro rootfstype=ext4 \
                       agent.cdh_api_timeout=50 agent.enable_signature_verification=false \
                       agent.debug_console agent.debug_console_vport=1026 agent.log=debug \
                       agent.debug_console_shell=/run/kata-extensions/kata-devkit/usr/bin/bash \
                       initcall_debug selinux=0 console=hvc0";

        assert!(encode_cmdline(cmdline).unwrap().len() < cmdline.len());
    }

    /// Kubernetes rejects annotation names longer than 63 characters, and an
    /// install that gets that far only to be refused by the apiserver would be a
    /// poor way to find out.
    #[test]
    fn the_annotation_key_is_a_valid_kubernetes_name() {
        let (prefix, name) = CMDLINE_ANNOTATION_KEY.split_once('/').unwrap();

        assert_eq!(prefix, "katacontainers.io");
        assert!(name.len() <= 63, "{name} is {} characters", name.len());
        assert!(name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')));
    }

    /// The command line is published where a launch measurement covers it, and
    /// the shims that qualify are recognized from their names alone.
    #[test]
    fn confidential_shims_are_recognized_by_name() {
        for shim in [
            "qemu-snp-runtime-rs",
            "qemu-tdx-runtime-rs",
            "qemu-nvidia-gpu-snp-runtime-rs",
            "qemu-nvidia-gpu-tdx-runtime-rs",
        ] {
            assert!(is_confidential_shim(shim), "{shim} should qualify");
        }
    }

    /// Not everything confidential-adjacent qualifies. coco-dev has no TEE
    /// behind it, and Secure Execution carries its command line inside the
    /// signed boot image rather than passing it to the guest.
    #[test]
    fn shims_without_a_measured_cmdline_are_left_out() {
        for shim in [
            "qemu",
            "qemu-runtime-rs",
            "qemu-coco-dev-runtime-rs",
            "qemu-se-runtime-rs",
            "clh-runtime-rs",
            "dragonball",
        ] {
            assert!(!is_confidential_shim(shim), "{shim} should not qualify");
        }
    }

    /// Matching whole segments rather than substrings, so that a shim whose name
    /// merely contains the letters does not get swept in.
    #[test]
    fn a_marker_has_to_be_a_whole_name_segment() {
        assert!(!is_confidential_shim("qemu-snpfoo-runtime-rs"));
        assert!(!is_confidential_shim("qemu-not-tdxish"));
    }

    /// A debug or devkit variant of a confidential class is a RuntimeClass of
    /// its own, with its own command line, and just as measured as the class it
    /// came from. Selecting on the shim rather than the class name is what keeps
    /// them in.
    #[test]
    fn variants_of_a_confidential_class_are_published_too() {
        let mut config = Config::for_tests(&["qemu-snp-runtime-rs", "qemu-runtime-rs"]);
        config.custom_runtimes = ["qemu-snp-runtime-rs", "qemu-runtime-rs"]
            .iter()
            .flat_map(|shim| {
                ["debug", "devkit"]
                    .iter()
                    .map(move |variant| CustomRuntime {
                        handler: format!("kata-{shim}-{variant}"),
                        base_config: shim.to_string(),
                        drop_in_file: None,
                        containerd_snapshotter: None,
                        crio_pull_type: None,
                        debug_variant: true,
                        devkit: *variant == "devkit",
                    })
            })
            .collect();

        let manifest = RuntimeManifest::from_config(&config).unwrap();
        let published = confidential_runtime_classes(&manifest);

        assert_eq!(
            published.iter().copied().collect::<Vec<_>>(),
            vec![
                "kata-qemu-snp-runtime-rs",
                "kata-qemu-snp-runtime-rs-debug",
                "kata-qemu-snp-runtime-rs-devkit",
            ]
        );
    }

    /// An ordinary install does no work at all: nothing to publish, so the
    /// whisperer is never even unpacked.
    #[test]
    fn an_install_without_a_tee_publishes_nothing() {
        let config = Config::for_tests(&["qemu-runtime-rs", "qemu", "clh-runtime-rs"]);

        let manifest = RuntimeManifest::from_config(&config).unwrap();

        assert!(confidential_runtime_classes(&manifest).is_empty());
        assert!(guest_kernel_cmdlines(&config).unwrap().is_none());
    }

    #[test]
    fn a_report_from_a_newer_whisperer_is_read_as_far_as_it_goes() {
        let report: Report = serde_json::from_str(
            r#"{
                "version": 1,
                "architecture": "x86_64",
                "runtime_classes": [
                    {
                        "runtime_class": "kata-qemu-runtime-rs",
                        "shim": "qemu-runtime-rs",
                        "cmdline": "reboot=k panic=1",
                        "something_added_later": true
                    }
                ],
                "skipped": [
                    {"runtime_class": "kata-clh-runtime-rs", "reason": "hypervisor is not qemu"}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(report.architecture, "x86_64");
        assert_eq!(report.runtime_classes.len(), 1);
        assert_eq!(report.runtime_classes[0].cmdline, "reboot=k panic=1");
        assert_eq!(report.skipped[0].runtime_class, "kata-clh-runtime-rs");
    }

    /// A report without the `skipped` list at all still has to load.
    #[test]
    fn skipped_is_optional() {
        let report: Report = serde_json::from_str(
            r#"{"version": 1, "architecture": "aarch64", "runtime_classes": []}"#,
        )
        .unwrap();

        assert!(report.skipped.is_empty());
    }

    /// The configurations record node paths, so the whisperer has to be pointed
    /// at the node path - reached, inside the container, through the link.
    #[test]
    fn the_install_dir_is_reachable_under_its_node_path() {
        let dir = TempDir::new().unwrap();
        let config = config_installing_into(&dir);

        let link = InstallDirLink::create(&config).unwrap().unwrap();

        assert_eq!(link.kata_root(), Path::new(&config.dest_dir));
        assert_eq!(
            fs::read_link(&config.dest_dir).unwrap(),
            Path::new(&config.host_install_dir)
        );
    }

    /// Scaffolding for one call: the container must not be left looking as
    /// though it had an installation of its own.
    #[test]
    fn the_link_is_removed_afterwards() {
        let dir = TempDir::new().unwrap();
        let config = config_installing_into(&dir);

        drop(InstallDirLink::create(&config).unwrap().unwrap());

        assert!(fs::symlink_metadata(&config.dest_dir).is_err());
    }

    /// A link left behind by an earlier attempt is reusable, and is not ours to
    /// clean up.
    #[test]
    fn an_existing_link_is_reused_and_left_in_place() {
        let dir = TempDir::new().unwrap();
        let config = config_installing_into(&dir);
        symlink(&config.host_install_dir, &config.dest_dir).unwrap();

        drop(InstallDirLink::create(&config).unwrap().unwrap());

        assert_eq!(
            fs::read_link(&config.dest_dir).unwrap(),
            Path::new(&config.host_install_dir)
        );
    }

    /// Anything else already at the install path belongs to whoever put it
    /// there. Give up on the annotation rather than replace it.
    #[test]
    fn a_real_install_dir_in_the_container_is_not_replaced() {
        let dir = TempDir::new().unwrap();
        let config = config_installing_into(&dir);
        fs::create_dir_all(&config.dest_dir).unwrap();

        assert!(InstallDirLink::create(&config).unwrap().is_none());
        assert!(fs::metadata(&config.dest_dir).unwrap().is_dir());
    }

    /// Running on a host rather than in a container: the install directory is
    /// already at its own path and there is nothing to link.
    #[test]
    fn no_link_is_made_when_there_is_no_container_to_bridge() {
        let dir = TempDir::new().unwrap();
        let mut config = config_installing_into(&dir);
        config.host_install_dir = config.dest_dir.clone();
        fs::create_dir_all(&config.dest_dir).unwrap();

        let link = InstallDirLink::create(&config).unwrap().unwrap();

        assert_eq!(link.kata_root(), Path::new(&config.dest_dir));
        assert!(fs::metadata(&config.dest_dir).unwrap().is_dir());
    }
}
