// Copyright (c) Kata Containers Contributors
//
// SPDX-License-Identifier: Apache-2.0

use anyhow::{bail, Context, Result};
use kata_types::{
    config::hypervisor::Hypervisor as HypervisorConfig, machine_type::MACHINE_TYPE_S390X_TYPE,
};

use crate::{kernel_param::KernelParams, HYPERVISOR_QEMU};

/// Build the QEMU guest kernel command line represented by a runtime
/// configuration.
///
/// This includes all configuration-derived parameters added by the QEMU start
/// path. Sandbox-specific annotation and device overrides are intentionally not
/// considered.
pub fn build_kernel_cmdline(hypervisor_name: &str, config: &HypervisorConfig) -> Result<String> {
    if hypervisor_name != HYPERVISOR_QEMU {
        bail!("unsupported hypervisor {hypervisor_name}; only qemu is supported");
    }

    let mut params = build_qemu_kernel_params(config)?;

    if config.device_info.enable_iommu {
        add_qemu_iommu_kernel_params(&mut params, &config.machine_info.machine_type);
    }

    if config.security_info.confidential_guest
        && config.machine_info.machine_type == MACHINE_TYPE_S390X_TYPE
    {
        remove_qemu_secure_execution_kernel_params(&mut params);
    }

    // QEMU always adds its virtio console immediately before spawning the VM.
    add_qemu_console_kernel_params(&mut params);

    params.to_string()
}

pub(crate) fn add_qemu_iommu_kernel_params(params: &mut KernelParams, machine_type: &str) {
    let iommu_params = if machine_type == "virt" {
        "iommu.passthrough=0"
    } else {
        "intel_iommu=on iommu=pt"
    };
    params.append(&mut KernelParams::from_string(iommu_params));
}

pub(crate) fn add_qemu_console_kernel_params(params: &mut KernelParams) {
    params.append(&mut KernelParams::from_string("console=hvc0"));
}

pub(crate) fn remove_qemu_secure_execution_kernel_params(params: &mut KernelParams) {
    for key in [
        "reboot",
        "systemd.unit",
        "systemd.mask",
        "root",
        "rootflags",
        "rootfstype",
    ] {
        params.remove_all_by_key(key.to_string());
    }
}

pub(crate) fn build_qemu_kernel_params(config: &HypervisorConfig) -> Result<KernelParams> {
    let mut params = KernelParams::new(config.debug_info.enable_debug);

    if config.boot_info.initrd.is_empty() {
        // DAX is disabled on ARM due to a kernel panic in
        // caches_clean_inval_pou.
        #[cfg(target_arch = "aarch64")]
        let use_dax = false;
        #[cfg(not(target_arch = "aarch64"))]
        let use_dax = true;

        let mut rootfs_params = KernelParams::new_rootfs_kernel_params(
            &config.boot_info.kernel_verity_params,
            &config.boot_info.vm_rootfs_driver,
            &config.boot_info.rootfs_type,
            use_dax,
        )
        .context("adding rootfs/verity params failed")?;
        params.append(&mut rootfs_params);
    }

    params.append(&mut KernelParams::from_string(
        &config.boot_info.kernel_params,
    ));
    params.append(&mut KernelParams::from_string(&format!(
        "selinux={}",
        if config.disable_guest_selinux { 0 } else { 1 }
    )));

    for extension in &config.guest_extension_images {
        params.append(&mut KernelParams::from_string(&format!(
            "kata.extension.{}.verity_params={}",
            extension.name, extension.verity_params
        )));
    }

    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kata_types::config::hypervisor::{GuestExtensionImage, VIRTIO_BLK_PCI};

    fn qemu_config() -> HypervisorConfig {
        let mut config = HypervisorConfig::default();
        config.boot_info.vm_rootfs_driver = VIRTIO_BLK_PCI.to_string();
        config.boot_info.rootfs_type = "ext4".to_string();
        config.boot_info.kernel_params = "foo=bar".to_string();
        config
    }

    #[test]
    fn qemu_image_cmdline() {
        let cmdline = build_kernel_cmdline(HYPERVISOR_QEMU, &qemu_config()).unwrap();

        assert_eq!(
            cmdline,
            "reboot=k panic=1 systemd.unit=kata-containers.target \
             systemd.mask=systemd-networkd.service \
             systemd.mask=systemd-networkd.socket root=/dev/vda1 \
             rootflags=data=ordered,errors=remount-ro ro rootfstype=ext4 \
             foo=bar selinux=1 console=hvc0"
        );
    }

    #[test]
    fn qemu_config_derived_extras() {
        let mut config = qemu_config();
        config.device_info.enable_iommu = true;
        config.disable_guest_selinux = true;
        config.guest_extension_images.push(GuestExtensionImage {
            name: "gpu".to_string(),
            path: String::new(),
            verity_params: "root_hash=abc".to_string(),
        });

        let cmdline = build_kernel_cmdline(HYPERVISOR_QEMU, &config).unwrap();

        assert!(cmdline.contains("foo=bar selinux=0"));
        assert!(cmdline.contains("kata.extension.gpu.verity_params=root_hash=abc"));
        assert!(cmdline.ends_with("intel_iommu=on iommu=pt console=hvc0"));
    }

    #[test]
    fn qemu_secure_execution_strips_boot_parameters() {
        let mut config = qemu_config();
        config.security_info.confidential_guest = true;
        config.machine_info.machine_type = MACHINE_TYPE_S390X_TYPE.to_string();

        let cmdline = build_kernel_cmdline(HYPERVISOR_QEMU, &config).unwrap();

        assert!(!cmdline.contains("reboot="));
        assert!(!cmdline.contains("systemd.unit="));
        assert!(!cmdline.contains("systemd.mask="));
        assert!(!cmdline.contains("root="));
        assert!(cmdline.contains("panic=1"));
        assert!(cmdline.ends_with("foo=bar selinux=1 console=hvc0"));
    }

    #[test]
    fn rejects_non_qemu_hypervisor() {
        let error = build_kernel_cmdline("clh", &qemu_config()).unwrap_err();
        assert!(error.to_string().contains("only qemu is supported"));
    }
}
