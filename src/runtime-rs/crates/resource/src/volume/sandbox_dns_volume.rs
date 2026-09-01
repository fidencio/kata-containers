// Copyright (c) 2025 NVIDIA Corporation
//
// SPDX-License-Identifier: Apache-2.0
//

use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;
use hypervisor::device::device_manager::DeviceManager;
use kata_sys_util::mount::get_mount_path;
use oci_spec::runtime as oci;
use tokio::sync::RwLock;

use super::Volume;

/// Where the agent materialises the sandbox resolv.conf during create_sandbox,
/// out of the `dns` field of CreateSandboxRequest.
const KATA_GUEST_SANDBOX_DNS_FILE: &str = "/run/kata-containers/sandbox/resolv.conf";

const GUEST_DNS_FILE: &str = "/etc/resolv.conf";

pub(crate) struct SandboxDnsVolume {
    mount: oci::Mount,
}

impl SandboxDnsVolume {
    pub fn new(mount: &oci::Mount) -> Result<Self> {
        let mut mount = mount.clone();
        mount.set_source(Some(Path::new(KATA_GUEST_SANDBOX_DNS_FILE).to_path_buf()));

        Ok(Self { mount })
    }
}

#[async_trait]
impl Volume for SandboxDnsVolume {
    fn get_volume_mount(&self) -> Result<Vec<oci::Mount>> {
        Ok(vec![self.mount.clone()])
    }

    fn get_storage(&self) -> Result<Vec<agent::Storage>> {
        Ok(vec![])
    }

    fn get_device_id(&self) -> Result<Option<String>> {
        Ok(None)
    }

    async fn cleanup(&self, _device_manager: &RwLock<DeviceManager>) -> Result<()> {
        Ok(())
    }
}

/// The host gives every container in the pod the same sandbox-scoped
/// resolv.conf, so pointing them all at the one guest copy reproduces that.
/// copy_file is what diverges, by handing each container an isolated copy.
pub(crate) fn is_sandbox_dns_mount(m: &oci::Mount) -> bool {
    get_mount_path(&Some(m.destination().clone())) == GUEST_DNS_FILE
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci_spec::runtime as oci;

    fn mount_at(destination: &str) -> oci::Mount {
        let mut m = oci::Mount::default();
        m.set_destination(Path::new(destination).to_path_buf());
        m.set_source(Some(Path::new("/var/lib/containerd/sandboxes/x/resolv.conf").to_path_buf()));
        m
    }

    #[test]
    fn only_matches_resolv_conf() {
        assert!(is_sandbox_dns_mount(&mount_at("/etc/resolv.conf")));
        assert!(!is_sandbox_dns_mount(&mount_at("/etc/hosts")));
        assert!(!is_sandbox_dns_mount(&mount_at("/etc/hostname")));
    }

    #[test]
    fn rewrites_source_to_the_guest_copy() {
        let volume = SandboxDnsVolume::new(&mount_at("/etc/resolv.conf")).unwrap();
        let mounts = volume.get_volume_mount().unwrap();

        assert_eq!(mounts.len(), 1);
        assert_eq!(
            mounts[0].source().as_ref().unwrap().as_path(),
            Path::new(KATA_GUEST_SANDBOX_DNS_FILE)
        );
        assert_eq!(
            mounts[0].destination().as_path(),
            Path::new(GUEST_DNS_FILE)
        );
        assert!(volume.get_storage().unwrap().is_empty());
        assert!(volume.get_device_id().unwrap().is_none());
    }
}
