# ESP32 Hardware Tools — GPIO, I2C, NVS, Serial Channel

**Issue:** #130
**Date:** 2026-02-24
**Status:** Approved

## Summary

Add ESP32 hardware control tools behind a `peripheral-esp32` cargo feature gate. Extends the existing peripheral system with generic I2C and NVS tools (reusable by future boards), an ESP32 board wrapper, and a full Serial Channel for UART-based agent messaging.

## Design Decisions

1. **Extend existing peripheral system** — not a standalone module. Reuses `SerialTransport`, `Peripheral` trait, existing GPIO tools.
2. **Generic board tools** — I2C and NVS tools work with any board whose firmware implements the JSON serial protocol. ESP32 is the first consumer.
3. **Board profile registry** — static pin/capability definitions per board type. Validation happens host-side before sending commands.
4. **Full Serial Channel** — `Channel` trait impl for UART messaging, so ESP32 devices can talk to the agent like Telegram/Discord.
5. **No new dependencies** — reuses `tokio-serial` from existing `hardware` feature.

## Architecture

### Board Profile Registry

```
src/peripherals/board_profile.rs
```

```rust
pub struct BoardProfile {
    pub name: &'static str,           // "esp32", "stm32f4", "rpi"
    pub gpio_pins: &'static [u8],     // valid GPIO pin numbers
    pub adc_pins: &'static [u8],      // ADC-capable pins
    pub i2c_buses: &'static [I2cBus], // available I2C buses
    pub has_nvs: bool,                // NVS storage support
    pub has_pwm: bool,                // PWM support
}

pub struct I2cBus {
    pub id: u8,
    pub sda_pin: u8,
    pub scl_pin: u8,
}

pub const ESP32_PROFILE: BoardProfile = BoardProfile {
    name: "esp32",
    gpio_pins: &[0,1,2,3,4,5,12,13,14,15,16,17,18,19,21,22,23,25,26,27,32,33,34,35,36,39],
    adc_pins: &[32,33,34,35,36,39],
    i2c_buses: &[I2cBus { id: 0, sda_pin: 21, scl_pin: 22 }],
    has_nvs: true,
    has_pwm: true,
};

pub fn profile_for(board_type: &str) -> Option<&'static BoardProfile> { ... }
```

Adding a new board = add a `const` profile. Tools validate pins/capabilities against the profile before sending serial commands.

### Generic I2C Tools

```
src/peripherals/i2c.rs
```

Three tools sharing `SerialTransport` + `BoardProfile`:

- **I2cScanTool** — `{"cmd":"i2c_scan","args":{"bus":0}}` → list of detected addresses
- **I2cReadTool** — `{"cmd":"i2c_read","args":{"bus":0,"addr":104,"reg":0,"len":6}}` → hex bytes
- **I2cWriteTool** — `{"cmd":"i2c_write","args":{"bus":0,"addr":104,"reg":0,"data":"0A1B"}}`

All validate `bus` against `profile.i2c_buses` before sending.

### Generic NVS Tools

```
src/peripherals/nvs.rs
```

Three tools for key-value storage on flash:

- **NvsGetTool** — `{"cmd":"nvs_get","args":{"namespace":"config","key":"wifi_ssid"}}`
- **NvsSetTool** — `{"cmd":"nvs_set","args":{"namespace":"config","key":"wifi_ssid","value":"MyNet"}}`
- **NvsDeleteTool** — `{"cmd":"nvs_delete","args":{"namespace":"config","key":"wifi_ssid"}}`

Only registered when `profile.has_nvs == true`.

### Esp32Peripheral

```
src/peripherals/esp32.rs
```

Wraps `SerialPeripheral` with ESP32 board profile:

```rust
pub struct Esp32Peripheral {
    inner: SerialPeripheral,
    profile: &'static BoardProfile,
}

impl Peripheral for Esp32Peripheral {
    fn board_type(&self) -> &str { "esp32" }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        // GPIO (always), I2C (if buses), NVS (if has_nvs)
        // All tools get profile reference for validation
    }
}
```

Feature gate: `peripheral-esp32` in Cargo.toml (reuses `tokio-serial` dep).

### Serial Channel

```
src/channels/serial.rs
```

Full `Channel` trait impl for UART-based agent messaging:

**Message protocol** (line-delimited JSON, distinct from tool protocol):
```
Inbound:  {"type":"message","text":"What's the temperature?","sender":"esp32-0"}
Outbound: {"type":"response","text":"The sensor reads 24.3°C"}
```

Demux from tool commands via `"type"` field (tool commands use `"cmd"`).

When the same serial port is used for both tools and messaging, `SerialTransport` and `SerialChannel` share an `Arc<Mutex<SerialStream>>`.

**Config:**
```json
{
  "channels": {
    "serial": {
      "enabled": true,
      "port": "/dev/ttyUSB0",
      "baud_rate": 115200,
      "deny_by_default": false
    }
  }
}
```

Gated under existing `hardware` feature (no new dep).

### Registration

**Peripheral tools** in `src/cli/common.rs`:
```rust
#[cfg(feature = "peripheral-esp32")]
if let Some(esp32_config) = &config.peripherals.esp32 {
    if esp32_config.enabled {
        let mut peripheral = Esp32Peripheral::new(&esp32_config.port)?;
        peripheral.connect().await?;
        for tool in peripheral.tools() {
            agent.register_tool(tool).await;
        }
    }
}
```

**Serial channel** in `src/cli/gateway.rs`:
```rust
#[cfg(feature = "hardware")]
if let Some(serial_config) = &config.channels.serial {
    if serial_config.enabled {
        let channel = SerialChannel::new(&serial_config.port, serial_config.baud_rate);
        manager.register("serial", Box::new(channel)).await;
    }
}
```

## Feature Gates

```toml
# Cargo.toml
[features]
peripheral-esp32 = ["tokio-serial"]  # ESP32 board profile + peripheral

# Serial channel included under existing `hardware` feature
hardware = ["nusb", "tokio-serial"]
```

## Files

| File | Description |
|------|-------------|
| `src/peripherals/board_profile.rs` | BoardProfile struct, I2cBus, ESP32 const, profile_for() |
| `src/peripherals/i2c.rs` | I2cScanTool, I2cReadTool, I2cWriteTool |
| `src/peripherals/nvs.rs` | NvsGetTool, NvsSetTool, NvsDeleteTool |
| `src/peripherals/esp32.rs` | Esp32Peripheral (Peripheral trait impl) |
| `src/channels/serial.rs` | SerialChannel (Channel trait impl) |

## Test Plan (~40 tests)

| Area | Count | Notes |
|------|-------|-------|
| BoardProfile | 8 | Pin validation, profile lookup, capability checks |
| I2cScanTool | 5 | Valid bus, invalid bus, parse response, timeout |
| I2cRead/WriteTool | 6 | Valid addr/reg, out-of-range, hex encode/decode |
| NvsGet/Set/DeleteTool | 6 | CRUD, missing key, namespace validation |
| Esp32Peripheral | 5 | Connect, health check, tools() returns correct set, disconnect |
| SerialChannel | 6 | Start, send, receive, demux tool vs message, port sharing |
| Integration | 4 | Feature gate compiles, stub errors, config loading |

All tests use mock `SerialTransport` — no real hardware needed.

## Binary Impact

Zero additional dependencies beyond what `hardware` already pulls in. Feature-gated so default binary size unchanged.
