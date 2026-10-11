use rp_extio_wire::{DEFAULT_PIN_MAP, Peripheral, PinMap};
use std::{env, fs, path::PathBuf};

fn configured(name: &str, default: u8) -> u8 {
    println!("cargo:rerun-if-env-changed={name}");
    env::var(name).map_or(default, |value| {
        value
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be a GPIO number, got {value:?}"))
    })
}

fn configured_alias(name: &str, legacy: &str, default: u8) -> u8 {
    println!("cargo:rerun-if-env-changed={name}");
    println!("cargo:rerun-if-env-changed={legacy}");
    env::var(name)
        .or_else(|_| env::var(legacy))
        .map_or(default, |value| {
            value
                .parse()
                .unwrap_or_else(|_| panic!("{name} must be a GPIO number, got {value:?}"))
        })
}

fn configured_enabled(name: &str, default: bool) -> bool {
    println!("cargo:rerun-if-env-changed={name}");
    env::var(name).map_or(default, |value| match value.as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => panic!("{name} must be a boolean, got {value:?}"),
    })
}

fn main() {
    println!("cargo:rerun-if-changed=memory.x");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", output.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", output.display());

    let mut pins = PinMap {
        uart_tx: [
            configured_alias(
                "RP_EXTIO_UART0_TX",
                "RP_EXTIO_UART_TX",
                DEFAULT_PIN_MAP.uart_tx[0],
            ),
            configured("RP_EXTIO_UART1_TX", DEFAULT_PIN_MAP.uart_tx[1]),
        ],
        uart_rx: [
            configured_alias(
                "RP_EXTIO_UART0_RX",
                "RP_EXTIO_UART_RX",
                DEFAULT_PIN_MAP.uart_rx[0],
            ),
            configured("RP_EXTIO_UART1_RX", DEFAULT_PIN_MAP.uart_rx[1]),
        ],
        i2c_sda: [
            configured_alias(
                "RP_EXTIO_I2C0_SDA",
                "RP_EXTIO_I2C_SDA",
                DEFAULT_PIN_MAP.i2c_sda[0],
            ),
            configured("RP_EXTIO_I2C1_SDA", DEFAULT_PIN_MAP.i2c_sda[1]),
        ],
        i2c_scl: [
            configured_alias(
                "RP_EXTIO_I2C0_SCL",
                "RP_EXTIO_I2C_SCL",
                DEFAULT_PIN_MAP.i2c_scl[0],
            ),
            configured("RP_EXTIO_I2C1_SCL", DEFAULT_PIN_MAP.i2c_scl[1]),
        ],
        spi_miso: [
            configured_alias(
                "RP_EXTIO_SPI0_MISO",
                "RP_EXTIO_SPI_MISO",
                DEFAULT_PIN_MAP.spi_miso[0],
            ),
            configured("RP_EXTIO_SPI1_MISO", DEFAULT_PIN_MAP.spi_miso[1]),
        ],
        spi_sck: [
            configured_alias(
                "RP_EXTIO_SPI0_SCK",
                "RP_EXTIO_SPI_SCK",
                DEFAULT_PIN_MAP.spi_sck[0],
            ),
            configured("RP_EXTIO_SPI1_SCK", DEFAULT_PIN_MAP.spi_sck[1]),
        ],
        spi_mosi: [
            configured_alias(
                "RP_EXTIO_SPI0_MOSI",
                "RP_EXTIO_SPI_MOSI",
                DEFAULT_PIN_MAP.spi_mosi[0],
            ),
            configured("RP_EXTIO_SPI1_MOSI", DEFAULT_PIN_MAP.spi_mosi[1]),
        ],
        spi_chip_selects: [
            [
                configured_alias(
                    "RP_EXTIO_SPI0_CS0",
                    "RP_EXTIO_SPI_CS0",
                    DEFAULT_PIN_MAP.spi_chip_selects[0][0],
                ),
                configured_alias(
                    "RP_EXTIO_SPI0_CS1",
                    "RP_EXTIO_SPI_CS1",
                    DEFAULT_PIN_MAP.spi_chip_selects[0][1],
                ),
            ],
            [
                configured("RP_EXTIO_SPI1_CS0", DEFAULT_PIN_MAP.spi_chip_selects[1][0]),
                configured("RP_EXTIO_SPI1_CS1", DEFAULT_PIN_MAP.spi_chip_selects[1][1]),
            ],
        ],
        enabled: Peripheral::ALL.into_iter().fold(0, |mask, peripheral| {
            let name = match peripheral {
                Peripheral::Uart0 => "RP_EXTIO_UART0_ENABLED",
                Peripheral::Uart1 => "RP_EXTIO_UART1_ENABLED",
                Peripheral::I2c0 => "RP_EXTIO_I2C0_ENABLED",
                Peripheral::I2c1 => "RP_EXTIO_I2C1_ENABLED",
                Peripheral::Spi0 => "RP_EXTIO_SPI0_ENABLED",
                Peripheral::Spi1 => "RP_EXTIO_SPI1_ENABLED",
            };
            if configured_enabled(name, DEFAULT_PIN_MAP.is_enabled(peripheral)) {
                mask | peripheral.mask()
            } else {
                mask
            }
        }),
    };
    pins.set_spi_cs1_enabled(
        0,
        configured_enabled(
            "RP_EXTIO_SPI0_CS1_ENABLED",
            DEFAULT_PIN_MAP.spi_cs1_enabled(0),
        ),
    );
    pins.set_spi_cs1_enabled(
        1,
        configured_enabled(
            "RP_EXTIO_SPI1_CS1_ENABLED",
            DEFAULT_PIN_MAP.spi_cs1_enabled(1),
        ),
    );
    pins.validate()
        .unwrap_or_else(|error| panic!("invalid default pin map: {error:?}"));
    fs::write(
        output.join("pin_config.rs"),
        format!(
            "const PIN_MAP: rp_extio_wire::PinMap = rp_extio_wire::PinMap {{\n\
             uart_tx: {:?}, uart_rx: {:?}, i2c_sda: {:?}, i2c_scl: {:?},\n\
             spi_miso: {:?}, spi_sck: {:?}, spi_mosi: {:?}, spi_chip_selects: {:?},\n\
             enabled: {},\n\
             }};\n",
            pins.uart_tx,
            pins.uart_rx,
            pins.i2c_sda,
            pins.i2c_scl,
            pins.spi_miso,
            pins.spi_sck,
            pins.spi_mosi,
            pins.spi_chip_selects,
            pins.enabled,
        ),
    )
    .unwrap();
    println!(
        "cargo:warning=rp-extio default enabled mask {:#04x}, UART {:?}/{:?}, I2C {:?}/{:?}, SPI MISO/SCK/MOSI {:?}/{:?}/{:?}, CS {:?}",
        pins.enabled,
        pins.uart_tx,
        pins.uart_rx,
        pins.i2c_sda,
        pins.i2c_scl,
        pins.spi_miso,
        pins.spi_sck,
        pins.spi_mosi,
        pins.spi_chip_selects,
    );
}
