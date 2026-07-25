// Copyright (c) Kata Containers Contributors
//
// SPDX-License-Identifier: Apache-2.0

mod args;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Once,
};

use anyhow::{anyhow, bail, Context, Result};
use args::Args;
use clap::Parser;
use hypervisor::guest_cmdline::build_kernel_cmdline;
use kata_types::config::{QemuConfig, TomlConfig};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct RuntimeClassOutput {
    runtime_class: String,
    architecture: String,
    config: PathBuf,
    cmdline: String,
}

static REGISTER_QEMU: Once = Once::new();

fn register_qemu_config() {
    REGISTER_QEMU.call_once(|| QemuConfig::new().register());
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

fn is_likely_qemu_runtime(config_path: &Path) -> bool {
    config_path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains("qemu"))
}

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

        for file in fs::read_dir(&path).with_context(|| {
            format!("failed to read configuration directory {}", path.display())
        })? {
            let file = file?;
            let file_path = file.path();
            let file_name = file_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if file_path.is_file()
                && file_name.starts_with("configuration-")
                && file_name.ends_with(".toml")
                && is_likely_qemu_runtime(&file_path)
            {
                configs.push(file_path);
            }
        }
    }

    configs.sort();
    Ok(configs)
}

fn discover_configs(kata_root: &Path) -> Result<Vec<PathBuf>> {
    let defaults = kata_root.join("share/defaults/kata-containers");
    let mut configs = configs_in_runtime_dir(&defaults.join("runtime-rs/runtimes"))?;
    configs.extend(configs_in_runtime_dir(&defaults.join("custom-runtimes"))?);
    configs.sort();
    configs.dedup();

    if configs.is_empty() {
        bail!(
            "no runtime configuration files found under {}",
            defaults.display()
        );
    }

    Ok(configs)
}

fn command_line_for_config(config_path: &Path) -> Result<RuntimeClassOutput> {
    register_qemu_config();

    // load_from_file deliberately includes config.d drop-ins and runs all
    // configuration adjustment hooks, matching runtime-rs.
    let (mut config, resolved_path) = TomlConfig::load_from_file(config_path)
        .with_context(|| format!("failed to load config {}", config_path.display()))?;

    // The runtime adds these after annotation processing. This tool does not
    // process annotations, but must retain the agent-derived parameters.
    config.add_agent_kernel_params();
    config
        .validate()
        .with_context(|| format!("failed to validate config {}", resolved_path.display()))?;

    let hypervisor_name = config.runtime.hypervisor_name.clone();
    let hypervisor_config = config.hypervisor.get(&hypervisor_name).ok_or_else(|| {
        anyhow!(
            "hypervisor {hypervisor_name} is not defined in {}",
            resolved_path.display()
        )
    })?;
    let cmdline = build_kernel_cmdline(&hypervisor_name, hypervisor_config)?;

    Ok(RuntimeClassOutput {
        runtime_class: runtime_class_for_config(&resolved_path)?,
        architecture: std::env::consts::ARCH.to_string(),
        config: resolved_path,
        cmdline,
    })
}

fn collect_config_paths(args: &Args) -> Result<Vec<PathBuf>> {
    let mut configs = args.config.clone();

    if let Some(kata_root) = &args.kata_root {
        configs.extend(discover_configs(kata_root)?);
    }

    configs.sort();
    configs.dedup();

    if configs.is_empty() {
        bail!("provide --kata-root and/or one or more --config paths");
    }

    Ok(configs)
}

fn run(args: Args) -> Result<()> {
    let mut outputs = Vec::new();

    for path in collect_config_paths(&args)? {
        match command_line_for_config(&path) {
            Ok(output) => outputs.push(output),
            Err(err) => {
                // Install roots include non-QEMU runtimes (dragonball, clh,
                // …). Skip those until their builders exist.
                let message = format!("{err:#}");
                if message.contains("only qemu is supported") {
                    eprintln!(
                        "skipping {}: {}",
                        path.display(),
                        message.lines().next().unwrap_or(&message)
                    );
                    continue;
                }
                return Err(err);
            }
        }
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&outputs)?);
    } else {
        for output in outputs {
            println!("{}: {}", output.runtime_class, output.cmdline);
        }
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

        let output = command_line_for_config(&runtime_dir.join("configuration-qemu-runtime-rs.toml"))
            .unwrap();

        assert_eq!(output.runtime_class, "kata-qemu-runtime-rs");
        assert_eq!(output.architecture, std::env::consts::ARCH);
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

    #[test]
    fn discovers_configs_under_kata_root() {
        let temp_dir = tempfile::tempdir().unwrap();
        let artifact = temp_dir.path().join("artifact");
        fs::write(&artifact, "").unwrap();

        let defaults = temp_dir
            .path()
            .join("share/defaults/kata-containers");
        write_runtime_config(
            &defaults.join("runtime-rs/runtimes/qemu-runtime-rs"),
            "qemu-runtime-rs",
            "qemu",
            &artifact,
        );
        write_runtime_config(
            &defaults.join("runtime-rs/runtimes/dragonball"),
            "dragonball",
            "dragonball",
            &artifact,
        );
        write_runtime_config(
            &defaults.join("custom-runtimes/kata-qemu-runtime-rs-devkit"),
            "qemu-runtime-rs",
            "qemu",
            &artifact,
        );

        let configs = discover_configs(temp_dir.path()).unwrap();
        assert_eq!(configs.len(), 2);
        assert!(configs.iter().any(|path| {
            path.ends_with("runtime-rs/runtimes/qemu-runtime-rs/configuration-qemu-runtime-rs.toml")
        }));
        assert!(configs.iter().any(|path| {
            path.ends_with(
                "custom-runtimes/kata-qemu-runtime-rs-devkit/configuration-qemu-runtime-rs.toml",
            )
        }));
        assert!(!configs.iter().any(|path| path.to_string_lossy().contains("dragonball")));

        let args = Args {
            config: Vec::new(),
            kata_root: Some(temp_dir.path().to_path_buf()),
            json: false,
        };
        let outputs = collect_config_paths(&args)
            .unwrap()
            .iter()
            .map(|path| command_line_for_config(path))
            .collect::<Result<Vec<_>>>()
            .unwrap();

        let classes: Vec<_> = outputs
            .iter()
            .map(|output| output.runtime_class.as_str())
            .collect();
        assert_eq!(
            classes,
            vec!["kata-qemu-runtime-rs-devkit", "kata-qemu-runtime-rs"]
        );
    }
}
