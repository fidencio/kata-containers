// Copyright (c) 2026 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

pub mod v1;

use v1::pod_resources_lister_client::PodResourcesListerClient;

use std::collections::HashMap;
use std::convert::TryFrom;

use anyhow::{anyhow, Context, Result};
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tokio::time::{timeout, Duration};
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::pod_resources::v1::GetPodResourcesRequest;

// containerd CRI annotations
const SANDBOX_NAME_ANNOTATION: &str = "io.kubernetes.cri.sandbox-name";
const SANDBOX_NAMESPACE_ANNOTATION: &str = "io.kubernetes.cri.sandbox-namespace";

// CRI-O annotations (fallback)
const CRIO_NAME_ANNOTATION: &str = "io.kubernetes.cri-o.KubeName";
const CRIO_NAMESPACE_ANNOTATION: &str = "io.kubernetes.cri-o.Namespace";
pub const DEFAULT_POD_RESOURCES_PATH: &str = "/var/lib/kubelet/pod-resources";
pub const DEFAULT_POD_RESOURCES_TIMEOUT: Duration = Duration::from_secs(10);
pub const CDI_K8S_PREFIX: &str = "cdi.k8s.io/";
const MAX_RECV_MSG_SIZE: usize = 16 * 1024 * 1024; // 16MB

// Create a gRPC channel to the specified Unix socket
async fn create_grpc_channel(socket_path: &str) -> Result<Channel> {
    let socket_path = socket_path.trim_start_matches("unix://");
    let socket_path_owned = socket_path.to_string();

    // Create a gRPC endpoint with a timeout
    let endpoint = Endpoint::try_from("http://[::]:50051")
        .context("failed to create endpoint")?
        .timeout(DEFAULT_POD_RESOURCES_TIMEOUT);

    // Connect to the Unix socket using a custom connector
    let channel = endpoint
        .connect_with_connector(service_fn(move |_: Uri| {
            let socket_path = socket_path_owned.clone();
            async move {
                let stream = UnixStream::connect(&socket_path).await.map_err(|e| {
                    std::io::Error::new(
                        e.kind(),
                        format!("failed to connect to {}: {}", socket_path, e),
                    )
                })?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
        .await
        .context("failed to connect to unix socket")?;

    Ok(channel)
}

pub async fn get_pod_cdi_devices(
    socket: &str,
    annotations: &HashMap<String, String>,
) -> Result<Vec<String>> {
    let pod_name = annotations
        .get(SANDBOX_NAME_ANNOTATION)
        .or_else(|| annotations.get(CRIO_NAME_ANNOTATION))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "cold plug: missing annotation {} or {}",
                SANDBOX_NAME_ANNOTATION,
                CRIO_NAME_ANNOTATION
            )
        })?;

    let pod_namespace = annotations
        .get(SANDBOX_NAMESPACE_ANNOTATION)
        .or_else(|| annotations.get(CRIO_NAMESPACE_ANNOTATION))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "cold plug: missing annotation {} or {}",
                SANDBOX_NAMESPACE_ANNOTATION,
                CRIO_NAMESPACE_ANNOTATION
            )
        })?;

    // Create gRPC channel to kubelet pod-resources socket
    let channel = create_grpc_channel(socket)
        .await
        .context("cold plug: failed to connect to kubelet")?;

    // Create PodResourcesLister client
    let mut client = PodResourcesListerClient::new(channel)
        .max_decoding_message_size(MAX_RECV_MSG_SIZE)
        .max_encoding_message_size(MAX_RECV_MSG_SIZE);

    // Prepare and send GetPodResources request
    let request = tonic::Request::new(GetPodResourcesRequest {
        pod_name: pod_name.to_string(),
        pod_namespace: pod_namespace.to_string(),
    });

    // Await response with timeout
    let response = timeout(DEFAULT_POD_RESOURCES_TIMEOUT, client.get(request))
        .await
        .context("cold plug: GetPodResources timeout")?
        .context("cold plug: GetPodResources RPC failed")?;

    // Extract PodResources from response
    let pod_resources = response
        .into_inner()
        .pod_resources
        .ok_or_else(|| anyhow!("cold plug: PodResources is nil"))?;

    Ok(extract_cdi_devices(&pod_resources))
}

fn push_unique(
    devices: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
    dev: String,
) {
    if dev.is_empty() {
        return;
    }

    if seen.insert(dev.clone()) {
        devices.push(dev);
    }
}

fn extract_cdi_devices(pod_resources: &v1::PodResources) -> Vec<String> {
    let mut devices = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for container in &pod_resources.containers {
        for device in &container.devices {
            for id in &device.device_ids {
                push_unique(
                    &mut devices,
                    &mut seen,
                    format!("{}={}", device.resource_name, id),
                );
            }
        }

        for dynamic_resource in &container.dynamic_resources {
            for claim_resource in &dynamic_resource.claim_resources {
                for cdi_device in &claim_resource.cdi_devices {
                    push_unique(&mut devices, &mut seen, cdi_device.name.clone());
                }
            }
        }
    }

    devices
}

#[cfg(test)]
mod tests {
    use super::extract_cdi_devices;
    use crate::pod_resources::v1::{
        CdiDevice, ClaimResource, ContainerDevices, ContainerResources, DynamicResource,
        PodResources,
    };

    #[test]
    fn extract_cdi_devices_legacy_only() {
        let pod_resources = PodResources {
            name: "pod".to_string(),
            namespace: "default".to_string(),
            containers: vec![ContainerResources {
                name: "ctr0".to_string(),
                devices: vec![ContainerDevices {
                    resource_name: "nvidia.com/pgpu".to_string(),
                    device_ids: vec!["vfio0".to_string(), "vfio1".to_string()],
                    topology: None,
                }],
                cpu_ids: vec![],
                memory: vec![],
                dynamic_resources: vec![],
            }],
        };

        let got = extract_cdi_devices(&pod_resources);
        assert_eq!(
            got,
            vec![
                "nvidia.com/pgpu=vfio0".to_string(),
                "nvidia.com/pgpu=vfio1".to_string()
            ]
        );
    }

    #[test]
    fn extract_cdi_devices_dra_only() {
        let pod_resources = PodResources {
            name: "pod".to_string(),
            namespace: "default".to_string(),
            containers: vec![ContainerResources {
                name: "ctr0".to_string(),
                devices: vec![],
                cpu_ids: vec![],
                memory: vec![],
                dynamic_resources: vec![DynamicResource {
                    claim_name: "gpu-claim".to_string(),
                    claim_namespace: "default".to_string(),
                    claim_resources: vec![ClaimResource {
                        cdi_devices: vec![
                            CdiDevice {
                                name: "nvidia.com/gpu=GPU-0".to_string(),
                            },
                            CdiDevice {
                                name: "nvidia.com/gpu=GPU-1".to_string(),
                            },
                        ],
                        driver_name: "gpu.nvidia.com".to_string(),
                        pool_name: "worker-gpu".to_string(),
                        device_name: "GPU".to_string(),
                    }],
                }],
            }],
        };

        let got = extract_cdi_devices(&pod_resources);
        assert_eq!(
            got,
            vec![
                "nvidia.com/gpu=GPU-0".to_string(),
                "nvidia.com/gpu=GPU-1".to_string()
            ]
        );
    }

    #[test]
    fn extract_cdi_devices_mixed_and_deduped() {
        let pod_resources = PodResources {
            name: "pod".to_string(),
            namespace: "default".to_string(),
            containers: vec![
                ContainerResources {
                    name: "ctr0".to_string(),
                    devices: vec![ContainerDevices {
                        resource_name: "nvidia.com/pgpu".to_string(),
                        device_ids: vec!["vfio0".to_string()],
                        topology: None,
                    }],
                    cpu_ids: vec![],
                    memory: vec![],
                    dynamic_resources: vec![DynamicResource {
                        claim_name: "gpu-claim".to_string(),
                        claim_namespace: "default".to_string(),
                        claim_resources: vec![ClaimResource {
                            cdi_devices: vec![
                                CdiDevice {
                                    name: "nvidia.com/gpu=GPU-0".to_string(),
                                },
                                CdiDevice {
                                    name: "nvidia.com/gpu=GPU-0".to_string(),
                                },
                            ],
                            driver_name: "gpu.nvidia.com".to_string(),
                            pool_name: "worker-gpu".to_string(),
                            device_name: "GPU".to_string(),
                        }],
                    }],
                },
                ContainerResources {
                    name: "ctr1".to_string(),
                    devices: vec![],
                    cpu_ids: vec![],
                    memory: vec![],
                    dynamic_resources: vec![DynamicResource {
                        claim_name: "gpu-claim-2".to_string(),
                        claim_namespace: "default".to_string(),
                        claim_resources: vec![ClaimResource {
                            cdi_devices: vec![
                                CdiDevice {
                                    name: "nvidia.com/gpu=GPU-1".to_string(),
                                },
                                CdiDevice {
                                    name: "".to_string(),
                                },
                            ],
                            driver_name: "gpu.nvidia.com".to_string(),
                            pool_name: "worker-gpu".to_string(),
                            device_name: "GPU".to_string(),
                        }],
                    }],
                },
            ],
        };

        let got = extract_cdi_devices(&pod_resources);
        assert_eq!(
            got,
            vec![
                "nvidia.com/pgpu=vfio0".to_string(),
                "nvidia.com/gpu=GPU-0".to_string(),
                "nvidia.com/gpu=GPU-1".to_string()
            ]
        );
    }
}
