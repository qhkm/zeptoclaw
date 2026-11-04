# Multi-Tenant Deployment

Run multiple ZeptoClaw tenants on a single VPS using Docker. Each tenant gets complete isolation with zero application code changes.

## Why Container-Per-Tenant

ZeptoClaw is ~4MB binary, ~6MB RSS idle. Docker provides process, filesystem, and network isolation out of the box. No multi-tenant code needed.

| Tenants | RAM Required | VPS Cost |
|---------|-------------|----------|
| 10 | ~60MB | Any VPS |
| 100 | ~600MB | $5/mo |
| 500 | ~3GB | $12/mo |
| 1000 | ~6GB | $24/mo |

Each tenant is fully isolated. If one tenant prompts the agent to destroy files, only their container is affected. Restart and they're back.

## Quick Start

```bash
# 1. Build the image
docker build -t zeptoclaw:latest .

# 2. Add tenants
./scripts/add-tenant.sh shop-ahmad "123:ABC..." "sk-ant-..."
./scripts/add-tenant.sh shop-fatimah "456:DEF..." "sk-ant-..."

# 3. Edit docker-compose.multi-tenant.yml with tenant blocks (see template)

# 4. Start everything
docker compose -f docker-compose.multi-tenant.yml up -d

# 5. Check status
docker compose -f docker-compose.multi-tenant.yml ps
docker logs zc-shop-ahmad
```

## File Structure

```
zeptoclaw/
├── docker-compose.multi-tenant.yml   # Compose file with all tenants
├── scripts/
│   └── add-tenant.sh                 # Script to create tenant config
└── tenants/
    ├── shop-ahmad/
    │   └── config.json               # Ahmad's config (API keys, bot token)
    ├── shop-fatimah/
    │   └── config.json
    └── shop-ali/
        └── config.json
```

Each tenant's persistent data lives in a Docker named volume (`shop-ahmad-data`, etc.), separate from config.

## Adding a Tenant

```bash
# Create config
./scripts/add-tenant.sh <name> <telegram-bot-token> <anthropic-api-key>

# Edit config to add WhatsApp, Google Sheets, Brave search, etc.
nano tenants/<name>/config.json

# Add the service block to docker-compose.multi-tenant.yml (script prints the template)

# Deploy
docker compose -f docker-compose.multi-tenant.yml up -d
```

## Removing a Tenant

```bash
# Stop the tenant
docker compose -f docker-compose.multi-tenant.yml stop tenant-<name>

# Remove their service block from docker-compose.multi-tenant.yml

# Optionally delete their data
docker volume rm zeptoclaw_<name>-data
rm -rf tenants/<name>
```

## Resource Limits

Each tenant container is limited by default (set in the compose file):

| Resource | Limit | Reservation |
|----------|-------|-------------|
| Memory | 128MB | 32MB |
| CPU | 0.25 cores | 0.05 cores |

Adjust per tenant in `docker-compose.multi-tenant.yml` under `deploy.resources`.

For tenants with heavy workloads (large Google Sheets, many cron jobs):

```yaml
deploy:
  resources:
    limits:
      memory: 256M
      cpus: "0.5"
```

## Monitoring

```bash
# All tenant status
docker compose -f docker-compose.multi-tenant.yml ps

# Tenant logs
docker logs zc-shop-ahmad --tail 50 -f

# Resource usage per tenant
docker stats --no-stream $(docker ps --filter "name=zc-" -q)

# Restart a tenant
docker compose -f docker-compose.multi-tenant.yml restart tenant-shop-ahmad
```

## Backups

Back up all tenant data volumes:

```bash
#!/bin/bash
# backup-tenants.sh
BACKUP_DIR="/backups/zeptoclaw/$(date +%Y-%m-%d)"
mkdir -p "$BACKUP_DIR"

for volume in $(docker volume ls -q | grep -E "^zeptoclaw_.*-data$"); do
    tenant=$(echo "$volume" | sed 's/zeptoclaw_\(.*\)-data/\1/')
    echo "Backing up $tenant..."
    docker run --rm -v "$volume:/data:ro" -v "$BACKUP_DIR:/backup" \
        alpine tar czf "/backup/$tenant.tar.gz" -C /data .
done

echo "Backups saved to $BACKUP_DIR"
```

Schedule with cron:

```
0 3 * * * /opt/zeptoclaw/backup-tenants.sh
```

## Auto-Restart on Crash

The `restart: unless-stopped` policy in the compose file handles this. If a tenant's container crashes, Docker restarts it automatically.

For extra resilience, add a healthcheck:

```yaml
tenant-shop-ahmad:
  <<: *defaults
  healthcheck:
    test: ["CMD", "pgrep", "zeptoclaw"]
    interval: 30s
    timeout: 5s
    retries: 3
```

## Scaling

For a large number of tenants, generate the compose file programmatically using the included script. It auto-generates service blocks with JSON logging, health checks, and container labels for each tenant in `tenants/`:

```bash
./scripts/generate-compose.sh > docker-compose.multi-tenant.yml
docker compose -f docker-compose.multi-tenant.yml up -d
```

Each generated tenant service includes:
- `RUST_LOG_FORMAT=json` for structured JSON logging
- `ZEPTOCLAW_HEALTH_PORT=9090` for liveness/readiness endpoints
- `com.zeptoclaw.tenant/version/env` container labels for ops filtering
- Docker healthcheck via `wget` to `/healthz`

You can override the version and environment labels:

```bash
ZEPTOCLAW_VERSION=0.2.0 ZEPTOCLAW_ENV=staging ./scripts/generate-compose.sh > docker-compose.multi-tenant.yml
```

## Swarm Configuration

Each tenant can enable agent swarm delegation, allowing the lead agent to spawn specialist sub-agents with role-specific system prompts and tool whitelists:

```json
{
  "swarm": {
    "enabled": true,
    "max_depth": 1,
    "max_concurrent": 3,
    "roles": {
      "researcher": {
        "system_prompt": "You are a research specialist. Find information thoroughly.",
        "tools": ["web_search", "web_fetch", "memory_search"]
      },
      "writer": {
        "system_prompt": "You are a writing specialist. Draft clear, concise content.",
        "tools": ["read_file", "write_file", "edit_file"]
      }
    }
  }
}
```

- `enabled` — Toggle delegation on/off (default: true)
- `max_depth` — Maximum delegation depth; 1 means sub-agents cannot delegate further (default: 1)
- `max_concurrent` — Reserved for future parallel delegation (default: 3)
- `roles` — Pre-defined role presets; if a role has an empty `tools` list, the sub-agent gets all tools except `delegate` and `spawn`

## Security Notes

- Config files are mounted read-only (`:ro`) - the agent cannot modify its own credentials
- Each container runs as non-root user (`zeptoclaw`)
- Containers have no network access to each other by default
- Tenant API keys stay in their own config file, never shared
- Docker volumes are isolated per tenant
- Resource limits prevent any single tenant from starving others
