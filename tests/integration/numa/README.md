# NUMA Verification Playbook

Ansible playbook that exercises every step in
[docs/how-to/how-to-use-numa-with-kata.md](../../../docs/how-to/how-to-use-numa-with-kata.md).

Supports both **multi-NUMA** hosts (full verification of guest topology,
vCPU pinning, memory binding, VFIO placement) and **single-NUMA** hosts
(verifies the "harmless no-op" path where the runtime correctly skips
multi-NUMA topology).

## Prerequisites

| Requirement | Details |
|---|---|
| Kubernetes node | Any node -- multi-NUMA (>= 2 nodes) or single-NUMA |
| Kubernetes | Fresh install with kubelet on the node |
| GPU Operator | Already deployed (if testing GPU passthrough) |
| Helm 3 | Installed and on `$PATH` |
| Ansible | >= 2.12 |
| `numactl`, `crictl` | Installed on the node |
| Root access | The playbook runs with `become: true` |

## Quick Start

```bash
# Non-TEE GPU runtime
ansible-playbook -i inventory.ini verify-numa.yml \
    -e gpu_runtime_class=kata-qemu-nvidia-gpu

# AMD SEV-SNP GPU runtime
ansible-playbook -i inventory.ini verify-numa.yml \
    -e gpu_runtime_class=kata-qemu-nvidia-gpu-snp

# Skip GPU tests entirely
ansible-playbook -i inventory.ini verify-numa.yml \
    -e skip_gpu_test=true
```

## Variables

| Variable | Default | Description |
|---|---|---|
| `kata_deploy_image` | `ghcr.io/kata-containers/kata-deploy-ci` | kata-deploy container image |
| `kata_deploy_tag` | `12948-b56b2b8a8d1e830585c88d82d4084aca5cbf9795-amd64` | Image tag |
| `gpu_runtime_class` | *(required)* | GPU RuntimeClass name (`kata-qemu-nvidia-gpu`, `-snp`, `-tdx`) |
| `numa_runtime_class` | `kata-qemu-numa` | RuntimeClass created by the NUMA drop-in |
| `helm_release_name` | `kata-deploy` | Helm release name |
| `helm_namespace` | `kata-system` | Kubernetes namespace for kata-deploy |
| `kubelet_config_path` | `/var/lib/kubelet/config.yaml` | Path to the Kubelet configuration file |
| `pod_cpu_limit` | `4` | CPU limit for the basic NUMA test pod |
| `pod_memory_limit` | `4Gi` | Memory limit for the basic NUMA test pod |
| `gpu_pod_cpu_limit` | `4` | CPU limit for the GPU test pod |
| `gpu_pod_memory_limit` | `8Gi` | Memory limit for the GPU test pod |
| `kata_deploy_chart_path` | `<repo>/tools/packaging/kata-deploy/helm-chart/kata-deploy` | Path to the local Helm chart |
| `pod_wait_timeout` | `300` | Seconds to wait for pods to reach Running |
| `daemonset_wait_timeout` | `600` | Seconds to wait for kata-deploy DaemonSet rollout |
| `skip_gpu_test` | `false` | Skip GPU-related phases |
| `cleanup_kata_deploy` | `false` | Uninstall kata-deploy Helm release during cleanup |

## Phases

The playbook runs through seven phases that map to the documentation:

| Phase | Doc section | What it does |
|---|---|---|
| 1 – Host Inspection | Step 1 | Runs `numactl`, discovers GPUs, detects single vs multi-NUMA |
| 2 – cpuManagerPolicy | Step 2 | Switches kubelet to `cpuManagerPolicy: static`, restarts if needed |
| 3 – Helm Deploy | Step 3 | Installs kata-deploy with NUMA drop-in via Helm |
| 4 – Basic NUMA Pod | Steps 4.1 + 5 | Deploys a pod and verifies guest NUMA topology |
| 5 – Host Verification | Step 6 | Checks vCPU pinning, shim logs, QEMU command line |
| 6 – GPU NUMA Pod | Steps 4.2 + 7 | Deploys a GPU pod and verifies VFIO NUMA placement |
| 7 – Cleanup | — | Deletes test pods, optionally uninstalls Helm release |

Each phase uses `block/rescue` to record pass/fail. A summary is printed at
the end of the run.

## Single-NUMA vs Multi-NUMA

The playbook auto-detects the host NUMA topology and adjusts its assertions:

| Check | Multi-NUMA (>= 2 nodes) | Single-NUMA (1 node) |
|---|---|---|
| Guest NUMA nodes | Must match host count | Must be exactly 1 |
| Guest CPU distribution | CPUs split across nodes | All CPUs on node 0 |
| Guest memory | Split across nodes | All on node 0 |
| QEMU `-numa` args | `policy=bind`, `host-nodes=`, `dist` | None (topology skipped) |
| VFIO placement log | `VFIO device NUMA placement validated` | Not applicable |

Running on a single-NUMA host ensures that enabling `enable_numa = true`
does not break anything -- the runtime detects one node and skips
multi-NUMA topology (the "harmless no-op" documented in the guide).

## Customising the Image

Override `kata_deploy_image` and `kata_deploy_tag` to test a different build:

```bash
ansible-playbook -i inventory.ini verify-numa.yml \
    -e kata_deploy_image=quay.io/kata-containers/kata-deploy \
    -e kata_deploy_tag=3.29.0 \
    -e gpu_runtime_class=kata-qemu-nvidia-gpu
```

## File Layout

```
tests/integration/numa/
├── README.md                             # This file
├── inventory.ini                         # Localhost inventory
├── verify-numa.yml                       # Main playbook
└── templates/
    ├── kata-deploy-numa-values.yaml.j2   # Helm values
    ├── numa-test-pod.yaml.j2             # Basic NUMA pod
    └── gpu-numa-test-pod.yaml.j2         # GPU NUMA pod
```
