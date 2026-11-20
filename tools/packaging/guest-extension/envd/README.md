# Envd guest extension prototype

Package [envd](https://github.com/e2b-dev/runtime/tree/main/packages/envd) as a
read-only [guest extension](../../../../docs/design/composable-vm-images.md) for
[PR #14118](https://github.com/kata-containers/kata-containers/pull/14118).
The extension is named `kata-envd` and supplies `bin/kata-envd`, matching the
PR's guest path `/run/kata-extensions/kata-envd/bin/kata-envd`.

The extension supplies the executable. The PR's agent integration opens it before
switching root, then forks and executes it inside each opted-in container.
The workload's init process keeps its PID and starts without waiting for envd.
No envd systemd unit, base-image overlay, or component manifest is needed.

## Build

The standard build scripts can build envd from the source revision and Go
toolchain pinned in `versions.yaml`, then package the extension:

```bash
make rootfs-image-envd-extension-tarball
```

This requires Docker and produces
`build/kata-static-rootfs-image-envd-extension.tar.zst` (the root `build` link
points into `tools/packaging/kata-deploy/local-build/build`). The archive carries
the image, integrity parameters, configuration fragment, executable checksum,
and source provenance under `/opt/kata/share/kata-containers/`.
The image build verifies the payload and integrity tree before packaging.
GitHub Actions builds the extension independently in the amd64 and arm64 asset
matrices, including the standard artifact upload, registry cache, and provenance
steps. Both CI and local builds include it in the Rust runtime's
`kata-static.tar.zst` bundle. It is excluded from the Go runtime and Substrate
bundles. Packaging does not enable envd; runtime configuration and container
opt-in are still required. The extension target has no dependency on an agent,
kernel, or base-image build.

### Package a supplied binary

Use a Linux build machine with the dependencies of
`tools/osbuilder/image-builder/image_builder.sh`, plus `binutils` and
`coreutils`. The EROFS path builds without mounting filesystems or requiring root.

Supply an executable, statically linked Linux envd binary for the guest's CPU
architecture. Build it from a chosen checkout of upstream's runtime repository:

```bash
cd /path/to/e2b-runtime/packages/envd
GOWORK=off CGO_ENABLED=0 GOOS=linux GOARCH=arm64 go build -trimpath -buildvcs=false -o /tmp/envd .
```

Use `GOARCH=amd64` for an x86-64 guest. Keep the upstream source revision with
your build records. The standalone packaging command does not fetch a binary or
choose its version.
Static linking is required because execution happens after switching to the
container's root filesystem, where the guest's dynamic loader is unavailable.
The builder inspects the binary without executing it, so its architecture can
differ from the build machine's architecture.

From this repository, build into a directory that does not exist yet:

```bash
bash tools/packaging/guest-extension/envd/build.sh /tmp/envd /tmp/envd-extension
```

The output contains:

- `kata-containers-envd-extension.img`: partitioned EROFS with a dm-verity tree.
- `root_hash_envd-extension.txt`: matching integrity parameters.
- `configuration-envd.toml`: a runtime configuration fragment containing those parameters.
- `extension-rootfs/` and `envd.sha256`: the packaged executable and its checksum.

Verify the partitions, extracted payload, and dm-verity tree (also requires `jq`):

```bash
bash tools/packaging/guest-extension/envd/test-image.sh /tmp/envd-extension
```

The check also corrupts a temporary copy of the data partition and verifies that
the integrity check rejects it. The original image is unchanged.

## Configure and use

Build the Rust runtime and agent with PR #14118 applied. The packaging prototype
alone does not add the PR's container integration to an existing Kata release.
The base guest must already include Kata's extension mount helpers and generator.

### Helm deployment

Use a kata-deploy image built with this branch's installer and envd component
tarball, plus a guest agent and Rust runtime containing PR #14118. Released
images without those changes cannot provide the integration.

Add `envd: true` to your existing chart values, or pass `--set envd=true` to
your Helm install/upgrade command. For each enabled Rust QEMU or Cloud Hypervisor
shim, and Dragonball, this creates an additional `kata-<shim>-envd`
RuntimeClass on supported amd64/arm64 configurations. For example, use
`kata-qemu-runtime-rs-envd`. `env.multiInstallSuffix: blue` makes that
`kata-qemu-runtime-rs-blue-envd` instead.

Kata-deploy extracts the envd component and writes `25-envd.toml` under the
variant's own configuration directory. The drop-in preserves existing extension
entries, appends `kata-envd` with its integrity parameters, and honors custom
installation prefixes. Containerd is configured to forward the experimental
annotation for these handlers. The base shim's snapshotter and pull settings are
inherited. Guest debugging stays off unless you select a separate debug class.

The plain RuntimeClasses are unchanged. Setting `envd: false` on an upgrade
removes the envd variants and their unused artifacts. Helm does not set workload
annotations or inject environment variables: each target container still opts
in as shown below. In a mixed-architecture cluster, select amd64 or arm64 nodes
for these Pods; the extension is not built for s390x or ppc64le.

### Manual configuration

Copy the extension image to the host path in `configuration-envd.toml`. Merge
that fragment into the runtime configuration, preserving all existing extension
entries. Change `hypervisor.qemu` to `hypervisor.clh` or `hypervisor.dragonball`
when using Cloud Hypervisor or Dragonball. Keep the image and its generated
integrity parameters together.

The runtime attaches the disk with serial `extension-kata-envd` and emits
`kata.extension.kata-envd.verity_params` on the kernel command line. The existing
generator schedules `kata-extension-mount@kata-envd.service`, which verifies and
mounts the extension before kata-agent starts. This mount service is part of
Kata's existing extension infrastructure; it does not launch envd.

Set the PR's experimental Pod annotation and enable envd on the selected container:

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: envd-example
  annotations:
    io.katacontainers.config.runtime.experimental_envd: >-
      {"args":["-isnotfc","-no-cgroups","-port","49983"]}
spec:
  runtimeClassName: kata-qemu-runtime-rs-envd
  nodeSelector:
    kubernetes.io/arch: amd64
  containers:
    - name: app
      image: ubuntu:resolute
      command: ["sleep", "infinity"]
      env:
        - name: KATA_ENVD_ENABLED
          value: "true"
      readinessProbe:
        httpGet:
          path: /health
          port: 49983
        periodSeconds: 1
```

For containerd CRI, add the experimental annotation to the runtime's
`pod_annotations` allowlist when configuring manually. Helm's envd handlers do
this automatically. Use the RuntimeClass mapped to your configured Rust runtime
for manual installations, and `kubernetes.io/arch: arm64` for ARM nodes.
The arguments above match upstream envd's flag-based CLI. Check them against the
binary you supply; `serve --addr` in the PR's examples describes a different CLI.
`-isnotfc` disables Firecracker metadata integration; `-no-cgroups` leaves cgroup
management to Kata. The workload image must provide any users, shells, and
commands needed by envd requests.

The readiness probe checks HTTP availability separately from workload startup.
When connectivity is configured, `/health` is also available at the Pod IP on
port `49983`. The prototype does not configure authentication, network policy,
host listeners, or port forwarding; use an isolated test workload. Envd's API
can execute commands with the selected container's privileges.

## Lifecycle and limitations

The PR starts envd for each opted-in container creation. It inherits that
container's namespaces, root filesystem, user, environment, working directory,
and cgroup. Use distinct ports if multiple opted-in containers share a network
namespace. The extension stays mounted read-only in the guest and does not need
to be mounted inside the workload.

Cleanup follows the container's PID namespace and cgroup lifecycle. Kata does
not supervise or independently restart envd, and its logs are discarded by the
PR's launcher. An HTTP readiness probe reports availability but does not restart
it. Guest seccomp must permit the fork, `execveat`, and envd's own system calls;
agent policy must permit the experimental annotation.

This prototype builds a measured EROFS extension through the existing image
builder. It does not add automatic deployment, Firecracker
extension attachment, or agent-as-init mounting support. Full validation requires
booting a Kata guest with the PR and exercising envd inside a workload container.
