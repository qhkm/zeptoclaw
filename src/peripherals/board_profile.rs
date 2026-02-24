//! Board profile registry for hardware validation.
//!
//! Provides static [`BoardProfile`] definitions that describe the GPIO, ADC,
//! I2C, NVS, and PWM capabilities of a given microcontroller board. All
//! peripheral tools use these profiles to validate pin numbers and
//! capabilities before communicating with hardware.
//!
//! # Example
//!
//! ```
//! use zeptoclaw::peripherals::board_profile::{profile_for, ESP32_PROFILE};
//!
//! let profile = profile_for("esp32").unwrap();
//! assert!(profile.is_valid_gpio(21));
//! assert!(!profile.is_valid_gpio(100));
//!
//! let bus = profile.i2c_bus(0).unwrap();
//! assert_eq!(bus.sda_pin, 21);
//! assert_eq!(bus.scl_pin, 22);
//! ```

/// A single I2C bus descriptor for a board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct I2cBus {
    /// Bus identifier (0-based index).
    pub id: u8,
    /// GPIO pin used for SDA (data).
    pub sda_pin: u8,
    /// GPIO pin used for SCL (clock).
    pub scl_pin: u8,
}

/// Static capability description for a microcontroller board.
///
/// All slices are `'static` so profiles can be stored as `const` values
/// without heap allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardProfile {
    /// Human-readable board name (e.g. `"ESP32"`).
    pub name: &'static str,
    /// GPIO pin numbers available on this board.
    pub gpio_pins: &'static [u8],
    /// ADC-capable pin numbers (subset of `gpio_pins`).
    pub adc_pins: &'static [u8],
    /// I2C buses exposed by this board.
    pub i2c_buses: &'static [I2cBus],
    /// Whether the board exposes non-volatile storage (NVS / flash KV).
    pub has_nvs: bool,
    /// Whether the board supports PWM output.
    pub has_pwm: bool,
}

impl BoardProfile {
    /// Returns `true` if `pin` is listed as a general-purpose GPIO pin.
    pub fn is_valid_gpio(&self, pin: u8) -> bool {
        self.gpio_pins.contains(&pin)
    }

    /// Returns `true` if `pin` supports ADC (analogue-to-digital conversion).
    pub fn is_valid_adc(&self, pin: u8) -> bool {
        self.adc_pins.contains(&pin)
    }

    /// Look up an I2C bus by its `id`.
    ///
    /// Returns `None` if no bus with that id exists on this board.
    pub fn i2c_bus(&self, id: u8) -> Option<&I2cBus> {
        self.i2c_buses.iter().find(|b| b.id == id)
    }
}

// ---------------------------------------------------------------------------
// Built-in profiles
// ---------------------------------------------------------------------------

/// Board profile for the Espressif ESP32 (dual-core Xtensa LX6).
///
/// Pin list covers all pins typically broken out on a 38-pin DevKit.
pub const ESP32_PROFILE: BoardProfile = BoardProfile {
    name: "ESP32",
    gpio_pins: &[
        0, 1, 2, 3, 4, 5, 12, 13, 14, 15, 16, 17, 18, 19, 21, 22, 23, 25, 26, 27, 32, 33, 34, 35,
        36, 39,
    ],
    adc_pins: &[32, 33, 34, 35, 36, 39],
    i2c_buses: &[I2cBus {
        id: 0,
        sda_pin: 21,
        scl_pin: 22,
    }],
    has_nvs: true,
    has_pwm: true,
};

// ---------------------------------------------------------------------------
// Registry lookup
// ---------------------------------------------------------------------------

/// Return the [`BoardProfile`] for a known `board_type` string.
///
/// Matching is case-insensitive on the canonical lowercase identifiers
/// listed below.
///
/// | `board_type` | Profile          |
/// |--------------|------------------|
/// | `"esp32"`    | [`ESP32_PROFILE`] |
///
/// Returns `None` for unknown board types.
pub fn profile_for(board_type: &str) -> Option<&'static BoardProfile> {
    match board_type {
        "esp32" => Some(&ESP32_PROFILE),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_esp32_profile_name() {
        assert_eq!(ESP32_PROFILE.name, "ESP32");
    }

    #[test]
    fn test_valid_gpio_pin() {
        // A pin that is in the ESP32 GPIO list
        assert!(ESP32_PROFILE.is_valid_gpio(21));
        assert!(ESP32_PROFILE.is_valid_gpio(0));
        assert!(ESP32_PROFILE.is_valid_gpio(39));
    }

    #[test]
    fn test_invalid_gpio_pin() {
        // Pins that are NOT in the ESP32 GPIO list
        assert!(!ESP32_PROFILE.is_valid_gpio(100));
        assert!(!ESP32_PROFILE.is_valid_gpio(6)); // 6 is reserved on ESP32
        assert!(!ESP32_PROFILE.is_valid_gpio(255));
    }

    #[test]
    fn test_valid_adc_pin() {
        assert!(ESP32_PROFILE.is_valid_adc(32));
        assert!(ESP32_PROFILE.is_valid_adc(36));
        assert!(ESP32_PROFILE.is_valid_adc(39));
    }

    #[test]
    fn test_invalid_adc_pin() {
        // GPIO-capable but not ADC
        assert!(!ESP32_PROFILE.is_valid_adc(21));
        assert!(!ESP32_PROFILE.is_valid_adc(22));
        assert!(!ESP32_PROFILE.is_valid_adc(100));
    }

    #[test]
    fn test_i2c_bus_lookup() {
        let bus = ESP32_PROFILE.i2c_bus(0).expect("bus 0 should exist");
        assert_eq!(bus.id, 0);
        assert_eq!(bus.sda_pin, 21);
        assert_eq!(bus.scl_pin, 22);

        // Non-existent bus
        assert!(ESP32_PROFILE.i2c_bus(1).is_none());
    }

    #[test]
    fn test_esp32_capabilities() {
        assert!(ESP32_PROFILE.has_nvs);
        assert!(ESP32_PROFILE.has_pwm);
    }

    #[test]
    fn test_profile_for_known_board() {
        let profile = profile_for("esp32").expect("esp32 should be known");
        assert_eq!(profile.name, "ESP32");
    }

    #[test]
    fn test_profile_for_unknown_board() {
        assert!(profile_for("stm32f4").is_none());
        assert!(profile_for("").is_none());
        assert!(profile_for("ESP32").is_none()); // case-sensitive
    }

    #[test]
    fn test_esp32_gpio_pin_count() {
        assert_eq!(ESP32_PROFILE.gpio_pins.len(), 26);
    }
}
