---
title: Building Custom Nodes
description: Create your own nodes in Rust
---

Extend r8r with custom nodes written in Rust.

## Project setup

Create a new Rust library:

```bash
cargo new --lib my-nodes
cd my-nodes
```

Add dependencies to `Cargo.toml`:

```toml
[dependencies]
r8r-sdk = "0.1"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
async-trait = "0.1"
```

## Define a node

```rust
use r8r_sdk::{Node, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub message: String,
    pub times: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub repeated: Vec<String>,
}

pub struct RepeatNode {
    config: Config,
}

#[async_trait::async_trait]
impl Node for RepeatNode {
    type Config = Config;
    type Output = Output;

    fn new(config: Config) -> Self {
        Self { config }
    }

    async fn execute(&self, ctx: Context) -> Result<Output> {
        let times = self.config.times.unwrap_or(1);
        let repeated = (0..times)
            .map(|_| self.config.message.clone())
            .collect();
        
        Ok(Output { repeated })
    }
}
```

## Register the node

```rust
use r8r_sdk::register_node;

register_node!("custom/repeat", RepeatNode);
```

## Build and install

Build your node library:

```bash
cargo build --release
```

Reference it in `r8r.toml`:

```toml
[nodes]
custom = "./my-nodes/target/release"
```

## Use your node

```yaml
nodes:
  - name: "greet"
    type: "custom/repeat"
    config:
      message: "Hello!"
      times: 3
```

## Accessing context

The `Context` provides access to:

```rust
impl Node for MyNode {
    async fn execute(&self, ctx: Context) -> Result<Output> {
        // Access workflow variables
        let value: Value = ctx.get("previous_node.output")?;
        
        // Access environment
        let api_key = ctx.env("API_KEY")?;
        
        // HTTP client
        let response = ctx.http()
            .get("https://api.example.com")
            .send()
            .await?;
        
        // Logging
        ctx.log("Processing...");
        
        Ok(Output { ... })
    }
}
```

## Testing nodes

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use r8r_sdk::test::*;

    #[tokio::test]
    async fn test_repeat() {
        let node = RepeatNode::new(Config {
            message: "Hi".to_string(),
            times: Some(2),
        });
        
        let ctx = test_context();
        let output = node.execute(ctx).await.unwrap();
        
        assert_eq!(output.repeated, vec!["Hi", "Hi"]);
    }
}
```

## Publishing

Publish to crates.io:

```bash
cargo publish
```

Users can then install via:

```toml
[dependencies]
r8r-node-myawesome = "1.0"
```
