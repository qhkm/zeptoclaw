---
title: Deployment
description: Deploy r8r to production
---

Deploy r8r workflows to various platforms and environments.

## Binary deployment

### Build optimized release

```bash
cargo build --release
```

The binary is at `target/release/r8r` (~15MB).

### Deploy with systemd

Create `/etc/systemd/system/r8r.service`:

```ini
[Unit]
Description=r8r workflow server
After=network.target

[Service]
Type=simple
User=r8r
WorkingDirectory=/opt/r8r
ExecStart=/usr/local/bin/r8r run --workflows /opt/r8r/workflows
Restart=on-failure
Environment="R8R_PROFILE=production"
Environment="SLACK_TOKEN=xxx"

[Install]
WantedBy=multi-user.target
```

Enable and start:

```bash
sudo systemctl enable r8r
sudo systemctl start r8r
```

## Docker deployment

### Dockerfile

```dockerfile
FROM debian:bookworm-slim

COPY target/release/r8r /usr/local/bin/r8r
COPY workflows /workflows
COPY r8r.toml /etc/r8r/

ENV R8R_CONFIG=/etc/r8r/r8r.toml

EXPOSE 3000

CMD ["r8r", "run", "--workflows", "/workflows"]
```

Build and run:

```bash
docker build -t my-r8r .
docker run -p 3000:3000 my-r8r
```

## Fly.io deployment

```bash
# Install flyctl
curl -L https://fly.io/install.sh | sh

# Launch app
fly launch

# Deploy
fly deploy
```

`fly.toml`:

```toml
app = "my-r8r"

[build]
  dockerfile = "Dockerfile"

[env]
  R8R_PROFILE = "production"

[[services]]
  internal_port = 3000
  protocol = "tcp"

  [[services.ports]]
    handlers = ["http"]
    port = 80
    force_https = true

  [[services.ports]]
    handlers = ["tls", "http"]
    port = 443
```

## Railway deployment

```bash
# Install Railway CLI
npm i -g @railway/cli

# Login and link
railway login
railway link

# Deploy
railway up
```

## Kubernetes deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: r8r
spec:
  replicas: 2
  selector:
    matchLabels:
      app: r8r
  template:
    metadata:
      labels:
        app: r8r
    spec:
      containers:
        - name: r8r
          image: ghcr.io/r8r/r8r:latest
          ports:
            - containerPort: 3000
          env:
            - name: R8R_PROFILE
              value: production
          resources:
            requests:
              memory: "32Mi"
              cpu: "100m"
            limits:
              memory: "128Mi"
              cpu: "500m"
---
apiVersion: v1
kind: Service
metadata:
  name: r8r
spec:
  selector:
    app: r8r
  ports:
    - port: 80
      targetPort: 3000
```

## Environment-specific config

```toml
# Base config
[server]
port = 3000

# Production overrides
[profile.production]
[profile.production.server]
port = 8080

[profile.production.logging]
level = "warn"
format = "json"
```

Deploy with profile:

```bash
R8R_PROFILE=production r8r run
```

## Health checks

Enable health endpoint:

```toml
[server]
health_check = true
```

Kubernetes probe:

```yaml
livenessProbe:
  httpGet:
    path: /health
    port: 3000
  initialDelaySeconds: 5
  periodSeconds: 10
```

## Monitoring

Export metrics:

```toml
[metrics]
enabled = true
path = "/metrics"
```

Prometheus scraping:

```yaml
annotations:
  prometheus.io/scrape: "true"
  prometheus.io/port: "3000"
  prometheus.io/path: "/metrics"
```
