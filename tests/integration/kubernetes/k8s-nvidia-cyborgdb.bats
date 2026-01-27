#!/usr/bin/env bats
#
# Copyright (c) 2026 NVIDIA Corporation
#
# SPDX-License-Identifier: Apache-2.0
#

load "${BATS_TEST_DIRNAME}/lib.sh"
load "${BATS_TEST_DIRNAME}/confidential_common.sh"

export KATA_HYPERVISOR="${KATA_HYPERVISOR:-qemu-nvidia-gpu}"

POD_NAME_CYBORGDB="nvidia-cyborgdb"
export POD_NAME_CYBORGDB

POD_READY_TIMEOUT_CYBORGDB=${POD_READY_TIMEOUT_CYBORGDB:-300s}
export POD_READY_TIMEOUT_CYBORGDB

# Base64 encoding for use as Kubernetes Secret in pod manifests
CYBORGDB_API_KEY_BASE64=$(
    echo -n "${CYBORGDB_API_KEY}" | base64 -w0
)
export CYBORGDB_API_KEY_BASE64

create_cyborgdb_pod() {
    envsubst <"${POD_CYBORGDB_YAML_IN}" >"${POD_CYBORGDB_YAML}"

    auto_generate_policy "${policy_settings_dir}" "${POD_CYBORGDB_YAML}"

    kubectl apply -f "${POD_CYBORGDB_YAML}"
    kubectl wait --for=condition=Ready --timeout="${POD_READY_TIMEOUT_CYBORGDB}" pod "${POD_NAME_CYBORGDB}"

    kubectl get pod "${POD_NAME_CYBORGDB}" -o jsonpath='{.status.podIP}'
    POD_IP_CYBORGDB=$(kubectl get pod "${POD_NAME_CYBORGDB}" -o jsonpath='{.status.podIP}')
    [[ -n "${POD_IP_CYBORGDB}" ]]

    # Generate a 32-byte encryption key for the index (hex encoded for API)
    TEST_INDEX_KEY=$(python3 -c "import secrets; print(secrets.token_bytes(32).hex())")

    # Use a well-defined index name for the test
    TEST_INDEX_NAME="kata-containers-test-index"

    echo "POD_IP_CYBORGDB=${POD_IP_CYBORGDB}" >"${BATS_SUITE_TMPDIR}/env"
    echo "TEST_INDEX_KEY=${TEST_INDEX_KEY}" >>"${BATS_SUITE_TMPDIR}/env"
    echo "TEST_INDEX_NAME=${TEST_INDEX_NAME}" >>"${BATS_SUITE_TMPDIR}/env"
    echo "# POD_IP_CYBORGDB=${POD_IP_CYBORGDB}" >&3
}

setup_file() {
    setup_common || die "setup_common failed"

    # Ensure CYBORGDB_API_KEY is set
    [[ -n "${CYBORGDB_API_KEY:-}" ]] || die "CYBORGDB_API_KEY environment variable must be set"

    export POD_CYBORGDB_YAML_IN="${pod_config_dir}/${POD_NAME_CYBORGDB}.yaml.in"
    export POD_CYBORGDB_YAML="${pod_config_dir}/${POD_NAME_CYBORGDB}.yaml"

    dpkg -s jq >/dev/null 2>&1 || sudo apt -y install jq

    policy_settings_dir="$(create_tmp_policy_settings_dir "${pod_config_dir}")"
    add_requests_to_policy_settings "${policy_settings_dir}" "ReadStreamRequest"

    create_cyborgdb_pod
}

@test "CyborgDB health check" {
    # shellcheck disable=SC1091
    source "${BATS_SUITE_TMPDIR}/env"
    [[ -n "${POD_IP_CYBORGDB}" ]]

    run curl -sX GET "http://${POD_IP_CYBORGDB}:8000/v1/health"
    [[ "${status}" -eq 0 ]]

    echo "${output}" | jq -e '.status == "healthy"'

    echo "# CyborgDB Health Response: ${output}" >&3
}

@test "CyborgDB create encrypted index" {
    # shellcheck disable=SC1091
    source "${BATS_SUITE_TMPDIR}/env"
    [[ -n "${POD_IP_CYBORGDB}" ]]
    [[ -n "${TEST_INDEX_KEY}" ]]
    [[ -n "${TEST_INDEX_NAME}" ]]

    run curl -sX POST "http://${POD_IP_CYBORGDB}:8000/v1/indexes/create" \
        -H "Content-Type: application/json" \
        -H "X-API-Key: ${CYBORGDB_API_KEY}" \
        -d "{
            \"index_name\": \"${TEST_INDEX_NAME}\",
            \"index_key\": \"${TEST_INDEX_KEY}\",
            \"index_config\": {\"type\": \"ivfflat\", \"dimension\": 128}
        }"

    echo "# Create Index Response: ${output}" >&3

    [[ "${status}" -eq 0 ]]
    echo "${output}" | jq -e '.status == "success"'
}

@test "CyborgDB upsert vectors to index" {
    # shellcheck disable=SC1091
    source "${BATS_SUITE_TMPDIR}/env"
    [[ -n "${POD_IP_CYBORGDB}" ]]
    [[ -n "${TEST_INDEX_KEY}" ]]
    [[ -n "${TEST_INDEX_NAME}" ]]

    # Build the upsert payload using Python for proper JSON formatting
    UPSERT_PAYLOAD=$(python3 -c "
import json
import random
payload = {
    'index_name': '${TEST_INDEX_NAME}',
    'index_key': '${TEST_INDEX_KEY}',
    'items': [
        {'id': 'kata-1', 'vector': [random.random() for _ in range(128)], 'metadata': {'type': 'container'}},
        {'id': 'kata-2', 'vector': [random.random() for _ in range(128)], 'metadata': {'type': 'hypervisor'}},
        {'id': 'kata-3', 'vector': [random.random() for _ in range(128)], 'metadata': {'type': 'gpu'}}
    ]
}
print(json.dumps(payload))
")

    run curl -sX POST "http://${POD_IP_CYBORGDB}:8000/v1/vectors/upsert" \
        -H "Content-Type: application/json" \
        -H "X-API-Key: ${CYBORGDB_API_KEY}" \
        -d "${UPSERT_PAYLOAD}"

    echo "# Upsert Response: ${output}" >&3

    [[ "${status}" -eq 0 ]]
    echo "${output}" | jq -e '.status == "success"'
}

@test "CyborgDB query encrypted index" {
    # shellcheck disable=SC1091
    source "${BATS_SUITE_TMPDIR}/env"
    [[ -n "${POD_IP_CYBORGDB}" ]]
    [[ -n "${TEST_INDEX_KEY}" ]]
    [[ -n "${TEST_INDEX_NAME}" ]]

    # Build the query payload using Python and save to temp file
    python3 -c "
import json
import random
payload = {
    'index_name': '${TEST_INDEX_NAME}',
    'index_key': '${TEST_INDEX_KEY}',
    'query_vectors': [random.random() for _ in range(128)],
    'top_k': 3
}
with open('${BATS_SUITE_TMPDIR}/query_payload.json', 'w') as f:
    json.dump(payload, f)
"

    run curl -sX POST "http://${POD_IP_CYBORGDB}:8000/v1/vectors/query" \
        -H "Content-Type: application/json" \
        -H "X-API-Key: ${CYBORGDB_API_KEY}" \
        -d @"${BATS_SUITE_TMPDIR}/query_payload.json"

    echo "# Query Response: ${output}" >&3

    [[ "${status}" -eq 0 ]]
    echo "${output}" | jq -e '.results | type == "array"'
}

@test "CyborgDB list indexes" {
    # shellcheck disable=SC1091
    source "${BATS_SUITE_TMPDIR}/env"
    [[ -n "${POD_IP_CYBORGDB}" ]]
    [[ -n "${TEST_INDEX_NAME}" ]]

    run curl -sX GET "http://${POD_IP_CYBORGDB}:8000/v1/indexes/list" \
        -H "X-API-Key: ${CYBORGDB_API_KEY}"

    echo "# List Indexes Response: ${output}" >&3

    [[ "${status}" -eq 0 ]]
    echo "${output}" | jq -e ".indexes | index(\"${TEST_INDEX_NAME}\")"
}

@test "CyborgDB delete index" {
    # shellcheck disable=SC1091
    source "${BATS_SUITE_TMPDIR}/env"
    [[ -n "${POD_IP_CYBORGDB}" ]]
    [[ -n "${TEST_INDEX_KEY}" ]]
    [[ -n "${TEST_INDEX_NAME}" ]]

    run curl -sX POST "http://${POD_IP_CYBORGDB}:8000/v1/indexes/delete" \
        -H "Content-Type: application/json" \
        -H "X-API-Key: ${CYBORGDB_API_KEY}" \
        -d "{
            \"index_name\": \"${TEST_INDEX_NAME}\",
            \"index_key\": \"${TEST_INDEX_KEY}\"
        }"

    echo "# Delete Index Response: ${output}" >&3

    [[ "${status}" -eq 0 ]]
    echo "${output}" | jq -e '.status == "success"'
}

teardown_file() {
    echo "=== CyborgDB Pod Logs ===" >&3
    kubectl logs "${POD_NAME_CYBORGDB}" >&3 || true

    delete_tmp_policy_settings_dir "${policy_settings_dir}"
    kubectl describe pods >&3

    [[ -f "${POD_CYBORGDB_YAML}" ]] && kubectl delete -f "${POD_CYBORGDB_YAML}" --ignore-not-found=true

    print_node_journal_since_test_start "${node}" "${node_start_time:-}" "${BATS_TEST_COMPLETED:-}" >&3
}
