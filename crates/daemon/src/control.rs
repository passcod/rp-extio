use rp_extio_wire::{
    ClockSource, Peripheral, PinMap, PinSignal, PinValidationError, PinValidationKind,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

const fn enabled_by_default() -> bool {
    true
}

const fn default_adc_pins() -> [u8; 3] {
    rp_extio_wire::DEFAULT_PIN_MAP.adc_pins
}

const fn default_pwm_pins() -> [u8; 16] {
    rp_extio_wire::DEFAULT_PIN_MAP.pwm_pins
}

const fn default_clock_pins() -> [u8; 2] {
    rp_extio_wire::DEFAULT_PIN_MAP.clock_pins
}

const fn default_clock_sources() -> [ClockSourceConfig; 2] {
    [ClockSourceConfig::System; 2]
}

const fn default_clock_hz() -> [u32; 2] {
    rp_extio_wire::DEFAULT_PIN_MAP.clock_hz
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockSourceConfig {
    System,
    Reference,
    Usb,
    Adc,
    Crystal,
    RingOscillator,
    LowPowerOscillator,
}

impl From<ClockSource> for ClockSourceConfig {
    fn from(source: ClockSource) -> Self {
        match source {
            ClockSource::System => Self::System,
            ClockSource::Reference => Self::Reference,
            ClockSource::Usb => Self::Usb,
            ClockSource::Adc => Self::Adc,
            ClockSource::Crystal => Self::Crystal,
            ClockSource::RingOscillator => Self::RingOscillator,
            ClockSource::LowPowerOscillator => Self::LowPowerOscillator,
        }
    }
}

impl From<ClockSourceConfig> for ClockSource {
    fn from(source: ClockSourceConfig) -> Self {
        match source {
            ClockSourceConfig::System => Self::System,
            ClockSourceConfig::Reference => Self::Reference,
            ClockSourceConfig::Usb => Self::Usb,
            ClockSourceConfig::Adc => Self::Adc,
            ClockSourceConfig::Crystal => Self::Crystal,
            ClockSourceConfig::RingOscillator => Self::RingOscillator,
            ClockSourceConfig::LowPowerOscillator => Self::LowPowerOscillator,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PinMapConfig {
    pub uart0_enabled: bool,
    pub uart0_tx: u8,
    pub uart0_rx: u8,
    pub uart1_enabled: bool,
    pub uart1_tx: u8,
    pub uart1_rx: u8,
    pub i2c0_enabled: bool,
    pub i2c0_sda: u8,
    pub i2c0_scl: u8,
    pub i2c1_enabled: bool,
    pub i2c1_sda: u8,
    pub i2c1_scl: u8,
    pub spi0_enabled: bool,
    pub spi0_miso: u8,
    pub spi0_sck: u8,
    pub spi0_mosi: u8,
    pub spi0_cs0: u8,
    #[serde(default = "enabled_by_default")]
    pub spi0_cs1_enabled: bool,
    pub spi0_cs1: u8,
    pub spi1_enabled: bool,
    pub spi1_miso: u8,
    pub spi1_sck: u8,
    pub spi1_mosi: u8,
    pub spi1_cs0: u8,
    #[serde(default = "enabled_by_default")]
    pub spi1_cs1_enabled: bool,
    pub spi1_cs1: u8,
    #[serde(default)]
    pub adc_enabled: [bool; 3],
    #[serde(default = "default_adc_pins")]
    pub adc_pins: [u8; 3],
    #[serde(default)]
    pub pwm_enabled: [bool; 16],
    #[serde(default = "default_pwm_pins")]
    pub pwm_pins: [u8; 16],
    #[serde(default)]
    pub clock_enabled: [bool; 2],
    #[serde(default = "default_clock_pins")]
    pub clock_pins: [u8; 2],
    #[serde(default = "default_clock_sources")]
    pub clock_sources: [ClockSourceConfig; 2],
    #[serde(default = "default_clock_hz")]
    pub clock_hz: [u32; 2],
}

impl From<PinMap> for PinMapConfig {
    fn from(map: PinMap) -> Self {
        Self {
            uart0_enabled: map.is_enabled(Peripheral::Uart0),
            uart0_tx: map.uart_tx[0],
            uart0_rx: map.uart_rx[0],
            uart1_enabled: map.is_enabled(Peripheral::Uart1),
            uart1_tx: map.uart_tx[1],
            uart1_rx: map.uart_rx[1],
            i2c0_enabled: map.is_enabled(Peripheral::I2c0),
            i2c0_sda: map.i2c_sda[0],
            i2c0_scl: map.i2c_scl[0],
            i2c1_enabled: map.is_enabled(Peripheral::I2c1),
            i2c1_sda: map.i2c_sda[1],
            i2c1_scl: map.i2c_scl[1],
            spi0_enabled: map.is_enabled(Peripheral::Spi0),
            spi0_miso: map.spi_miso[0],
            spi0_sck: map.spi_sck[0],
            spi0_mosi: map.spi_mosi[0],
            spi0_cs0: map.spi_chip_selects[0][0],
            spi0_cs1_enabled: map.spi_cs1_enabled(0),
            spi0_cs1: map.spi_chip_selects[0][1],
            spi1_enabled: map.is_enabled(Peripheral::Spi1),
            spi1_miso: map.spi_miso[1],
            spi1_sck: map.spi_sck[1],
            spi1_mosi: map.spi_mosi[1],
            spi1_cs0: map.spi_chip_selects[1][0],
            spi1_cs1_enabled: map.spi_cs1_enabled(1),
            spi1_cs1: map.spi_chip_selects[1][1],
            adc_enabled: core::array::from_fn(|channel| map.adc_enabled & (1 << channel) != 0),
            adc_pins: map.adc_pins,
            pwm_enabled: core::array::from_fn(|channel| map.pwm_enabled & (1 << channel) != 0),
            pwm_pins: map.pwm_pins,
            clock_enabled: core::array::from_fn(|output| map.clock_enabled & (1 << output) != 0),
            clock_pins: map.clock_pins,
            clock_sources: map.clock_sources.map(Into::into),
            clock_hz: map.clock_hz,
        }
    }
}

impl From<PinMapConfig> for PinMap {
    fn from(map: PinMapConfig) -> Self {
        let mut enabled = 0;
        for (peripheral, value) in [
            (Peripheral::Uart0, map.uart0_enabled),
            (Peripheral::Uart1, map.uart1_enabled),
            (Peripheral::I2c0, map.i2c0_enabled),
            (Peripheral::I2c1, map.i2c1_enabled),
            (Peripheral::Spi0, map.spi0_enabled),
            (Peripheral::Spi1, map.spi1_enabled),
        ] {
            if value {
                enabled |= peripheral.mask();
            }
        }
        let mut pins = Self {
            uart_tx: [map.uart0_tx, map.uart1_tx],
            uart_rx: [map.uart0_rx, map.uart1_rx],
            i2c_sda: [map.i2c0_sda, map.i2c1_sda],
            i2c_scl: [map.i2c0_scl, map.i2c1_scl],
            spi_miso: [map.spi0_miso, map.spi1_miso],
            spi_sck: [map.spi0_sck, map.spi1_sck],
            spi_mosi: [map.spi0_mosi, map.spi1_mosi],
            spi_chip_selects: [[map.spi0_cs0, map.spi0_cs1], [map.spi1_cs0, map.spi1_cs1]],
            enabled,
            adc_pins: map.adc_pins,
            adc_enabled: map
                .adc_enabled
                .into_iter()
                .enumerate()
                .fold(0, |mask, (channel, enabled)| {
                    mask | (u8::from(enabled) << channel)
                }),
            pwm_pins: map.pwm_pins,
            pwm_enabled: map
                .pwm_enabled
                .into_iter()
                .enumerate()
                .fold(0, |mask, (channel, enabled)| {
                    mask | (u16::from(enabled) << channel)
                }),
            clock_pins: map.clock_pins,
            clock_enabled: map
                .clock_enabled
                .into_iter()
                .enumerate()
                .fold(0, |mask, (output, enabled)| {
                    mask | (u8::from(enabled) << output)
                }),
            clock_sources: map.clock_sources.map(Into::into),
            clock_hz: map.clock_hz,
        };
        pins.set_spi_cs1_enabled(0, map.spi0_cs1_enabled);
        pins.set_spi_cs1_enabled(1, map.spi1_cs1_enabled);
        pins
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlRequest {
    List,
    Show {
        serial: String,
    },
    Set {
        serial: String,
        generation: u64,
        map: PinMapConfig,
    },
    Default {
        serial: String,
        generation: u64,
    },
    Reload,
    AdcRead {
        serial: String,
        channel: u8,
    },
    PwmGet {
        serial: String,
        channel: u8,
    },
    PwmApply {
        serial: String,
        pwm: PwmInfo,
    },
    GpioSnapshot {
        serial: String,
    },
    GpioOutput {
        serial: String,
        pin: u8,
        value: bool,
    },
    GpioInput {
        serial: String,
        pin: u8,
        bias: GpioBias,
    },
    GpioStartupSet {
        serial: String,
        startup: GpioStartup,
    },
    GpioStartupClear {
        serial: String,
        pin: u8,
    },
}

impl ControlRequest {
    pub const fn requires_authorization(&self) -> bool {
        !matches!(
            self,
            Self::List
                | Self::Show { .. }
                | Self::AdcRead { .. }
                | Self::PwmGet { .. }
                | Self::GpioSnapshot { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GpioBias {
    None,
    PullUp,
    PullDown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "direction", rename_all = "snake_case", deny_unknown_fields)]
pub enum GpioStartup {
    Input { pin: u8, bias: GpioBias },
    Output { pin: u8, value: bool },
}

impl GpioStartup {
    pub const fn pin(self) -> u8 {
        match self {
            Self::Input { pin, .. } | Self::Output { pin, .. } => pin,
        }
    }
}

impl GpioBias {
    pub const fn wire_value(self) -> u8 {
        match self {
            Self::None => rp_extio_wire::GPIO_BIAS_NONE,
            Self::PullUp => rp_extio_wire::GPIO_BIAS_PULL_UP,
            Self::PullDown => rp_extio_wire::GPIO_BIAS_PULL_DOWN,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GpioInfo {
    pub line: u8,
    pub pin: u8,
    pub level: bool,
    pub output: bool,
    pub bias: GpioBias,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PwmInfo {
    pub channel: u8,
    pub enabled: bool,
    pub inverted: bool,
    pub period_ns: u64,
    pub duty_ns: u64,
}

impl From<rp_extio_wire::PwmState> for PwmInfo {
    fn from(state: rp_extio_wire::PwmState) -> Self {
        Self {
            channel: state.channel,
            enabled: state.enabled,
            inverted: state.inverted,
            period_ns: state.period_ns,
            duty_ns: state.duty_ns,
        }
    }
}

impl From<PwmInfo> for rp_extio_wire::PwmState {
    fn from(state: PwmInfo) -> Self {
        Self {
            channel: state.channel,
            enabled: state.enabled,
            inverted: state.inverted,
            period_ns: state.period_ns,
            duty_ns: state.duty_ns,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BoardInfo {
    pub serial: String,
    pub connected: bool,
    pub active: Option<PinMapConfig>,
    pub desired: Option<PinMapConfig>,
    pub generation: Option<u64>,
    pub i2c_buses: [Option<i32>; 2],
    pub spi_buses: [Option<i32>; 2],
    pub gpio_instance: Option<u32>,
    pub iio_device: Option<i32>,
    pub pwm_chip: Option<i32>,
    pub gpio_startup: Vec<GpioStartup>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ValidationInfo {
    pub kind: String,
    pub signal: String,
    pub other: Option<String>,
    pub pin: u8,
}

impl From<PinValidationError> for ValidationInfo {
    fn from(error: PinValidationError) -> Self {
        Self {
            kind: validation_kind_name(error.kind).into(),
            signal: signal_name(error.signal).into(),
            other: error.other.map(|signal| signal_name(signal).into()),
            pin: error.pin,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlResponse {
    Boards {
        boards: Vec<BoardInfo>,
    },
    Board {
        board: BoardInfo,
    },
    Applied {
        board: BoardInfo,
    },
    Ok,
    Adc {
        channel: u8,
        raw: u16,
        reference_mv: u16,
        resolution_bits: u8,
    },
    Pwm {
        pwm: PwmInfo,
    },
    GpioSnapshot {
        gpio: Vec<GpioInfo>,
    },
    Gpio {
        gpio: GpioInfo,
    },
    Error {
        code: String,
        message: String,
        validation: Option<ValidationInfo>,
    },
}

impl ControlResponse {
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
            validation: None,
        }
    }

    pub fn validation(error: PinValidationError) -> Self {
        Self::Error {
            code: "invalid_pin_map".into(),
            message: format_validation(error),
            validation: Some(error.into()),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    version: u32,
    boards: BTreeMap<String, PinMapConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConfigFileV4 {
    version: u32,
    boards: BTreeMap<String, BoardConfigV4>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BoardConfigV4 {
    #[serde(flatten)]
    pin_map: PinMapConfig,
    #[serde(default)]
    gpio_startup: Vec<GpioStartup>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyConfigFile {
    version: u32,
    boards: BTreeMap<String, LegacyPinMapConfig>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyPinMapConfig {
    uart_tx: u8,
    uart_rx: u8,
    i2c_sda: u8,
    i2c_scl: u8,
    spi_miso: u8,
    spi_sck: u8,
    spi_mosi: u8,
    spi_cs0: u8,
    spi_cs1: u8,
}

impl From<LegacyPinMapConfig> for PinMap {
    fn from(legacy: LegacyPinMapConfig) -> Self {
        let mut map = rp_extio_wire::DEFAULT_PIN_MAP;
        map.uart_tx[0] = legacy.uart_tx;
        map.uart_rx[0] = legacy.uart_rx;
        map.i2c_sda[0] = legacy.i2c_sda;
        map.i2c_scl[0] = legacy.i2c_scl;
        map.spi_miso[0] = legacy.spi_miso;
        map.spi_sck[0] = legacy.spi_sck;
        map.spi_mosi[0] = legacy.spi_mosi;
        map.spi_chip_selects[0] = [legacy.spi_cs0, legacy.spi_cs1];
        map
    }
}

pub struct ConfigStore {
    path: PathBuf,
    boards: BTreeMap<String, PinMap>,
    gpio_startup: BTreeMap<String, Vec<GpioStartup>>,
}

impl ConfigStore {
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let (boards, gpio_startup) = match fs::read(&path) {
            Ok(bytes) => {
                let value: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(invalid_data)?;
                let version = value
                    .get("version")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "configuration version is missing or invalid",
                        )
                    })?;
                let (maps, startup): (BTreeMap<String, PinMap>, _) = match version {
                    1 => {
                        let config: LegacyConfigFile =
                            serde_json::from_value(value).map_err(invalid_data)?;
                        debug_assert_eq!(config.version, 1);
                        (
                            config
                                .boards
                                .into_iter()
                                .map(|(serial, map)| (serial, map.into()))
                                .collect(),
                            BTreeMap::new(),
                        )
                    }
                    2 | 3 => {
                        let config: ConfigFile =
                            serde_json::from_value(value).map_err(invalid_data)?;
                        debug_assert!(matches!(config.version, 2 | 3));
                        (
                            config
                                .boards
                                .into_iter()
                                .map(|(serial, map)| (serial, map.into()))
                                .collect(),
                            BTreeMap::new(),
                        )
                    }
                    4 => {
                        let config: ConfigFileV4 =
                            serde_json::from_value(value).map_err(invalid_data)?;
                        debug_assert_eq!(config.version, 4);
                        let mut maps = BTreeMap::new();
                        let mut startup = BTreeMap::new();
                        for (serial, board) in config.boards {
                            maps.insert(serial.clone(), board.pin_map.into());
                            if !board.gpio_startup.is_empty() {
                                startup.insert(serial, board.gpio_startup);
                            }
                        }
                        (maps, startup)
                    }
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("unsupported configuration version {version}"),
                        ));
                    }
                };
                let boards = maps
                    .into_iter()
                    .map(|(serial, map)| {
                        map.validate().map_err(|error| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!(
                                    "invalid pin map for {serial}: {}",
                                    format_validation(error)
                                ),
                            )
                        })?;
                        Ok((serial, map))
                    })
                    .collect::<io::Result<BTreeMap<_, _>>>()?;
                for (serial, entries) in &startup {
                    let map = boards.get(serial).expect("version 4 board has a pin map");
                    validate_gpio_startup(map, entries).map_err(|message| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("invalid GPIO startup state for {serial}: {message}"),
                        )
                    })?;
                }
                (boards, startup)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                (BTreeMap::new(), BTreeMap::new())
            }
            Err(error) => return Err(error),
        };
        Ok(Self {
            path,
            boards,
            gpio_startup,
        })
    }

    pub fn boards(&self) -> &BTreeMap<String, PinMap> {
        &self.boards
    }

    pub fn get(&self, serial: &str) -> Option<PinMap> {
        self.boards.get(serial).copied()
    }

    pub fn contains_board(&self, serial: &str) -> bool {
        self.boards.contains_key(serial)
    }

    pub fn gpio_startup(&self, serial: &str) -> &[GpioStartup] {
        self.gpio_startup.get(serial).map_or(&[], Vec::as_slice)
    }

    pub fn replace(&mut self, boards: BTreeMap<String, PinMap>) {
        self.boards = boards;
        self.gpio_startup.clear();
    }

    pub fn reload(&mut self) -> io::Result<()> {
        let replacement = Self::load(self.path.clone())?;
        self.boards = replacement.boards;
        self.gpio_startup = replacement.gpio_startup;
        Ok(())
    }

    pub fn set(&mut self, serial: String, map: PinMap) -> io::Result<()> {
        validate_gpio_startup(&map, self.gpio_startup(&serial))
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
        let previous = self.boards.insert(serial.clone(), map);
        if let Err(error) = self.save() {
            match previous {
                Some(previous) => self.boards.insert(serial, previous),
                None => self.boards.remove(&serial),
            };
            return Err(error);
        }
        Ok(())
    }

    pub fn remove(&mut self, serial: &str) -> io::Result<()> {
        let previous = self.boards.remove(serial);
        let previous_startup = self.gpio_startup.remove(serial);
        if let Err(error) = self.save() {
            if let Some(previous) = previous {
                self.boards.insert(serial.into(), previous);
            }
            if let Some(previous) = previous_startup {
                self.gpio_startup.insert(serial.into(), previous);
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn set_gpio_startup(&mut self, serial: String, startup: GpioStartup) -> io::Result<()> {
        let map = self
            .boards
            .get(&serial)
            .copied()
            .unwrap_or(rp_extio_wire::DEFAULT_PIN_MAP);
        let previous_map = self.boards.insert(serial.clone(), map);
        let previous = self.gpio_startup.get(&serial).cloned();
        let entries = self.gpio_startup.entry(serial.clone()).or_default();
        entries.retain(|entry| entry.pin() != startup.pin());
        entries.push(startup);
        entries.sort_by_key(|entry| entry.pin());
        if let Err(message) = validate_gpio_startup(&map, entries) {
            restore_entry(&mut self.boards, &serial, previous_map);
            restore_entry(&mut self.gpio_startup, &serial, previous);
            return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
        }
        if let Err(error) = self.save() {
            restore_entry(&mut self.boards, &serial, previous_map);
            restore_entry(&mut self.gpio_startup, &serial, previous);
            return Err(error);
        }
        Ok(())
    }

    pub fn clear_gpio_startup(&mut self, serial: &str, pin: u8) -> io::Result<()> {
        let previous = self.gpio_startup.get(serial).cloned();
        let Some(entries) = self.gpio_startup.get_mut(serial) else {
            return Ok(());
        };
        entries.retain(|entry| entry.pin() != pin);
        if entries.is_empty() {
            self.gpio_startup.remove(serial);
        }
        if let Err(error) = self.save() {
            restore_entry(&mut self.gpio_startup, serial, previous);
            return Err(error);
        }
        Ok(())
    }

    fn save(&self) -> io::Result<()> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let name = self
            .path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "config path has no name"))?
            .to_string_lossy();
        let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let config = ConfigFileV4 {
                version: 4,
                boards: self
                    .boards
                    .iter()
                    .map(|(serial, map)| {
                        (
                            serial.clone(),
                            BoardConfigV4 {
                                pin_map: (*map).into(),
                                gpio_startup: self.gpio_startup(serial).to_vec(),
                            },
                        )
                    })
                    .collect(),
            };
            serde_json::to_writer_pretty(&mut file, &config).map_err(io::Error::other)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn restore_entry<T>(map: &mut BTreeMap<String, T>, key: &str, value: Option<T>) {
    match value {
        Some(value) => {
            map.insert(key.into(), value);
        }
        None => {
            map.remove(key);
        }
    }
}

pub fn validate_gpio_startup(map: &PinMap, entries: &[GpioStartup]) -> Result<(), String> {
    let pins = map.gpio_pins().map_err(|error| format_validation(error))?;
    let mut seen = Vec::new();
    for entry in entries {
        let pin = entry.pin();
        if !pins.as_slice().contains(&pin) {
            return Err(format!("GP{pin} is not configured as GPIO"));
        }
        if seen.contains(&pin) {
            return Err(format!("GP{pin} has more than one startup state"));
        }
        seen.push(pin);
    }
    Ok(())
}

pub fn signal_name(signal: PinSignal) -> &'static str {
    match signal {
        PinSignal::Uart0Tx => "uart0_tx",
        PinSignal::Uart0Rx => "uart0_rx",
        PinSignal::Uart1Tx => "uart1_tx",
        PinSignal::Uart1Rx => "uart1_rx",
        PinSignal::I2c0Sda => "i2c0_sda",
        PinSignal::I2c0Scl => "i2c0_scl",
        PinSignal::I2c1Sda => "i2c1_sda",
        PinSignal::I2c1Scl => "i2c1_scl",
        PinSignal::Spi0Miso => "spi0_miso",
        PinSignal::Spi0Sck => "spi0_sck",
        PinSignal::Spi0Mosi => "spi0_mosi",
        PinSignal::Spi0Cs0 => "spi0_cs0",
        PinSignal::Spi0Cs1 => "spi0_cs1",
        PinSignal::Spi1Miso => "spi1_miso",
        PinSignal::Spi1Sck => "spi1_sck",
        PinSignal::Spi1Mosi => "spi1_mosi",
        PinSignal::Spi1Cs0 => "spi1_cs0",
        PinSignal::Spi1Cs1 => "spi1_cs1",
        PinSignal::Adc0 => "adc0",
        PinSignal::Adc1 => "adc1",
        PinSignal::Adc2 => "adc2",
        PinSignal::Pwm0A => "pwm0a",
        PinSignal::Pwm0B => "pwm0b",
        PinSignal::Pwm1A => "pwm1a",
        PinSignal::Pwm1B => "pwm1b",
        PinSignal::Pwm2A => "pwm2a",
        PinSignal::Pwm2B => "pwm2b",
        PinSignal::Pwm3A => "pwm3a",
        PinSignal::Pwm3B => "pwm3b",
        PinSignal::Pwm4A => "pwm4a",
        PinSignal::Pwm4B => "pwm4b",
        PinSignal::Pwm5A => "pwm5a",
        PinSignal::Pwm5B => "pwm5b",
        PinSignal::Pwm6A => "pwm6a",
        PinSignal::Pwm6B => "pwm6b",
        PinSignal::Pwm7A => "pwm7a",
        PinSignal::Pwm7B => "pwm7b",
        PinSignal::Clock0 => "clock0",
        PinSignal::Clock1 => "clock1",
    }
}

fn validation_kind_name(kind: PinValidationKind) -> &'static str {
    match kind {
        PinValidationKind::UnavailablePin => "unavailable_pin",
        PinValidationKind::InvalidRoute => "invalid_route",
        PinValidationKind::MismatchedPair => "mismatched_pair",
        PinValidationKind::DuplicatePin => "duplicate_pin",
        PinValidationKind::InvalidEnableMask => "invalid_enable_mask",
    }
}

pub fn format_validation(error: PinValidationError) -> String {
    let signal = signal_name(error.signal);
    match error.kind {
        PinValidationKind::UnavailablePin => {
            format!("GP{} is not an exposed Pico 2 pin for {signal}", error.pin)
        }
        PinValidationKind::InvalidRoute => {
            format!("GP{} is not a valid route for {signal}", error.pin)
        }
        PinValidationKind::MismatchedPair => {
            format!(
                "{signal} does not form a matching pair with {}",
                signal_name(error.other.unwrap())
            )
        }
        PinValidationKind::DuplicatePin => format!(
            "GP{} is assigned to both {signal} and {}",
            error.pin,
            signal_name(error.other.unwrap())
        ),
        PinValidationKind::InvalidEnableMask => {
            format!("invalid peripheral enable mask {:#04x}", error.pin)
        }
    }
}

fn invalid_data(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "rp-extio-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn control_messages_round_trip() {
        let request = ControlRequest::Set {
            serial: "ABC".into(),
            generation: 7,
            map: rp_extio_wire::DEFAULT_PIN_MAP.into(),
        };
        let encoded = serde_json::to_vec(&request).unwrap();
        assert_eq!(
            serde_json::from_slice::<ControlRequest>(&encoded).unwrap(),
            request
        );

        let response = ControlResponse::Board {
            board: BoardInfo {
                serial: "ABC".into(),
                connected: true,
                active: Some(rp_extio_wire::DEFAULT_PIN_MAP.into()),
                desired: None,
                generation: Some(7),
                i2c_buses: [Some(12), None],
                spi_buses: [Some(4), None],
                gpio_instance: Some(99),
                iio_device: Some(2),
                pwm_chip: Some(1),
                gpio_startup: vec![GpioStartup::Output {
                    pin: 22,
                    value: true,
                }],
            },
        };
        let encoded = serde_json::to_vec(&response).unwrap();
        assert_eq!(
            serde_json::from_slice::<ControlResponse>(&encoded).unwrap(),
            response
        );
    }

    #[test]
    fn only_read_requests_bypass_authorization() {
        assert!(!ControlRequest::List.requires_authorization());
        assert!(
            !ControlRequest::Show {
                serial: "ABC".into()
            }
            .requires_authorization()
        );
        assert!(ControlRequest::Reload.requires_authorization());
        assert!(
            ControlRequest::Default {
                serial: "ABC".into(),
                generation: 1,
            }
            .requires_authorization()
        );
        assert!(
            ControlRequest::Set {
                serial: "ABC".into(),
                generation: 1,
                map: rp_extio_wire::DEFAULT_PIN_MAP.into(),
            }
            .requires_authorization()
        );
        assert!(
            !ControlRequest::AdcRead {
                serial: "ABC".into(),
                channel: 0,
            }
            .requires_authorization()
        );
        assert!(
            !ControlRequest::PwmGet {
                serial: "ABC".into(),
                channel: 0,
            }
            .requires_authorization()
        );
        assert!(
            !ControlRequest::GpioSnapshot {
                serial: "ABC".into(),
            }
            .requires_authorization()
        );
        assert!(
            ControlRequest::GpioOutput {
                serial: "ABC".into(),
                pin: 22,
                value: true,
            }
            .requires_authorization()
        );
    }

    #[test]
    fn version_one_configuration_migrates_to_instance_zero() {
        let directory = TemporaryDirectory::new("config-migration");
        let path = directory.0.join("config.json");
        fs::write(
            &path,
            r#"{
                "version": 1,
                "boards": {
                    "ABC": {
                        "uart_tx": 14, "uart_rx": 15,
                        "i2c_sda": 12, "i2c_scl": 13,
                        "spi_miso": 0, "spi_sck": 2, "spi_mosi": 3,
                        "spi_cs0": 16, "spi_cs1": 17
                    }
                }
            }"#,
        )
        .unwrap();

        let mut store = ConfigStore::load(path.clone()).unwrap();
        let map = store.get("ABC").unwrap();
        assert_eq!((map.uart_tx[0], map.uart_rx[0]), (14, 15));
        assert_eq!((map.i2c_sda[0], map.i2c_scl[0]), (12, 13));
        assert_eq!((map.uart_tx[1], map.uart_rx[1]), (8, 9));
        assert!(map.is_enabled(Peripheral::Uart0));
        assert!(!map.is_enabled(Peripheral::Uart1));

        store.set("ABC".into(), map).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["version"], 4);
        assert_eq!(saved["boards"]["ABC"]["uart1_enabled"], false);
        assert_eq!(
            saved["boards"]["ABC"]["adc_enabled"],
            serde_json::json!([false, false, false])
        );
    }

    #[test]
    fn existing_version_two_maps_default_second_chip_selects_on() {
        let config: PinMapConfig = serde_json::from_str(
            r#"{
                "uart0_enabled": true, "uart0_tx": 0, "uart0_rx": 1,
                "uart1_enabled": false, "uart1_tx": 8, "uart1_rx": 9,
                "i2c0_enabled": true, "i2c0_sda": 4, "i2c0_scl": 5,
                "i2c1_enabled": false, "i2c1_sda": 10, "i2c1_scl": 11,
                "spi0_enabled": true,
                "spi0_miso": 16, "spi0_sck": 18, "spi0_mosi": 19,
                "spi0_cs0": 17, "spi0_cs1": 21,
                "spi1_enabled": false,
                "spi1_miso": 12, "spi1_sck": 14, "spi1_mosi": 15,
                "spi1_cs0": 13, "spi1_cs1": 20
            }"#,
        )
        .unwrap();
        assert!(config.spi0_cs1_enabled);
        assert!(config.spi1_cs1_enabled);
        let map = PinMap::from(config);
        assert_eq!(map.enabled, rp_extio_wire::DEFAULT_PIN_MAP.enabled);
        assert_eq!(map.adc_enabled, 0);
        assert_eq!(map.pwm_enabled, 0);
        assert_eq!(map.clock_enabled, 0);
    }

    #[test]
    fn version_four_gpio_startup_round_trips() {
        let directory = TemporaryDirectory::new("gpio-startup-roundtrip");
        let path = directory.0.join("config.json");
        let mut store = ConfigStore::load(path.clone()).unwrap();
        store
            .set_gpio_startup(
                "ABC".into(),
                GpioStartup::Output {
                    pin: 22,
                    value: true,
                },
            )
            .unwrap();
        store
            .set_gpio_startup(
                "ABC".into(),
                GpioStartup::Input {
                    pin: 28,
                    bias: GpioBias::PullUp,
                },
            )
            .unwrap();

        let loaded = ConfigStore::load(path).unwrap();
        assert_eq!(
            loaded.gpio_startup("ABC"),
            &[
                GpioStartup::Output {
                    pin: 22,
                    value: true,
                },
                GpioStartup::Input {
                    pin: 28,
                    bias: GpioBias::PullUp,
                },
            ]
        );
    }

    #[test]
    fn version_four_rejects_duplicate_and_peripheral_gpio_startup_pins() {
        let directory = TemporaryDirectory::new("gpio-startup-validation");
        let path = directory.0.join("config.json");
        let board =
            serde_json::to_value(PinMapConfig::from(rp_extio_wire::DEFAULT_PIN_MAP)).unwrap();
        let mut board = board.as_object().unwrap().clone();
        board.insert(
            "gpio_startup".into(),
            serde_json::json!([
                {"direction":"output", "pin":22, "value":true},
                {"direction":"input", "pin":22, "bias":"pull_up"}
            ]),
        );
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "version": 4,
                "boards": {"ABC": board}
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(
            ConfigStore::load(path.clone())
                .err()
                .unwrap()
                .to_string()
                .contains("more than one startup state")
        );

        board.insert(
            "gpio_startup".into(),
            serde_json::json!([{"direction":"output", "pin":0, "value":true}]),
        );
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "version": 4,
                "boards": {"ABC": board}
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(
            ConfigStore::load(path)
                .err()
                .unwrap()
                .to_string()
                .contains("GP0 is not configured as GPIO")
        );
    }

    #[test]
    fn pin_map_cannot_claim_a_persisted_gpio() {
        let directory = TemporaryDirectory::new("gpio-startup-map-conflict");
        let path = directory.0.join("config.json");
        let mut store = ConfigStore::load(path).unwrap();
        store
            .set_gpio_startup(
                "ABC".into(),
                GpioStartup::Output {
                    pin: 22,
                    value: false,
                },
            )
            .unwrap();
        let mut map = rp_extio_wire::DEFAULT_PIN_MAP;
        map.pwm_pins[6] = 22;
        map.pwm_enabled |= 1 << 6;
        assert!(store.set("ABC".into(), map).is_err());
    }
}
