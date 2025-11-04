#!/usr/bin/env bash
#
# Copyright 2019 HyperHQ Inc.
#
# SPDX-License-Identifier: Apache-2.0
#

set -o errexit -o pipefail -o nounset

# Define the root directory for all proto files
BASEDIR="$(dirname "$0")"
cd ${BASEDIR}/..
BASEPATH=`pwd`

proto_files_list=(protocols/cdiresolver/cdiresolver.proto protocols/cache/cache.proto)

for f in "${proto_files_list[@]}"; do
	echo -e "\n   [golang] compiling ${f} ..."
	PROTOPATH=$(dirname ${f})
	protoc \
		-I="${PROTOPATH}":"${BASEPATH}/vendor" \
		--go_out=paths=source_relative:${PROTOPATH} \
		--go-grpc_out=paths=source_relative:${PROTOPATH} \
		${f}
	echo -e "   [golang] ${f} compiled\n"
done
