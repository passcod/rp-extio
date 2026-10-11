use rp_extio_wire::{Peripheral, PinMap, PinSignal, PinValidationError, PinValidationKind};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

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
    pub spi0_cs1: u8,
    pub spi1_enabled: bool,
    pub spi1_miso: u8,
    pub spi1_sck: u8,
    pub spi1_mosi: u8,
    pub spi1_cs0: u8,
    pub spi1_cs1: u8,
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
            spi0_cs1: map.spi_chip_selects[0][1],
            spi1_enabled: map.is_enabled(Peripheral::Spi1),
            spi1_miso: map.spi_miso[1],
            spi1_sck: map.spi_sck[1],
            spi1_mosi: map.spi_mosi[1],
            spi1_cs0: map.spi_chip_selects[1][0],
            spi1_cs1: map.spi_chip_selects[1][1],
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
        Self {
            uart_tx: [map.uart0_tx, map.uart1_tx],
            uart_rx: [map.uart0_rx, map.uart1_rx],
            i2c_sda: [map.i2c0_sda, map.i2c1_sda],
            i2c_scl: [map.i2c0_scl, map.i2c1_scl],
            spi_miso: [map.spi0_miso, map.spi1_miso],
            spi_sck: [map.spi0_sck, map.spi1_sck],
            spi_mosi: [map.spi0_mosi, map.spi1_mosi],
            spi_chip_selects: [[map.spi0_cs0, map.spi0_cs1], [map.spi1_cs0, map.spi1_cs1]],
            enabled,
        }
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
}

impl ConfigStore {
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let boards = match fs::read(&path) {
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
                let maps: BTreeMap<String, PinMap> = match version {
                    1 => {
                        let config: LegacyConfigFile =
                            serde_json::from_value(value).map_err(invalid_data)?;
                        debug_assert_eq!(config.version, 1);
                        config
                            .boards
                            .into_iter()
                            .map(|(serial, map)| (serial, map.into()))
                            .collect()
                    }
                    2 => {
                        let config: ConfigFile =
                            serde_json::from_value(value).map_err(invalid_data)?;
                        debug_assert_eq!(config.version, 2);
                        config
                            .boards
                            .into_iter()
                            .map(|(serial, map)| (serial, map.into()))
                            .collect()
                    }
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("unsupported configuration version {version}"),
                        ));
                    }
                };
                maps.into_iter()
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
                    .collect::<io::Result<_>>()?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error),
        };
        Ok(Self { path, boards })
    }

    pub fn boards(&self) -> &BTreeMap<String, PinMap> {
        &self.boards
    }

    pub fn get(&self, serial: &str) -> Option<PinMap> {
        self.boards.get(serial).copied()
    }

    pub fn replace(&mut self, boards: BTreeMap<String, PinMap>) {
        self.boards = boards;
    }

    pub fn reload(&mut self) -> io::Result<()> {
        let replacement = Self::load(self.path.clone())?;
        self.boards = replacement.boards;
        Ok(())
    }

    pub fn set(&mut self, serial: String, map: PinMap) -> io::Result<()> {
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
        if let Err(error) = self.save() {
            if let Some(previous) = previous {
                self.boards.insert(serial.into(), previous);
            }
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
            let config = ConfigFile {
                version: 2,
                boards: self
                    .boards
                    .iter()
                    .map(|(serial, map)| (serial.clone(), (*map).into()))
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
            },
        };
        let encoded = serde_json::to_vec(&response).unwrap();
        assert_eq!(
            serde_json::from_slice::<ControlResponse>(&encoded).unwrap(),
            response
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
        assert_eq!(saved["version"], 2);
        assert_eq!(saved["boards"]["ABC"]["uart1_enabled"], false);
    }
}
