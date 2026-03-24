// Copyright (c) 2025 NVIDIA CORPORATION
//
// SPDX-License-Identifier: Apache-2.0
//

//! Minimal kubelet PodResources gRPC client for CDI cold-plug.
//!
//! Only the [`get_pod_resources`] RPC of the `PodResourcesLister` service is
//! implemented — that is all we need to discover the CDI devices allocated
//! to a sandbox at creation time.
//!
//! Message types are defined manually using `prost` derive macros so that no
//! `.proto` file or `build.rs` code-generation step is required. Field
//! numbers are taken verbatim from the upstream proto:
//! `k8s.io/kubelet/pkg/apis/podresources/v1/api.proto`

use anyhow::{anyhow, Context, Result};
use tonic::transport::{Endpoint, Uri};

// gRPC service/method path for the Get RPC.
const GET_RPC_PATH: &str = "/v1.PodResourcesLister/Get";

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

#[derive(Clone, prost::Message)]
pub struct GetPodResourcesRequest {
    #[prost(string, tag = "1")]
    pub pod_name: String,
    #[prost(string, tag = "2")]
    pub pod_namespace: String,
}

#[derive(Clone, prost::Message)]
pub struct GetPodResourcesResponse {
    #[prost(message, optional, tag = "1")]
    pub pod_resources: Option<PodResources>,
}

#[derive(Clone, prost::Message)]
pub struct PodResources {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub namespace: String,
    #[prost(message, repeated, tag = "3")]
    pub containers: Vec<ContainerResources>,
}

#[derive(Clone, prost::Message)]
pub struct ContainerResources {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(message, repeated, tag = "2")]
    pub devices: Vec<ContainerDevices>,
}

#[derive(Clone, prost::Message)]
pub struct ContainerDevices {
    #[prost(string, tag = "1")]
    pub resource_name: String,
    #[prost(string, repeated, tag = "2")]
    pub device_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Query the kubelet PodResources API and return CDI device IDs for the given
/// pod formatted as `"<resource_name>=<device_id>"` strings expected by the
/// CDI injection layer.
pub async fn get_cdi_devices(
    socket_path: &str,
    pod_name: &str,
    pod_namespace: &str,
) -> Result<Vec<String>> {
    // tonic requires a dummy http URL; actual transport goes through the UDS connector.
    let path = socket_path.to_owned();
    let channel = Endpoint::try_from("http://[::]:0")
        .context("build tonic endpoint")?
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            tokio::net::UnixStream::connect(path.clone())
        }))
        .await
        .with_context(|| format!("connect to kubelet socket {socket_path}"))?;

    let mut client = tonic::client::Grpc::new(channel);

    let request = tonic::Request::new(GetPodResourcesRequest {
        pod_name: pod_name.to_owned(),
        pod_namespace: pod_namespace.to_owned(),
    });

    client.ready().await.context("kubelet gRPC channel not ready")?;

    let codec = tonic::codec::ProstCodec::<GetPodResourcesRequest, GetPodResourcesResponse>::default();
    let path = http::uri::PathAndQuery::from_static(GET_RPC_PATH);
    let response = client
        .unary(request, path, codec)
        .await
        .context("PodResourcesLister/Get RPC")?;

    let pod_resources = response
        .into_inner()
        .pod_resources
        .ok_or_else(|| anyhow!("kubelet returned empty PodResources for pod {pod_name}/{pod_namespace}"))?;

    let mut cdi_devices = Vec::new();
    for container in &pod_resources.containers {
        for device in &container.devices {
            for id in &device.device_ids {
                cdi_devices.push(format!("{}={}", device.resource_name, id));
            }
        }
    }

    Ok(cdi_devices)
}
