# Helm Configuration

## Parameters

The helm chart provides a comprehensive set of configuration options. You may view the parameters and their descriptions by going to the [GitHub source](https://github.com/kata-containers/kata-containers/blob/main/tools/packaging/kata-deploy/helm-chart/kata-deploy/values.yaml) or by using helm:

```sh
# List available kata-deploy chart versions:
#   helm search repo kata-deploy-charts/kata-deploy --versions
#
# Then replace X.Y.Z below with the desired chart version:
helm show values --version X.Y.Z oci://ghcr.io/kata-containers/kata-deploy-charts/kata-deploy
```

### shims

Kata ships with a number of pre-built artifacts and runtimes. You may selectively enable or disable specific shims. For example:

```yaml title="values.yaml"
shims:
  disableAll: true
  qemu:
    enabled: true
  qemu-nvidia-gpu:
    enabled: true
  qemu-nvidia-gpu-snp:
    enabled: false

```

Shims can also have configuration options specific to them:

```yaml
  qemu-nvidia-gpu:
    enabled: ~
    supportedArches:
      - amd64
    allowedHypervisorAnnotations: []
    containerd:
      snapshotter: ""
    runtimeClass:
      # This label is automatically added by gpu-operator. Override it
      # if you want to use a different label.
      # Uncomment once GPU Operator v26.3 is out
      # nodeSelector:
        # nvidia.com/cc.ready.state: "false"
```

It's best to reference the default `values.yaml` file above for more details.

### Custom Runtimes

Kata allows you to create custom runtime configurations. This is done by overlaying one of the pre-existing runtime configs with user-provided configs. For example, we can use the `qemu-nvidia-gpu` as a base config and overlay our own parameters to it:

```yaml
customRuntimes:
  enabled: false
  runtimes:
    my-gpu-runtime:
      baseConfig: "qemu-nvidia-gpu"  # Required: existing config to use as base
      dropIn: |                      # Optional: overrides via config.d mechanism
        [hypervisor.qemu]
        default_memory = 1024
        default_vcpus = 4
      runtimeClass: |
        kind: RuntimeClass
        apiVersion: node.k8s.io/v1
        metadata:
          name: kata-my-gpu-runtime
          labels:
            app.kubernetes.io/managed-by: kata-deploy
        handler: kata-my-gpu-runtime
        overhead:
          podFixed:
            memory: "640Mi"
            cpu: "500m"
        scheduling:
          nodeSelector:
            katacontainers.io/kata-runtime: "true"
      # Optional: CRI-specific configuration
      containerd:
        snapshotter: "nydus"  # Configure containerd snapshotter (nydus, erofs, etc.)
      crio:
        pullType: "guest-pull"  # Configure CRI-O runtime_pull_image = true
```

Again, view the default [`values.yaml`](#parameters) file for more details.

## Deployment Modes (DaemonSet vs Job)

The chart can install Kata on nodes in one of two ways, selected with the
top-level `deploymentMode` value:

- **`daemonset`** (default): the long-running `kata-deploy` DaemonSet installs
  Kata on every matching node and reverts it when the pod is terminated (i.e. on
  uninstall). This is the historical behavior and is unchanged.
- **`job`**: a short-lived, staged per-node install `Job` (one per targeted
  node) runs the install pipeline as ordered `initContainers` and then exits:

  ```
  host-check -> artifacts -> cri   (initContainers)  ->  label (main)
  ```

  On `helm uninstall`, a per-node `pre-delete` hook Job runs the same pipeline
  in reverse (`unlabel -> revert-cri -> remove-artifacts`). Unlike the DaemonSet,
  **nothing keeps running on the node after installation completes.**

```yaml title="values.yaml"
deploymentMode: job
```

### Adding nodes in `job` mode

The set of per-node Jobs is computed at `helm install` / `helm upgrade` time by
enumerating the cluster's nodes. There is **no controller watching for new
nodes**, so when you add nodes later, re-run `helm upgrade` to create install
Jobs for them:

```sh
helm upgrade kata-deploy "${CHART}" --version "${VERSION}" --reuse-values
```

Each per-node stage is idempotent (it skips when already applied), so the
upgrade only does real work on the newly added nodes.

### Choosing which nodes get a Job

In `job` mode, node selection is configured under the `job` key, with the
following precedence (highest first):

1. `job.nodes`: an explicit list of node names, used verbatim.
2. `job.nodeSelector` (an equality map) **ANDed with**
   `job.nodeSelectorExpressions` (Kubernetes label-selector requirements using
   the operators `In`, `NotIn`, `Exists`, `DoesNotExist`).

By **default the expressions target worker (non-control-plane) nodes**, so no
custom node labeling is required (this differs from the DaemonSet `nodeSelector`
examples above, which rely on you labeling nodes). Override as needed:

```yaml title="values.yaml"
# Target nodes carrying a specific label:
job:
  nodeSelector:
    kata-containers: "enabled"

# Target every node, including control-plane (e.g. single-node clusters / CI):
job:
  nodeSelectorExpressions: []

# Richer expressions:
job:
  nodeSelectorExpressions:
    - { key: kubernetes.io/os, operator: In, values: ["linux"] }
    - { key: node-role.kubernetes.io/control-plane, operator: DoesNotExist }

# Pin to explicit nodes (also handy for `helm template`):
job:
  nodes: ["worker-1", "worker-2"]
```

### Choosing which nodes are cleaned up on uninstall

The cleanup Jobs are Helm **`pre-delete` hooks**. Helm renders and *stores* hook
manifests at install/upgrade time and replays them verbatim on `helm uninstall`
— it does **not** re-template at delete time, so any `lookup` is evaluated at
install/upgrade time, never at uninstall. The cleanup node set therefore has to
be derivable at render time. (A label-based default such as "every node with the
`katacontainers.io/kata-runtime` label" cannot work: that label is applied by
the install Jobs, which run *after* the cleanup hook has already been rendered
and stored, so the lookup would always be empty and nothing would clean up.)

By **default, uninstall mirrors the install selection**, so it targets exactly
the nodes install targeted. That set is frozen at the last
`helm install`/`helm upgrade` — which is precisely where Kata was installed —
and it stays correct even if node labels drift afterwards, since it reflects the
install-time state rather than re-evaluating a selector at delete time.

You can override it under `job.cleanup`, with the same precedence/semantics as
install (`cleanup.nodes`, then `cleanup.nodeSelector` ANDed with
`cleanup.nodeSelectorExpressions`). Setting any of these disables the
install-mirror and uses your selection instead:

```yaml title="values.yaml"
# Only uninstall from specific nodes:
job:
  cleanup:
    nodes: ["worker-1"]

# Use an explicit selector instead of mirroring install:
job:
  cleanup:
    nodeSelectorExpressions:
      - { key: node-role.kubernetes.io/control-plane, operator: DoesNotExist }
```

See the default [`values.yaml`](#parameters) for the remaining `job.*` options
(e.g. `ttlSecondsAfterFinished`, `backoffLimit`).

## Examples

We provide a few examples that you can pass to helm via the `-f`/`--values` flag.

### [`try-kata-tee.values.yaml`](https://github.com/kata-containers/kata-containers/blob/main/tools/packaging/kata-deploy/helm-chart/kata-deploy/try-kata-tee.values.yaml)

This file enables only the TEE (Trusted Execution Environment) shims for confidential computing:

```sh
helm install kata-deploy oci://ghcr.io/kata-containers/kata-deploy-charts/kata-deploy \
  --version VERSION \
  -f try-kata-tee.values.yaml
```

Includes:

- `qemu-snp` - AMD SEV-SNP (amd64)
- `qemu-tdx` - Intel TDX (amd64)
- `qemu-se` - IBM Secure Execution for Linux (SEL) (s390x)
- `qemu-se-runtime-rs` - IBM Secure Execution for Linux (SEL) Rust runtime (s390x)
- `qemu-coco-dev` - Confidential Containers development (amd64, s390x)
- `qemu-coco-dev-runtime-rs` - Confidential Containers development Rust runtime (amd64, arm64, s390x)

### [`try-kata-nvidia-gpu.values.yaml`](https://github.com/kata-containers/kata-containers/blob/main/tools/packaging/kata-deploy/helm-chart/kata-deploy/try-kata-nvidia-gpu.values.yaml)

This file enables only the NVIDIA GPU-enabled shims:

```sh
helm install kata-deploy oci://ghcr.io/kata-containers/kata-deploy-charts/kata-deploy \
  --version VERSION \
  -f try-kata-nvidia-gpu.values.yaml
```

Includes:

- `qemu-nvidia-gpu` - Standard NVIDIA GPU support (amd64)
- `qemu-nvidia-gpu-snp` - NVIDIA GPU with AMD SEV-SNP (amd64)
- `qemu-nvidia-gpu-tdx` - NVIDIA GPU with Intel TDX (amd64)

### `nodeSelector`

We can deploy Kata only to specific nodes using `nodeSelector`

```sh
# First, label the nodes where you want kata-containers to be installed
$ kubectl label nodes worker-node-1 kata-containers=enabled
$ kubectl label nodes worker-node-2 kata-containers=enabled

# Then install the chart with `nodeSelector`
$ helm install kata-deploy \
  --set nodeSelector.kata-containers="enabled" \
  "${CHART}" --version  "${VERSION}"
```

You can also use a values file:

```yaml title="values.yaml"
nodeSelector:
  kata-containers: "enabled"
  node-type: "worker"
```

```sh
$ helm install kata-deploy -f values.yaml "${CHART}" --version "${VERSION}"
```

### Multiple Kata installations on the Same Node

For debugging, testing and other use-case it is possible to deploy multiple
versions of Kata on the very same node. All the needed artifacts are getting the
`multiInstallSuffix` appended to distinguish each installation. **BEWARE** that one
needs at least **containerd-2.0** since this version has drop-in conf support
which is a prerequisite for the `multiInstallSuffix` to work properly.

```sh
$ helm install kata-deploy-cicd       \
  -n kata-deploy-cicd                 \
  --set env.multiInstallSuffix=cicd   \
  --set env.debug=true                \
  "${CHART}" --version  "${VERSION}"
```

Note: `runtimeClasses` are automatically created by Helm (via
      `runtimeClasses.enabled=true`, which is the default).

Now verify the installation by examining the `runtimeClasses`:

```sh
$ kubectl get runtimeClasses
NAME                            HANDLER                         AGE
kata-clh-cicd                   kata-clh-cicd                   77s
kata-clh-runtime-rs-cicd        kata-clh-runtime-rs-cicd        77s
kata-dragonball-cicd            kata-dragonball-cicd            77s
kata-fc-cicd                    kata-fc-cicd                    77s
kata-qemu-cicd                  kata-qemu-cicd                  77s
kata-qemu-coco-dev-cicd         kata-qemu-coco-dev-cicd         77s
kata-qemu-nvidia-gpu-cicd       kata-qemu-nvidia-gpu-cicd       77s
kata-qemu-nvidia-gpu-snp-cicd   kata-qemu-nvidia-gpu-snp-cicd   77s
kata-qemu-nvidia-gpu-tdx-cicd   kata-qemu-nvidia-gpu-tdx-cicd   76s
kata-qemu-runtime-rs-cicd       kata-qemu-runtime-rs-cicd       77s
kata-qemu-se-runtime-rs-cicd    kata-qemu-se-runtime-rs-cicd    77s
kata-qemu-snp-cicd              kata-qemu-snp-cicd              77s
kata-qemu-tdx-cicd              kata-qemu-tdx-cicd              77s
kata-stratovirt-cicd            kata-stratovirt-cicd            77s
```

## RuntimeClass Node Selectors for TEE Shims

**Manual configuration:** Any `nodeSelector` you set under `shims.<shim>.runtimeClass.nodeSelector`
is **always applied** to that shim's RuntimeClass, whether or not NFD is present. Use this when
you want to pin TEE workloads to specific nodes (e.g. without NFD, or with custom labels).

**Auto-inject when NFD is present:** If you do *not* set a `runtimeClass.nodeSelector` for a
TEE shim, the chart can **automatically inject** NFD-based labels when NFD is detected in the
cluster (deployed by this chart with `node-feature-discovery.enabled=true` or found externally):

- AMD SEV-SNP shims: `amd.feature.node.kubernetes.io/snp: "true"`
- Intel TDX shims: `intel.feature.node.kubernetes.io/tdx: "true"`
- IBM Secure Execution for Linux (SEL) shims (s390x): `feature.node.kubernetes.io/cpu-security.se.enabled: "true"`

The chart uses Helm's `lookup` function to detect NFD (by looking for the
`node-feature-discovery-worker` DaemonSet). Auto-inject only runs when NFD is detected and
no manual `runtimeClass.nodeSelector` is set for that shim.

**Note**: NFD detection requires cluster access. During `helm template` (dry-run without a
cluster), external NFD is not seen, so auto-injected labels are not added. Manual
`runtimeClass.nodeSelector` values are still applied in all cases.

## Customizing Configuration with Drop-in Files

When kata-deploy installs Kata Containers, the base configuration files should not
be modified directly. Instead, use drop-in configuration files to customize
settings. This approach ensures your customizations survive kata-deploy upgrades.

### How Drop-in Files Work

The Kata runtime reads the base configuration file and then applies any `.toml`
files found in the `config.d/` directory alongside it. Files are processed in
alphabetical order, with later files overriding earlier settings.

### Creating Custom Drop-in Files

To add custom settings, create a `.toml` file in the appropriate `config.d/`
directory. Use a numeric prefix to control the order of application.

**Reserved prefixes** (used by kata-deploy):

- `10-*`: Core kata-deploy settings
- `20-*`: Debug settings
- `30-*`: Kernel parameters

**Recommended prefixes for custom settings**: `50-89`

### Drop-In Config Examples

#### Adding Custom Kernel Parameters

```bash
# SSH into the node or use kubectl exec
sudo mkdir -p /opt/kata/share/defaults/kata-containers/runtimes/qemu/config.d/
sudo cat > /opt/kata/share/defaults/kata-containers/runtimes/qemu/config.d/50-custom.toml << 'EOF'
[hypervisor.qemu]
kernel_params = "my_param=value"
EOF
```

#### Changing Default Memory Size

```bash
sudo cat > /opt/kata/share/defaults/kata-containers/runtimes/qemu/config.d/50-memory.toml << 'EOF'
[hypervisor.qemu]
default_memory = 4096
EOF
```
