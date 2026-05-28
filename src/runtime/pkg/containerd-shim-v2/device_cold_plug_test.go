// Copyright (c) 2026
//
// SPDX-License-Identifier: Apache-2.0
//

package containerdshim

import (
	"reflect"
	"testing"

	podresourcesv1 "k8s.io/kubelet/pkg/apis/podresources/v1"
)

func TestExtractCDIDevicesLegacyOnly(t *testing.T) {
	podRes := &podresourcesv1.PodResources{
		Containers: []*podresourcesv1.ContainerResources{
			{
				Name: "ctr0",
				Devices: []*podresourcesv1.ContainerDevices{
					{
						ResourceName: "nvidia.com/pgpu",
						DeviceIds:    []string{"vfio0", "vfio1"},
					},
				},
			},
		},
	}

	got := extractCDIDevices(podRes)
	want := []string{"nvidia.com/pgpu=vfio0", "nvidia.com/pgpu=vfio1"}

	if !reflect.DeepEqual(got, want) {
		t.Fatalf("unexpected devices\n got: %#v\nwant: %#v", got, want)
	}
}

func TestExtractCDIDevicesDRAOnly(t *testing.T) {
	podRes := &podresourcesv1.PodResources{
		Containers: []*podresourcesv1.ContainerResources{
			{
				Name: "ctr0",
				DynamicResources: []*podresourcesv1.DynamicResource{
					{
						ClaimResources: []*podresourcesv1.ClaimResource{
							{
								CDIDevices: []*podresourcesv1.CDIDevice{
									{Name: "nvidia.com/gpu=GPU-0"},
									{Name: "nvidia.com/gpu=GPU-1"},
								},
							},
						},
					},
				},
			},
		},
	}

	got := extractCDIDevices(podRes)
	want := []string{"nvidia.com/gpu=GPU-0", "nvidia.com/gpu=GPU-1"}

	if !reflect.DeepEqual(got, want) {
		t.Fatalf("unexpected devices\n got: %#v\nwant: %#v", got, want)
	}
}

func TestExtractCDIDevicesMixedAndDeduped(t *testing.T) {
	podRes := &podresourcesv1.PodResources{
		Containers: []*podresourcesv1.ContainerResources{
			{
				Name: "ctr0",
				Devices: []*podresourcesv1.ContainerDevices{
					{
						ResourceName: "nvidia.com/pgpu",
						DeviceIds:    []string{"vfio0"},
					},
				},
				DynamicResources: []*podresourcesv1.DynamicResource{
					{
						ClaimResources: []*podresourcesv1.ClaimResource{
							{
								CDIDevices: []*podresourcesv1.CDIDevice{
									{Name: "nvidia.com/gpu=GPU-0"},
									{Name: "nvidia.com/gpu=GPU-0"},
								},
							},
						},
					},
				},
			},
			{
				Name: "ctr1",
				DynamicResources: []*podresourcesv1.DynamicResource{
					{
						ClaimResources: []*podresourcesv1.ClaimResource{
							{
								CDIDevices: []*podresourcesv1.CDIDevice{
									{Name: "nvidia.com/gpu=GPU-1"},
									{Name: ""},
								},
							},
						},
					},
				},
			},
		},
	}

	got := extractCDIDevices(podRes)
	want := []string{
		"nvidia.com/pgpu=vfio0",
		"nvidia.com/gpu=GPU-0",
		"nvidia.com/gpu=GPU-1",
	}

	if !reflect.DeepEqual(got, want) {
		t.Fatalf("unexpected devices\n got: %#v\nwant: %#v", got, want)
	}
}
