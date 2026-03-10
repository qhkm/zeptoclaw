#!/bin/bash
set -euo pipefail

source "$(dirname "$0")/container-base.sh"

container_run "cargo bench --bench message_bus --no-run"