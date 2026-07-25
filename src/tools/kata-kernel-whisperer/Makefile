# Copyright (c) Kata Containers Contributors
#
# SPDX-License-Identifier: Apache-2.0

include ../../../utils.mk

PROJECT_COMPONENT = kata-kernel-whisperer
TARGET = $(PROJECT_COMPONENT)

# Produce a fully static binary when targeting musl (x86_64 and aarch64).
ifeq ($(LIBC),musl)
EXTRA_RUSTFLAGS := -C link-self-contained=yes
endif

.DEFAULT_GOAL := default

default: build

build:
	@RUSTFLAGS="$(EXTRA_RUSTFLAGS) --deny warnings" cargo build -p $(PROJECT_COMPONENT) --target $(TRIPLE) $(if $(findstring release,$(BUILD_TYPE)),--release)

test:
	@RUSTFLAGS="$(EXTRA_RUSTFLAGS) --deny warnings" cargo test -p $(PROJECT_COMPONENT) --target $(TRIPLE) $(if $(findstring release,$(BUILD_TYPE)),--release)

check: standard_rust_check

clean:
	@cargo clean

install:
	@RUSTFLAGS="$(EXTRA_RUSTFLAGS) --deny warnings" cargo install --locked --target $(TRIPLE) --path .

.PHONY: build check clean default install test
