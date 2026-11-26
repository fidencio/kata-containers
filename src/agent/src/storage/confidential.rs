// Copyright (c) 2026 Kata Containers contributors
//
// SPDX-License-Identifier: Apache-2.0

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use kata_types::dmverity::{
    create_dmverity_device, destroy_dm_device_at_path, destroy_partition_dmverity_device,
    DmVerityInfo,
};
use kata_types::mount::{
    CONFIDENTIAL_LAYER_DESCRIPTOR, CONFIDENTIAL_UPPER_MOUNT, KATA_VIRTUAL_VOLUME_IMAGE_GUEST_PULL,
    MAX_CONFIDENTIAL_DESCRIPTOR_BYTES,
};
use nix::sys::stat::{major, minor};
use protocols::agent::Storage;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{UnparsedPublicKey, ED25519};
use rustjail::container::WAIT_PID_LOCKER;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{has_confidential_storage_option, CONFIDENTIAL_ROOTFS_POLICY};
const KEY_DIRECTORY: &str = "/etc/kata-containers/confidential-rootfs.keys";
const SIGNATURE_DOMAIN: &[u8] = b"confidential-snapshotter/layer/v1\0";
const VERITY_BLOCK_SIZE: u64 = 4096;
const SECTOR_SIZE: u64 = 512;
const MAX_LAYERS: usize = 128;
const KEY_BYTES: usize = 32;

#[derive(Deserialize)]
struct Policy {
    manifests: Vec<String>,
    public_key: String,
}

#[derive(Deserialize)]
struct Envelope {
    payload: String,
    signature: String,
}

#[derive(Deserialize)]
struct Verity {
    data_blocks: u64,
    root: String,
    salt: String,
}

#[derive(Deserialize)]
pub(super) struct Layer {
    version: u32,
    manifest: String,
    layer: String,
    chain_id: String,
    blob: String,
    key_ref: String,
    size: u64,
    diff_id: String,
    config: String,
    index: usize,
    count: usize,
    verity: Verity,
}

fn read_policy() -> Result<Option<Policy>> {
    match fs::read(CONFIDENTIAL_ROOTFS_POLICY) {
        Ok(bytes) => {
            let policy: Policy =
                serde_json::from_slice(&bytes).context("read guest rootfs policy")?;
            ensure!(
                !policy.manifests.is_empty(),
                "missing authorized confidential manifests"
            );
            for manifest in &policy.manifests {
                sha256_hex(manifest)?;
            }
            Ok(Some(policy))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).context("open guest rootfs policy"),
    }
}

pub(crate) fn check_rootfs_policy(storages: &[Storage], root: Option<&Path>) -> Result<()> {
    let confidential: Vec<_> = storages
        .iter()
        .filter(|storage| has_confidential_storage_option(storage))
        .collect();
    let policy = read_policy()?;
    ensure!(
        policy.is_some() != confidential.is_empty(),
        "guest rootfs policy and confidential storage do not match"
    );
    if !confidential.is_empty() {
        let root = root.context("confidential rootfs requires a guest mount point")?;
        ensure!(
            storages
                .iter()
                .all(|storage| storage.driver != KATA_VIRTUAL_VOLUME_IMAGE_GUEST_PULL),
            "guest image pull cannot be combined with a confidential rootfs"
        );
        ensure!(
            confidential.iter().all(|storage| {
                super::multi_layer_erofs::is_multi_layer_storage(storage)
                    && !storage.shared
                    && Path::new(&storage.mount_point) == root
            }),
            "confidential devices must belong to the container rootfs"
        );
        ensure!(
            storages
                .iter()
                .filter(|storage| Path::new(&storage.mount_point) == root)
                .all(has_confidential_storage_option),
            "ordinary storage cannot use the confidential rootfs mount point"
        );
    }
    Ok(())
}

fn sha256_hex(value: &str) -> Result<&str> {
    let hex = value
        .strip_prefix("sha256:")
        .context("expected SHA-256 digest")?;
    ensure!(valid_hash(hex), "invalid SHA-256 digest");
    Ok(hex)
}

fn valid_hash(hex: &str) -> bool {
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn verify_group(upper: &Storage, lowers: &[&Storage]) -> Result<Vec<Layer>> {
    let policy = read_policy()?.context("confidential rootfs requires a guest trust policy")?;
    verify_layers(&policy, upper, lowers)
}

fn verify_layers(policy: &Policy, upper: &Storage, lowers: &[&Storage]) -> Result<Vec<Layer>> {
    ensure!(
        upper
            .driver_options
            .iter()
            .filter(|option| *option == CONFIDENTIAL_UPPER_MOUNT)
            .count()
            == 1
            && !upper
                .driver_options
                .iter()
                .any(|option| option.starts_with(CONFIDENTIAL_LAYER_DESCRIPTOR)),
        "expected one private encrypted upper"
    );
    ensure!(
        (1..=MAX_LAYERS).contains(&lowers.len()),
        "invalid confidential layer count"
    );
    let public = STANDARD
        .decode(&policy.public_key)
        .context("decode worker public key")?;
    ensure!(public.len() == 32, "expected a raw Ed25519 public key");
    let verifier = UnparsedPublicKey::new(&ED25519, public);
    let mut verified = Vec::with_capacity(lowers.len());
    // Incoming layers have overlay priority order, while the signed chain
    // starts at the bottom of the image.
    for (index, storage) in lowers.iter().rev().enumerate() {
        ensure!(
            !storage.shared
                && !storage
                    .driver_options
                    .iter()
                    .any(|option| option == CONFIDENTIAL_UPPER_MOUNT),
            "expected a read-only confidential lower"
        );
        let mut envelopes = storage
            .driver_options
            .iter()
            .filter_map(|option| option.strip_prefix(CONFIDENTIAL_LAYER_DESCRIPTOR));
        let envelope = envelopes
            .next()
            .context("missing signed layer descriptor")?;
        ensure!(
            envelopes.next().is_none() && envelope.len() <= MAX_CONFIDENTIAL_DESCRIPTOR_BYTES,
            "invalid signed layer descriptor"
        );
        let envelope: Envelope =
            serde_json::from_str(envelope).context("parse signed layer envelope")?;
        let payload = STANDARD
            .decode(envelope.payload)
            .context("decode signed layer payload")?;
        let signature = STANDARD
            .decode(envelope.signature)
            .context("decode layer signature")?;
        let mut message = Vec::with_capacity(SIGNATURE_DOMAIN.len() + payload.len());
        message.extend_from_slice(SIGNATURE_DOMAIN);
        message.extend_from_slice(&payload);
        verifier
            .verify(&message, &signature)
            .map_err(|_| anyhow!("invalid confidential layer signature"))?;
        let layer: Layer = serde_json::from_slice(&payload).context("parse verified layer")?;
        let previous: Option<&Layer> = verified.last();
        layer.validate(policy, index, lowers.len(), previous)?;
        verified.push(layer);
    }
    verified.reverse();
    Ok(verified)
}

impl Layer {
    fn validate(
        &self,
        policy: &Policy,
        index: usize,
        count: usize,
        previous: Option<&Self>,
    ) -> Result<()> {
        for digest in [
            &self.manifest,
            &self.layer,
            &self.chain_id,
            &self.blob,
            &self.diff_id,
            &self.config,
        ] {
            sha256_hex(digest)?;
        }
        let chain = match previous {
            Some(previous) => {
                ensure!(
                    self.config == previous.config && self.manifest == previous.manifest,
                    "layers have different OCI manifest or config digests"
                );
                let mut hash = Sha256::new();
                hash.update(previous.chain_id.as_bytes());
                hash.update(b" ");
                hash.update(self.diff_id.as_bytes());
                format!("sha256:{}", encode_hex(&hash.finalize()))
            }
            None => self.diff_id.clone(),
        };
        ensure!(
            self.version == 1
                && policy.manifests.contains(&self.manifest)
                && self.index == index
                && self.count == count
                && self.chain_id == chain,
            "descriptor does not match the authorized manifest or layer order"
        );
        ensure!(
            self.key_ref == format!("fixture:{}", self.diff_id),
            "unsupported confidential layer key reference"
        );
        ensure!(
            self.size.is_multiple_of(VERITY_BLOCK_SIZE)
                && self.verity.data_blocks > 0
                && self.verity.data_blocks < self.size / VERITY_BLOCK_SIZE
                && valid_hash(&self.verity.root)
                && valid_hash(&self.verity.salt),
            "invalid confidential layer geometry"
        );
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct RootfsMappings {
    mount_point: String,
    layers: Vec<Mapping>,
}

impl RootfsMappings {
    pub(super) fn new(mount_point: String, layers: Vec<Mapping>) -> Self {
        Self {
            mount_point,
            layers,
        }
    }
}

impl Drop for RootfsMappings {
    fn drop(&mut self) {
        unmount(Path::new(&self.mount_point));
        self.layers.clear();
    }
}

pub(super) fn unmount(mount: &Path) {
    if let Err(err) = nix::mount::umount(mount) {
        if err != nix::errno::Errno::EINVAL && err != nix::errno::Errno::ENOENT {
            slog::warn!(slog_scope::logger(), "failed to unmount confidential backing"; "error" => %err);
        }
    }
}

// Own the mappings and backing mount until overlay setup completes. Dropping a
// partially setup layer releases resources in the opposite order.
#[derive(Debug)]
pub(crate) struct Mapping {
    name: String,
    verity: Option<String>,
    mounted: Option<PathBuf>,
    opened: bool,
}

impl Mapping {
    fn new(name: String) -> Self {
        Self {
            name,
            verity: None,
            mounted: None,
            opened: false,
        }
    }

    fn path(&self) -> String {
        format!("/dev/mapper/{}", self.name)
    }

    pub(super) fn mounted(&mut self, path: &Path) {
        self.mounted = Some(path.to_path_buf());
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        let logger = slog_scope::logger();
        if let Some(mount) = &self.mounted {
            unmount(mount);
        }
        if self.opened {
            if let Err(err) = destroy_dm_device_at_path(&self.path()) {
                slog::warn!(logger, "failed to close confidential mapping"; "error" => %err);
            }
        }
        if let Some(verity) = &self.verity {
            if let Err(err) = destroy_partition_dmverity_device(verity, &logger) {
                slog::warn!(logger, "failed to remove confidential verity device"; "error" => %err);
            }
        }
    }
}

pub(super) async fn open_lower(device: &str, layer: &Layer) -> Result<(String, Mapping)> {
    let meta = fs::metadata(device)?;
    ensure!(
        meta.file_type().is_block_device(),
        "confidential lower must be a block device"
    );
    let rdev = meta.rdev();
    let sectors: u64 = fs::read_to_string(format!(
        "/sys/dev/block/{}:{}/size",
        major(rdev),
        minor(rdev)
    ))?
    .trim()
    .parse()
    .context("read confidential partition sectors")?;
    ensure!(
        sectors.checked_mul(SECTOR_SIZE) == Some(layer.size),
        "confidential partition size mismatch"
    );
    let key = File::open(Path::new(KEY_DIRECTORY).join(sha256_hex(&layer.diff_id)?))
        .context("open guest fixture key")?;
    ensure!(
        key.metadata()?.is_file() && key.metadata()?.len() == KEY_BYTES as u64,
        "invalid guest fixture key"
    );
    let verity = DmVerityInfo {
        hashtype: "sha256".into(),
        hash: layer.verity.root.clone(),
        blocknum: layer.verity.data_blocks,
        blocksize: VERITY_BLOCK_SIZE,
        hashsize: VERITY_BLOCK_SIZE,
        offset: layer.verity.data_blocks * VERITY_BLOCK_SIZE,
        salt: Some(layer.verity.salt.clone()),
        hash_type: 1,
        no_superblock: true,
    };
    let mut mapping = Mapping::new(mapping_name()?);
    let verified = create_dmverity_device(&verity, Path::new(device)).await?;
    mapping.verity = Some(verified.clone());
    // Hold the lock until the waiter owns the exit status, even if the RPC is cancelled.
    let child_wait = WAIT_PID_LOCKER.clone().lock_owned().await;
    tokio::task::spawn_blocking(move || {
        let _child_wait = child_wait;
        run(Command::new("cryptsetup")
            .args([
                "open",
                "--readonly",
                "--type",
                "luks2",
                "--key-file",
                "-",
                &verified,
                &mapping.name,
            ])
            .stdin(Stdio::from(key)))?;
        mapping.opened = true;
        Ok((mapping.path(), mapping))
    })
    .await?
}

pub(super) async fn open_upper(device: String) -> Result<(String, Mapping)> {
    // Hold the lock until the waiter owns the exit status, even if the RPC is cancelled.
    let child_wait = WAIT_PID_LOCKER.clone().lock_owned().await;
    tokio::task::spawn_blocking(move || {
        let _child_wait = child_wait;
        ensure!(
            fs::metadata(&device)?.file_type().is_block_device(),
            "private upper must be a block device"
        );
        let mut key = Zeroizing::new([0; KEY_BYTES]);
        SystemRandom::new()
            .fill(key.as_mut())
            .map_err(|_| anyhow!("generate ephemeral upper key"))?;
        let mut mapping = Mapping::new(mapping_name()?);
        command(
            "cryptsetup",
            &[
                "luksFormat",
                "--batch-mode",
                "--type",
                "luks2",
                "--pbkdf",
                "pbkdf2",
                "--pbkdf-force-iterations",
                "1000",
                "--key-file",
                "-",
                &device,
            ],
            Some(key.as_ref()),
        )?;
        command(
            "cryptsetup",
            &[
                "open",
                "--type",
                "luks2",
                "--key-file",
                "-",
                &device,
                &mapping.name,
            ],
            Some(key.as_ref()),
        )?;
        mapping.opened = true;
        command(
            "mkfs.ext4",
            &["-q", "-F", "-E", "nodiscard", &mapping.path()],
            None,
        )?;
        Ok((mapping.path(), mapping))
    })
    .await?
}

fn mapping_name() -> Result<String> {
    let mut nonce = [0; 16];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| anyhow!("generate mapping name"))?;
    let suffix = encode_hex(&nonce);
    Ok(format!("kata-confidential-{suffix}"))
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn command(program: &str, args: &[&str], input: Option<&[u8]>) -> Result<()> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(input) = input {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn {program}"))?;
        let mut stdin = child.stdin.take().context("open command stdin")?;
        let written = stdin.write_all(input);
        drop(stdin);
        let output = child
            .wait_with_output()
            .with_context(|| format!("wait for {program}"))?;
        written?;
        check_output(program, output)
    } else {
        run(&mut command)
    }
}

fn run(command: &mut Command) -> Result<()> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command
        .output()
        .with_context(|| format!("execute {program}"))?;
    check_output(&program, output)
}

fn check_output(program: &str, output: std::process::Output) -> Result<()> {
    ensure!(
        output.status.success(),
        "{program} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use serde_json::json;

    #[test]
    fn signed_layers_require_authorized_manifest_and_overlay_order() {
        let signing = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
        let manifest = format!("sha256:{}", "a".repeat(64));
        let diff = format!("sha256:{}", "b".repeat(64));
        let chain = format!(
            "sha256:{}",
            encode_hex(&Sha256::digest(format!("{diff} {diff}").as_bytes()))
        );
        let policy = Policy {
            manifests: vec![manifest.clone()],
            public_key: STANDARD.encode(signing.public_key()),
        };
        let upper = Storage {
            driver_options: vec![CONFIDENTIAL_UPPER_MOUNT.into()],
            ..Default::default()
        };
        let lower = |index: usize, chain_id: &str| {
            let payload = serde_json::to_vec(&json!({
                "version": 1, "manifest": manifest, "config": manifest, "layer": manifest,
                "blob": manifest, "chain_id": chain_id, "size": 8192,
                "key_ref": format!("fixture:{diff}"), "diff_id": diff,
                "index": index, "count": 2,
                "verity": {"data_blocks": 1, "root": "0".repeat(64), "salt": "1".repeat(64)},
            }))
            .unwrap();
            let signature = signing.sign(&[SIGNATURE_DOMAIN, &payload].concat());
            let envelope = json!({
                "payload": STANDARD.encode(payload),
                "signature": STANDARD.encode(signature),
            });
            Storage {
                driver_options: vec![format!("{CONFIDENTIAL_LAYER_DESCRIPTOR}{envelope}")],
                ..Default::default()
            }
        };
        let base = lower(0, &diff);
        let mut top = lower(1, &chain);
        assert!(verify_layers(&policy, &upper, &[&top, &base]).is_ok());
        assert!(verify_layers(&policy, &upper, &[&base, &top]).is_err());
        let mismatched = Policy {
            manifests: vec![diff],
            public_key: policy.public_key.clone(),
        };
        assert!(verify_layers(&mismatched, &upper, &[&top, &base]).is_err());
        top.driver_options[0].replace_range(CONFIDENTIAL_LAYER_DESCRIPTOR.len().., "{}");
        assert!(verify_layers(&policy, &upper, &[&top, &base]).is_err());
    }
}
