# Copyright (c) 2026 NVIDIA Corporation
# SPDX-License-Identifier: Apache-2.0

ARG GO_TOOLCHAIN
FROM golang:${GO_TOOLCHAIN}

RUN apt-get update && apt-get install -y --no-install-recommends \
    binutils \
    cryptsetup-bin \
    erofs-utils \
    jq \
    parted \
    qemu-utils \
    util-linux && \
    rm -rf /var/lib/apt/lists/*
