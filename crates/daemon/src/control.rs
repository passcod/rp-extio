use rp_extio_wire::{PinMap, PinSignal, PinValidationError, PinValidationKind};
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
    pub uart_tx: u8,
    pub uart_rx: u8,
    pub i2c_sda: u8,
    pub i2c_scl: u8,
    pub spi_miso: u8,
    pub spi_sck: u8,
    pub spi_mosi: u8,
    pub spi_cs0: u8,
    pub spi_cs1: u8,
}

impl From<PinMap> for PinMapConfig {
    fn from(map: PinMap) -> Self {
        Self {
            uart_tx: map.uart_tx,
            uart_rx: map.uart_rx,
            i2c_sda: map.i2c_sda,
            i2c_scl: map.i2c_scl,
            spi_miso: map.spi_miso,
            spi_sck: map.spi_sck,
            spi_mosi: map.spi_mosi,
            spi_cs0: map.spi_chip_selects[0],
            spi_cs1: map.spi_chip_selects[1],
        }
    }
}

impl From<PinMapConfig> for PinMap {
    fn from(map: PinMapConfig) -> Self {
        Self {
            uart_tx: map.uart_tx,
            uart_rx: map.uart_rx,
            i2c_sda: map.i2c_sda,
            i2c_scl: map.i2c_scl,
            spi_miso: map.spi_miso,
            spi_sck: map.spi_sck,
            spi_mosi: map.spi_mosi,
            spi_chip_selects: [map.spi_cs0, map.spi_cs1],
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
    pub i2c_bus: Option<i32>,
    pub spi_bus: Option<i32>,
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

pub struct ConfigStore {
    path: PathBuf,
    boards: BTreeMap<String, PinMap>,
}

impl ConfigStore {
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let boards = match fs::read(&path) {
            Ok(bytes) => {
                let config: ConfigFile = serde_json::from_slice(&bytes).map_err(invalid_data)?;
                if config.version != 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unsupported configuration version {}", config.version),
                    ));
                }
                config
                    .boards
                    .into_iter()
                    .map(|(serial, config)| {
                        let map = PinMap::from(config);
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
                version: 1,
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
        PinSignal::UartTx => "uart_tx",
        PinSignal::UartRx => "uart_rx",
        PinSignal::I2cSda => "i2c_sda",
        PinSignal::I2cScl => "i2c_scl",
        PinSignal::SpiMiso => "spi_miso",
        PinSignal::SpiSck => "spi_sck",
        PinSignal::SpiMosi => "spi_mosi",
        PinSignal::SpiCs0 => "spi_cs0",
        PinSignal::SpiCs1 => "spi_cs1",
    }
}

fn validation_kind_name(kind: PinValidationKind) -> &'static str {
    match kind {
        PinValidationKind::UnavailablePin => "unavailable_pin",
        PinValidationKind::InvalidRoute => "invalid_route",
        PinValidationKind::MismatchedPair => "mismatched_pair",
        PinValidationKind::DuplicatePin => "duplicate_pin",
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
            format!("{signal} does not form a matching pair with i2c_sda")
        }
        PinValidationKind::DuplicatePin => format!(
            "GP{} is assigned to both {signal} and {}",
            error.pin,
            signal_name(error.other.unwrap())
        ),
    }
}

fn invalid_data(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                i2c_bus: Some(12),
                spi_bus: Some(4),
                gpio_instance: Some(99),
            },
        };
        let encoded = serde_json::to_vec(&response).unwrap();
        assert_eq!(
            serde_json::from_slice::<ControlResponse>(&encoded).unwrap(),
            response
        );
    }
}
