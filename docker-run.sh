#!/usr/bin/env bash
set -euo pipefail

docker run --rm \
    --cap-add CAP_BPF \
    --cap-add CAP_PERFMON \
    --cap-add CAP_SYS_ADMIN \
    --cap-add CAP_NET_ADMIN \
    -v /sys/kernel/debug:/sys/kernel/debug:ro \
    -v /sys/fs/bpf:/sys/fs/bpf \
    scaphandre "$@"
