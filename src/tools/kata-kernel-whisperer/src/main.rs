// Copyright (c) Kata Containers Contributors
//
// SPDX-License-Identifier: Apache-2.0

mod args;
mod manifest;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Once,
};

use anyhow::{anyhow, bail, Context, Result};
use args::Args;
use clap::Parser;
use hypervisor::guest_cmdline::build_kernel_cmdline;
use kata_types::config::{
    CloudHypervisorConfig, DragonballConfig, FirecrackerConfig, QemuConfig, RemoteConfig,
    TomlConfig,
};
use manifest::RuntimeManifest;
use serde::Serialize;

/// The only hypervisor whose guest command line this tool can assemble.
const QEMU: &str = "qemu";

/// Schema version of the report printed by `--json`.
const REPORT_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
struct Report {
    version: u32,
    /// Architecture of this binary. Some guest parameters differ per
    /// architecture, so a report only describes the one it was produced on.
    architecture: String,
    runtime_classes: Vec<RuntimeClassOutput>,
    /// runtime-rs RuntimeClasses found but not described - currently those on a
    /// hypervisor other than QEMU - so a consumer can tell an entry that has gone
    /// missing from one this tool cannot speak for yet.
    skipped: Vec<SkippedRuntimeClass>,
}

#[derive(Debug, Serialize)]
struct RuntimeClassOutput {
    runtime_class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    shim: Option<String>,
    cmdline: String,
}

#[derive(Debug, Serialize)]
struct SkippedRuntimeClass {
    runtime_class: String,
    reason: String,
}

/// A RuntimeClass to describe, and where its configuration lives.
#[derive(Debug)]
struct Target {
    runtime_class: String,
    shim: Option<String>,
    config: PathBuf,
    /// Whether runtime-rs serves this RuntimeClass. `None` when nothing in the
    /// installation says either way, in which case an explicitly requested
    /// configuration is taken at face value.
    runtime_rs: Option<bool>,
    /// Hypervisor named by the manifest, which lets an unsupported RuntimeClass
    /// be skipped without reading its configuration at all.
    hypervisor: Option<String>,
}

enum Outcome {
    Described(RuntimeClassOutput),
    Skipped(SkippedRuntimeClass),
}

static REGISTER_PLUGINS: Once = Once::new();

/// Register every hypervisor configuration plugin, as the runtime does. Only
/// QEMU command lines can be assembled, but a configuration cannot even be
/// loaded without its plugin, and reporting an unsupported hypervisor is nicer
/// than failing to parse its configuration.
fn register_hypervisor_plugins() {
    REGISTER_PLUGINS.call_once(|| {
        QemuConfig::new().register();
        CloudHypervisorConfig::new().register();
        DragonballConfig::new().register();
        FirecrackerConfig::new().register();
        RemoteConfig::new().register();
    });
}

fn runtime_class_for_config(config_path: &Path) -> Result<String> {
    // Kata-deploy installs configs under runtimes/<shim>/ or
    // custom-runtimes/<handler>/. The RuntimeClass comes from that directory
    // name, not the configuration filename.
    let name = config_path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            anyhow!(
                "cannot derive runtime class from configuration path {}",
                config_path.display()
            )
        })?;

    if name.starts_with("kata-") {
        Ok(name.to_string())
    } else {
        Ok(format!("kata-{name}"))
    }
}

/// The shim a `configuration-<shim>.toml` file configures.
fn shim_for_config(config_path: &Path) -> Option<String> {
    config_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("configuration-"))
        .and_then(|name| name.strip_suffix(".toml"))
        .map(str::to_string)
}

/// Whether `shim` is served by runtime-rs, decided by where the installation
/// keeps its shipped configuration: kata-deploy files the Go runtime's shims
/// under share/defaults/kata-containers/ and runtime-rs' under the runtime-rs/
/// subdirectory of it. Asking the installation avoids keeping a copy of
/// kata-deploy's shim list here, which would silently rot as shims are added.
fn shim_is_runtime_rs(kata_root: &Path, shim: &str) -> Option<bool> {
    let defaults = kata_root.join("share/defaults/kata-containers");
    let file = format!("configuration-{shim}.toml");

    if defaults.join("runtime-rs").join(&file).exists() {
        Some(true)
    } else if defaults.join(&file).exists() {
        Some(false)
    } else {
        None
    }
}

fn targets_from_manifest(kata_root: &Path, manifest: &RuntimeManifest) -> Vec<Target> {
    manifest
        .runtimes
        .iter()
        .map(|entry| Target {
            runtime_class: entry.runtime_class.clone(),
            shim: Some(entry.shim.clone()),
            config: kata_root.join(&entry.config),
            runtime_rs: Some(entry.runtime_rs),
            hypervisor: Some(entry.hypervisor.clone()),
        })
        .collect()
}

/// Collect the `configuration-*.toml` files one level below `dir`, which is how
/// kata-deploy lays out both runtimes/ and custom-runtimes/.
fn configs_in_runtime_dir(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut configs = Vec::new();

    if !dir.is_dir() {
        return Ok(configs);
    }

    for entry in fs::read_dir(dir)
        .with_context(|| format!("failed to read runtime directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        for file in fs::read_dir(&path)
            .with_context(|| format!("failed to read configuration directory {}", path.display()))?
        {
            let file = file?;
            let file_path = file.path();
            let file_name = file_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if file_path.is_file()
                && file_name.starts_with("configuration-")
                && file_name.ends_with(".toml")
            {
                configs.push(file_path);
            }
        }
    }

    configs.sort();
    Ok(configs)
}

/// Discover RuntimeClasses by walking an installation that carries no manifest.
fn targets_from_layout(kata_root: &Path) -> Result<Vec<Target>> {
    let defaults = kata_root.join("share/defaults/kata-containers");
    let mut configs = configs_in_runtime_dir(&defaults.join("runtime-rs/runtimes"))?;
    configs.extend(configs_in_runtime_dir(&defaults.join("runtimes"))?);
    configs.extend(configs_in_runtime_dir(&defaults.join("custom-runtimes"))?);
    configs.sort();
    configs.dedup();

    if configs.is_empty() {
        bail!(
            "no runtime configuration files found under {}",
            defaults.display()
        );
    }

    configs
        .into_iter()
        .map(|config| {
            let shim = shim_for_config(&config);
            let runtime_rs = shim
                .as_deref()
                .and_then(|shim| shim_is_runtime_rs(kata_root, shim));
            Ok(Target {
                runtime_class: runtime_class_for_config(&config)?,
                shim,
                config,
                runtime_rs,
                hypervisor: None,
            })
        })
        .collect()
}

fn cmdline_for_target(target: &Target) -> Result<Outcome> {
    let skip = |reason: String| {
        Ok(Outcome::Skipped(SkippedRuntimeClass {
            runtime_class: target.runtime_class.clone(),
            reason,
        }))
    };

    let unsupported_hypervisor =
        |hypervisor: &str| format!("hypervisor {hypervisor} is not supported yet; only {QEMU} is");

    if let Some(hypervisor) = target.hypervisor.as_deref() {
        if hypervisor != QEMU {
            return skip(unsupported_hypervisor(hypervisor));
        }
    }

    register_hypervisor_plugins();

    // load_from_file deliberately includes config.d drop-ins and runs all
    // configuration adjustment hooks, matching runtime-rs.
    let (mut config, resolved_path) = TomlConfig::load_from_file(&target.config)
        .with_context(|| format!("failed to load config {}", target.config.display()))?;

    let hypervisor_name = config.runtime.hypervisor_name.clone();
    if hypervisor_name != QEMU {
        return skip(unsupported_hypervisor(&hypervisor_name));
    }

    // The runtime adds these after annotation processing. This tool does not
    // process annotations, but must retain the agent-derived parameters.
    config.add_agent_kernel_params();
    config
        .validate()
        .with_context(|| format!("failed to validate config {}", resolved_path.display()))?;

    let hypervisor_config = config.hypervisor.get(&hypervisor_name).ok_or_else(|| {
        anyhow!(
            "hypervisor {hypervisor_name} is not defined in {}",
            resolved_path.display()
        )
    })?;

    Ok(Outcome::Described(RuntimeClassOutput {
        runtime_class: target.runtime_class.clone(),
        shim: target.shim.clone(),
        cmdline: build_kernel_cmdline(&hypervisor_name, hypervisor_config)?,
    }))
}

fn collect_targets(args: &Args) -> Result<Vec<Target>> {
    let mut targets = Vec::new();

    if let Some(kata_root) = &args.kata_root {
        match RuntimeManifest::load(kata_root)? {
            Some(manifest) => targets.extend(targets_from_manifest(kata_root, &manifest)),
            None => {
                eprintln!(
                    "note: {} has no {}; falling back to discovery by directory layout. Render \
                     the tree with `kata-deploy render-configs` for an authoritative list.",
                    kata_root.display(),
                    manifest::MANIFEST_FILE_NAME
                );
                targets.extend(targets_from_layout(kata_root)?);
            }
        }
    }

    for config in &args.config {
        targets.push(Target {
            runtime_class: runtime_class_for_config(config)?,
            shim: shim_for_config(config),
            config: config.clone(),
            runtime_rs: None,
            hypervisor: None,
        });
    }

    if targets.is_empty() {
        bail!("provide --kata-root and/or one or more --config paths");
    }

    // This tool speaks for runtime-rs, so a RuntimeClass served by the Go runtime
    // is out of scope rather than pending support: leave it out of the report
    // entirely instead of listing it as skipped. The manifest still records which
    // classes those are, so nothing is lost.
    let (targets, go_runtime): (Vec<Target>, Vec<Target>) = targets
        .into_iter()
        .partition(|target| target.runtime_rs != Some(false));

    if !go_runtime.is_empty() {
        eprintln!(
            "note: not describing {} Go runtime RuntimeClass(es); this tool covers runtime-rs only.",
            go_runtime.len()
        );
    }

    let mut targets = targets;
    targets.sort_by(|a, b| a.runtime_class.cmp(&b.runtime_class));
    targets.dedup_by(|a, b| a.runtime_class == b.runtime_class && a.config == b.config);

    Ok(targets)
}

fn report(args: &Args) -> Result<Report> {
    let mut runtime_classes = Vec::new();
    let mut skipped = Vec::new();

    for target in collect_targets(args)? {
        match cmdline_for_target(&target)? {
            Outcome::Described(output) => runtime_classes.push(output),
            Outcome::Skipped(skip) => skipped.push(skip),
        }
    }

    Ok(Report {
        version: REPORT_VERSION,
        architecture: std::env::consts::ARCH.to_string(),
        runtime_classes,
        skipped,
    })
}

fn run(args: Args) -> Result<()> {
    let report = report(&args)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    for output in &report.runtime_classes {
        println!("{}: {}", output.runtime_class, output.cmdline);
    }

    for skip in &report.skipped {
        eprintln!("skipped {}: {}", skip.runtime_class, skip.reason);
    }

    Ok(())
}

fn main() -> Result<()> {
    run(Args::parse())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_runtime_config(runtime_dir: &Path, shim: &str, hypervisor: &str, artifact: &Path) {
        fs::create_dir_all(runtime_dir).unwrap();
        fs::write(
            runtime_dir.join(format!("configuration-{shim}.toml")),
            format!(
                r#"
[hypervisor.{hypervisor}]
path = "{artifact}"
kernel = "{artifact}"
image = "{artifact}"
rootfs_type = "ext4"
vm_rootfs_driver = "virtio-blk-pci"
kernel_params = "base=value"

[agent.kata]
enable_debug = true

[runtime]
hypervisor_name = "{hypervisor}"
agent_name = "kata"
"#,
                artifact = artifact.display(),
                hypervisor = hypervisor,
            ),
        )
        .unwrap();
    }

    #[test]
    fn config_drop_in_and_agent_params_are_applied() {
        let temp_dir = tempfile::tempdir().unwrap();
        let artifact = temp_dir.path().join("artifact");
        fs::write(&artifact, "").unwrap();

        let runtime_dir = temp_dir.path().join("qemu-runtime-rs");
        write_runtime_config(&runtime_dir, "qemu-runtime-rs", "qemu", &artifact);

        let drop_in_dir = runtime_dir.join("config.d");
        fs::create_dir(&drop_in_dir).unwrap();
        fs::write(
            drop_in_dir.join("10-kernel-params.toml"),
            r#"
[hypervisor.qemu]
kernel_params = "dropin=applied"
"#,
        )
        .unwrap();

        let target = Target {
            runtime_class: "kata-qemu-runtime-rs".to_string(),
            shim: Some("qemu-runtime-rs".to_string()),
            config: runtime_dir.join("configuration-qemu-runtime-rs.toml"),
            runtime_rs: Some(true),
            hypervisor: Some("qemu".to_string()),
        };

        let output = match cmdline_for_target(&target).unwrap() {
            Outcome::Described(output) => output,
            Outcome::Skipped(skip) => panic!("unexpectedly skipped: {}", skip.reason),
        };

        assert_eq!(output.runtime_class, "kata-qemu-runtime-rs");
        assert!(output.cmdline.contains("agent.log=debug"));
        assert!(output.cmdline.contains("dropin=applied"));
        assert!(!output.cmdline.contains("base=value"));
        assert!(output.cmdline.ends_with("console=hvc0"));
    }

    #[test]
    fn runtime_class_uses_parent_directory_name() {
        assert_eq!(
            runtime_class_for_config(Path::new(
                "/opt/kata/share/defaults/kata-containers/runtime-rs/runtimes/qemu-coco-dev-runtime-rs/configuration-qemu-coco-dev-runtime-rs.toml"
            ))
            .unwrap(),
            "kata-qemu-coco-dev-runtime-rs"
        );
        assert_eq!(
            runtime_class_for_config(Path::new(
                "/opt/kata/share/defaults/kata-containers/custom-runtimes/kata-qemu-runtime-rs-devkit/configuration-qemu-runtime-rs.toml"
            ))
            .unwrap(),
            "kata-qemu-runtime-rs-devkit"
        );
    }

    /// A rendered installation, with the debug variant of both a runtime-rs and
    /// a Go shim, as kata-deploy lays it out.
    fn write_installation(root: &Path, artifact: &Path) {
        let defaults = root.join("share/defaults/kata-containers");

        // Shipped configurations: their location is what says which runtime
        // serves a shim.
        write_runtime_config(
            &defaults.join("runtime-rs"),
            "qemu-runtime-rs",
            "qemu",
            artifact,
        );
        write_runtime_config(&defaults, "qemu", "qemu", artifact);

        write_runtime_config(
            &defaults.join("runtime-rs/runtimes/qemu-runtime-rs"),
            "qemu-runtime-rs",
            "qemu",
            artifact,
        );
        write_runtime_config(&defaults.join("runtimes/qemu"), "qemu", "qemu", artifact);
        write_runtime_config(
            &defaults.join("custom-runtimes/kata-qemu-runtime-rs-debug"),
            "qemu-runtime-rs",
            "qemu",
            artifact,
        );
        write_runtime_config(
            &defaults.join("custom-runtimes/kata-qemu-debug"),
            "qemu",
            "qemu",
            artifact,
        );
    }

    #[test]
    fn manifest_drives_discovery_and_leaves_out_the_go_runtime() {
        let temp_dir = tempfile::tempdir().unwrap();
        let artifact = temp_dir.path().join("artifact");
        fs::write(&artifact, "").unwrap();
        write_installation(temp_dir.path(), &artifact);

        fs::write(
            temp_dir
                .path()
                .join("share/defaults/kata-containers")
                .join(manifest::MANIFEST_FILE_NAME),
            r#"{
  "version": 1,
  "architecture": "x86_64",
  "install_dir": "/opt/kata",
  "runtimes": [
    {
      "runtime_class": "kata-qemu",
      "shim": "qemu",
      "hypervisor": "qemu",
      "runtime_rs": false,
      "config": "share/defaults/kata-containers/runtimes/qemu/configuration-qemu.toml"
    },
    {
      "runtime_class": "kata-qemu-runtime-rs",
      "shim": "qemu-runtime-rs",
      "hypervisor": "qemu",
      "runtime_rs": true,
      "config": "share/defaults/kata-containers/runtime-rs/runtimes/qemu-runtime-rs/configuration-qemu-runtime-rs.toml"
    },
    {
      "runtime_class": "kata-qemu-runtime-rs-debug",
      "shim": "qemu-runtime-rs",
      "hypervisor": "qemu",
      "runtime_rs": true,
      "config": "share/defaults/kata-containers/custom-runtimes/kata-qemu-runtime-rs-debug/configuration-qemu-runtime-rs.toml"
    }
  ]
}
"#,
        )
        .unwrap();

        let args = Args {
            config: Vec::new(),
            kata_root: Some(temp_dir.path().to_path_buf()),
            json: false,
        };
        let report = report(&args).unwrap();

        let described: Vec<_> = report
            .runtime_classes
            .iter()
            .map(|output| output.runtime_class.as_str())
            .collect();
        assert_eq!(
            described,
            vec!["kata-qemu-runtime-rs", "kata-qemu-runtime-rs-debug"]
        );

        // The Go runtime's class shares both a hypervisor and a "qemu" in its
        // name with the runtime-rs ones, so the danger is describing it with
        // runtime-rs' rules. It is out of scope, so it appears nowhere at all.
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn layout_discovery_identifies_the_go_runtime_without_a_manifest() {
        let temp_dir = tempfile::tempdir().unwrap();
        let artifact = temp_dir.path().join("artifact");
        fs::write(&artifact, "").unwrap();
        write_installation(temp_dir.path(), &artifact);

        let targets = targets_from_layout(temp_dir.path()).unwrap();
        let classified: Vec<_> = targets
            .iter()
            .map(|target| (target.runtime_class.as_str(), target.runtime_rs))
            .collect();

        assert!(classified.contains(&("kata-qemu-runtime-rs", Some(true))));
        assert!(classified.contains(&("kata-qemu-runtime-rs-debug", Some(true))));
        assert!(classified.contains(&("kata-qemu", Some(false))));
        assert!(classified.contains(&("kata-qemu-debug", Some(false))));
    }

    #[test]
    fn non_qemu_hypervisors_are_skipped_not_fatal() {
        let temp_dir = tempfile::tempdir().unwrap();
        let artifact = temp_dir.path().join("artifact");
        fs::write(&artifact, "").unwrap();

        let runtime_dir = temp_dir.path().join("dragonball");
        write_runtime_config(&runtime_dir, "dragonball", "dragonball", &artifact);

        // No manifest said which hypervisor this is, so it has to be recognized
        // from the configuration itself.
        let target = Target {
            runtime_class: "kata-dragonball".to_string(),
            shim: Some("dragonball".to_string()),
            config: runtime_dir.join("configuration-dragonball.toml"),
            runtime_rs: Some(true),
            hypervisor: None,
        };

        match cmdline_for_target(&target).unwrap() {
            Outcome::Skipped(skip) => assert!(skip.reason.contains("dragonball")),
            Outcome::Described(output) => panic!("unexpectedly described: {}", output.cmdline),
        }
    }
}
