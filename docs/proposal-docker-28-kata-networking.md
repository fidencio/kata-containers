# Proposal: Fix Kata Containers networking with Docker 28+

This document is a proposal for the Docker (Moby) community to restore working container networking when using the Kata Containers runtime (`io.containerd.run.kata.v2`) with Docker 28 and later. It explains the problem, how to reproduce it, how things worked before the breakage, and what changes are needed on both the Docker and Kata sides.

---

## 1. Problem summary

With **Docker 28.0 and later** (28.x, 29.x), containers started with the Kata runtime have **no working network**: inside the container only the loopback interface is present; there is no `eth0` and no IP address. As a result, the container cannot reach the internet or other containers.

- **Affected:** `docker run --runtime io.containerd.run.kata.v2 ...` (and equivalent API usage).
- **Not affected:** The default runc runtime; Kata when used with containerd directly (e.g. nerdctl, ctr) or with Kubernetes.
- **When it broke:** Docker 28+ (Docker switched to using containerd for container execution in this line of releases).

References: [kata-containers/kata-containers#9340](https://github.com/kata-containers/kata-containers/issues/9340), [moby/moby#47626](https://github.com/moby/moby/issues/47626).

---

## 2. Reproducer

**Prerequisites**

- Linux host.
- Docker 28 or 29 installed and using the default containerd-based execution.
- Kata Containers runtime installed and configured (e.g. `io.containerd.run.kata.v2` in `daemon.json`).

**Steps**

```bash
# Run a container with the Kata runtime
sudo docker run --runtime io.containerd.run.kata.v2 -it --rm alpine sh

# Inside the container, check interfaces
/ # ip addr
```

**Expected (before Docker 28 / with fix)**

- At least `lo` and `eth0`.
- `eth0` has an IPv4 address (e.g. `inet 172.17.0.2/16`).
- `apk update` and network access work.

**Actual (Docker 28+ without fix)**

- Only `lo` (loopback).
- No `eth0`, no IP.
- `apk update` fails (no network).

```text
1: lo: <LOOPBACK,UP,LOWER_UP> ...
    inet 127.0.0.1/8 scope host lo
    inet6 ::1/128 ...
/ # apk update
fetch https://dl-cdn.alpinelinux.org/...
WARNING: ... temporary error (try again later)
```

### Testing with qemu-runtime-rs (Rust runtime)

To run the **same local test** with the Kata Rust runtime (`qemu-runtime-rs`) instead of the Go shim:

1. **Ensure the runtime-rs shim is available under the name containerd expects**  
   Containerd looks for a binary named `containerd-shim-kata-qemu-runtime-rs-v2` for the runtime type `io.containerd.kata-qemu-runtime-rs.v2`. Create a symlink if your install only has the generic runtime-rs shim:

   ```bash
   sudo ln -sf /opt/kata/runtime-rs/bin/containerd-shim-kata-v2 \
       /usr/local/bin/containerd-shim-kata-qemu-runtime-rs-v2
   ```

2. **Use the qemu-runtime-rs configuration**  
   Point the runtime-rs config dir at the QEMU runtime-rs config:

   ```bash
   sudo ln -sf /opt/kata/share/defaults/kata-containers/runtime-rs/configuration-qemu-runtime-rs.toml \
       /opt/kata/share/defaults/kata-containers/runtime-rs/configuration.toml
   ```

3. **Register the runtime in Docker**  
   Add the runtime to `/etc/docker/daemon.json` (merge into existing `"runtimes"` if present):

   ```json
   "runtimes": {
     "io.containerd.run.kata.v2": { "path": "/opt/kata/bin/containerd-shim-kata-v2" },
     "io.containerd.kata-qemu-runtime-rs.v2": { "path": "/usr/local/bin/containerd-shim-kata-qemu-runtime-rs-v2" }
   }
   ```

   Then restart Docker: `sudo systemctl restart docker`.

4. **Run the same test as with the Go shim**  
   Use the runtime name that contains `"kata"` so the Docker-side fix applies:

   ```bash
   sudo docker run --runtime io.containerd.kata-qemu-runtime-rs.v2 -it --rm alpine sh
   ```

   Inside the container:

   ```bash
   / # ip addr
   / # apk update
   ```

   With the fix (Docker + Kata): you should see `eth0` with an IPv4 address and working network, same as with `io.containerd.run.kata.v2`.

---

## 3. How it worked before (Docker &lt; 28)

Previously, Docker used the **built-in runtime** (runc) for execution. The flow was:

1. Docker created a libnetwork **sandbox** and its **network namespace** (e.g. under `/var/run/docker/netns/`).
2. When starting the container, Docker ran the **container process** (runc’s child) **inside that netns**.
3. After the process was running, Docker called **SetKey** with `/proc/<container-pid>/ns/net` (the container’s netns) and then **allocateNetwork**, which created the veth pair and attached one end to that netns.
4. So the veth lived in the same netns as the container process, and the container saw `eth0` with an IP.

For runc, the **task PID** that Docker uses after “create task” is the **container process** itself, so `/proc/<task-pid>/ns/net` is the container’s netns. That matches the netns libnetwork is supposed to use.

---

## 4. Why it breaks with Docker 28+

With Docker 28+, Docker uses **containerd** to run the container. The “task” is the **containerd shim** (e.g. `containerd-shim-kata-v2`), not the process inside the Kata VM. The shim runs on the **host** in the **host** network namespace.

The current flow is:

1. Docker creates the libnetwork sandbox and its netns path (e.g. `/var/run/docker/netns/<id>`).
2. The OCI spec is built with **no** network namespace path (or “runtime creates netns”).
3. Docker creates the **task** in containerd (the Kata shim starts).
4. **After** the task exists, Docker calls **SetKey** with `/proc/<task-pid>/ns/net`. That is the **shim’s** netns, i.e. the **host** netns.
5. Docker then calls **allocateNetwork**, which attaches the veth to that netns.
6. So the veth is placed in the **host** netns, not in the netns Kata uses for the VM/container.

Kata either creates its own netns or uses one it manages; that netns never receives the veth. The netns path Docker later uses (after SetKey) is the host netns. So from inside the Kata container, only loopback is visible.

**Root cause:** Docker assumes the **task PID** owns the container’s network namespace. That is true for runc (task = container process) but false for Kata (task = shim, which is on the host). Using the task’s netns for SetKey and allocateNetwork puts the veth in the wrong place for Kata.

---

## 5. What is needed to make it work again

Two sides need to align:

1. **Docker (Moby):** For runtimes like Kata (where the task is not the container process), do **not** use the task’s netns. Instead, keep using the **libnetwork sandbox** netns, pass that path to the runtime in a way that does not force the runtime process into that netns (e.g. via an annotation), and call **allocateNetwork before** creating the task so the veth is in that netns before the runtime starts.
2. **Kata:** Use the netns path provided by Docker (e.g. from the annotation) as the network namespace for the sandbox so that the veth Docker creates there is visible inside the VM.

Details follow.

---

### 5.1 Docker (Moby) side

**Goal:** For “external” runtimes (e.g. Kata), use the **libnetwork-created** netns path and allocate the network **before** the task is created; do not overwrite the sandbox key with the task’s netns after the task exists.

**Concrete changes (summary):**

1. **Create the osl sandbox (and its netns path) for all containers**  
   Today, when `useExternalKey` is true, the code may skip creating the osl sandbox, so the netns path does not exist when needed. Create it when `sb.osSbox == nil` so the path (e.g. `/var/run/docker/netns/<id>`) exists for Kata.

2. **Do not destroy that path when replacing the sandbox in SetKey**  
   When SetKey is later called (for runc) with the container’s netns, the code replaces the pre-created sandbox. It must **not** unmount/remove the netns path (e.g. use a “close handle only” path instead of full release/destroy), so that the same path can still be used for Kata and so runc’s flow still works.

3. **For runtimes that need the sandbox netns (e.g. Kata), before creating the task:**
   - Resolve the sandbox key (netns path).
   - If the path is openable (e.g. `netns.GetFromPath`), set an **annotation** (e.g. `com.docker.network.namespace.path`) to that path so the runtime can use it **without** the launcher putting the runtime process into that netns.
   - Call **allocateNetwork** so the veth is created in that netns **before** the task (and thus the Kata shim) starts.

4. **In initializeCreatedTask (after the task exists):**  
   If the spec already has that annotation set, **skip** SetKey and allocateNetwork for network (already done before the task). For runc (no annotation), keep current behavior: SetKey with `/proc/<task-pid>/ns/net` then allocateNetwork.

5. **Bridge driver / endpoint options:**  
   When the runtime name indicates Kata (or similar) and the sandbox netns path is not yet set on the sandbox object, pass the sandbox **key** (path) to the bridge driver so it can create the veth in that netns (e.g. `WithNetnsPath(sb.Key())` in the appropriate create-endpoint options).

**Why annotation instead of OCI spec network path:**  
If the netns path is set in the OCI spec’s `Linux.Namespaces` for the network, some launchers may put the **runtime process** (the Kata shim) into that netns. The shim needs to run in the host netns (e.g. to create the VM and devices). Passing the path only via an annotation avoids that; Kata reads the annotation and uses that netns for the sandbox’s network, without the process being moved there.

**Files touched (for reference):**  
`daemon/libnetwork/controller_linux.go`, `daemon/libnetwork/sandbox_linux.go`, `daemon/libnetwork/osl/namespace_linux.go` (and no-op for other OS), `daemon/network.go`, `daemon/start.go`, `daemon/start_linux.go`.

---

### 5.2 Kata Containers side

**Goal:** When Docker passes the libnetwork netns path (e.g. via `com.docker.network.namespace.path`), Kata should use that path as the sandbox network namespace so the veth Docker creates there is visible inside the VM.

**Concrete changes (summary):**

1. **Annotation:**  
   Define and respect the same key as Docker (e.g. `com.docker.network.namespace.path`). Read it from the OCI spec annotations when building the network config and set `NetworkConfig.NetworkID` to that path so the sandbox uses that netns.

2. **Fallback:**  
   If the path is not set or not accessible (e.g. not visible in the shim’s mount namespace), fall back to existing behavior (e.g. create a new netns or use spec’s namespace path) so the container can still start.

3. **Detection of Docker containers:**  
   When Docker uses containerd, it may not inject the classic libnetwork prestart hook. Kata should also detect Docker by the presence of annotations in the `com.docker.*` namespace so it can apply Docker-specific logic (e.g. retries when no endpoints are found yet).

4. **Robustness:**  
   If the netns from the annotation cannot be opened (e.g. different mount namespace), do not fail sandbox creation; log a warning and continue with no network endpoints. Optionally retry “add endpoints” a few times with short delays so that if Docker adds the veth shortly after the task starts, Kata can pick it up.

**Files touched (for reference):**  
`src/runtime/pkg/oci/utils.go`, `src/runtime/virtcontainers/pkg/annotations/docker/annotations.go`, `src/runtime/virtcontainers/network_linux.go`, `src/runtime/virtcontainers/sandbox.go`, `src/runtime/virtcontainers/utils/utils.go` (e.g. `IsDockerContainer`).

The same annotation (`com.docker.network.namespace.path`) is supported by the **Rust runtime (runtime-rs)** so that `io.containerd.kata-qemu-runtime-rs.v2` (and other runtime-rs variants) get working networking with Docker 28+. Implementation: the annotation key is defined in kata-types’ Docker-related annotations (`src/libs/kata-types/src/annotations/docker.rs`); `src/runtime-rs/crates/runtimes/src/manager.rs` reads the annotation and uses that path as the sandbox network namespace when present and accessible (with fallback to existing behavior otherwise).

---

## 6. Summary table

| Aspect | Before Docker 28 (runc) | Docker 28+ with runc | Docker 28+ with Kata (broken) | Docker 28+ with Kata (fixed) |
|--------|-------------------------|------------------------|--------------------------------|------------------------------|
| Task = container process? | Yes | Yes | No (task = shim) | No |
| SetKey uses | Container netns | Container netns | Host netns (wrong) | Not used for Kata |
| allocateNetwork | After task, veth in container netns | Same | After task, veth in host netns | Before task, veth in sandbox netns |
| Netns path to runtime | N/A (runc in netns) | N/A | N/A | Annotation |
| Kata uses | N/A | N/A | Own netns (no veth) | Sandbox netns (veth there) |

---

## 7. Alternative: Kata-only network monitor (PR 11749)

An alternative approach exists that **does not require any Docker/Moby changes**: a **network monitor** inside the Kata runtime that watches for new interfaces in the netns where Docker actually puts the veth (the host netns) and hotplugs them into the VM.

**How it works (e.g. [PR #11749](https://github.com/kata-containers/kata-containers/pull/11749)):**

1. When the container bundle path contains `"moby"` (Docker), the Kata shim starts a **netmon** goroutine.
2. The netmon enters the **hypervisor process’s** network namespace (same as host when Docker is broken) via `GetHypervisorPid()` and `setns`.
3. It subscribes to netlink link and route events in that netns.
4. When Docker adds the veth (after task create), the netmon receives **RTM_NEWLINK** and, when the interface is UP and RUNNING, calls **AddInterface** to hotplug it into the VM.
5. On **RTM_DELLINK** (e.g. `docker network disconnect`), it calls **RemoveInterface** and updates routes. The PR also reorders **HotDetach** (hypervisor remove before netns cleanup) and uses the last endpoint’s PCI path for the device path when multiple interfaces exist.

**Pros:**

- No changes to Docker/Moby; works with stock Docker 28+.
- Supports connect/disconnect (hotplug and hot-unplug of interfaces).
- Single place to maintain (Kata only).

**Cons:**

- **Which veth is ours?** With multiple Kata containers, all veths end up in the host netns. The netmon uses heuristics (e.g. ignore interfaces whose name contains `"kata"`, add first new UP+RUNNING). In races or with many containers, the wrong veth could be attached.
- **Tied to current “wrong” behavior:** Relies on Docker putting the veth in the same netns as the hypervisor (host). If Docker is fixed to put the veth in a different netns, the netmon would need to run in that netns (e.g. using an annotation-provided path) and would then overlap with the annotation-based design.
- **Fragile:** Detection via `bundle` containing `"moby"`; netlink event ordering and interface naming are not a contract.
- **Hypervisor support:** The PR warns that non-QEMU hypervisors may not support network device hot-unplug and could crash on disconnect.

**Relation to the annotation-based fix:**

- The **annotation-based fix** (Docker passes sandbox netns path, allocates network before task, Kata uses that path) is the **correct fix** from a contract and correctness standpoint: the right netns is used from the start, no heuristics, no “which veth” ambiguity.
- The **netmon approach** is a **Kata-only workaround** that can restore networking with unpatched Docker 28+ at the cost of the above limitations. It can coexist with the annotation-based fix (e.g. netmon only when annotation is not set, or netmon for late-attach scenarios).

---

## 8. Feedback requested

We would welcome feedback from the Docker/Moby community on:

1. **Approach:** Using an annotation for the netns path (instead of the OCI spec’s network namespace path) to avoid the runtime process being moved into that netns.
2. **Runtime detection:** Using runtime name (e.g. containing `"kata"`) to enable this path; or whether a more explicit contract (e.g. capability or label) is preferred.
3. **Backward compatibility:** Ensuring the default (runc) and other runtimes are unchanged and that the osl sandbox creation/SetKey changes do not regress existing behavior.
4. **Naming and location:** The exact annotation key (`com.docker.network.namespace.path`) and where it is set in the daemon.

We have implemented and tested the described changes on both Moby and Kata and can provide patches or point to branches/commits for review.

---

## 9. References

- [kata-containers/kata-containers#9340](https://github.com/kata-containers/kata-containers/issues/9340) – Kata: no eth0 with Docker (breakage observed from Docker 28+).
- [moby/moby#47626](https://github.com/moby/moby/issues/47626) – Docker 26 breaks Kata networking.
- Internal design note: `docs/design/docker-26-kata-networking-investigation.md` (root cause and annotation-based fix).
