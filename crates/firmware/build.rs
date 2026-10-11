use rp_extio_wire::{DEFAULT_PIN_MAP, PinMap};
use std::{env, fs, path::PathBuf};

fn configured(name: &str, default: u8) -> u8 {
    println!("cargo:rerun-if-env-changed={name}");
    env::var(name).map_or(default, |value| {
        value
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be a GPIO number, got {value:?}"))
    })
}

fn main() {
    println!("cargo:rerun-if-changed=memory.x");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", output.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", output.display());

    let pins = PinMap {
        uart_tx: configured("RP_EXTIO_UART_TX", DEFAULT_PIN_MAP.uart_tx),
        uart_rx: configured("RP_EXTIO_UART_RX", DEFAULT_PIN_MAP.uart_rx),
        i2c_sda: configured("RP_EXTIO_I2C_SDA", DEFAULT_PIN_MAP.i2c_sda),
        i2c_scl: configured("RP_EXTIO_I2C_SCL", DEFAULT_PIN_MAP.i2c_scl),
        spi_miso: configured("RP_EXTIO_SPI_MISO", DEFAULT_PIN_MAP.spi_miso),
        spi_sck: configured("RP_EXTIO_SPI_SCK", DEFAULT_PIN_MAP.spi_sck),
        spi_mosi: configured("RP_EXTIO_SPI_MOSI", DEFAULT_PIN_MAP.spi_mosi),
        spi_chip_selects: [
            configured("RP_EXTIO_SPI_CS0", DEFAULT_PIN_MAP.spi_chip_selects[0]),
            configured("RP_EXTIO_SPI_CS1", DEFAULT_PIN_MAP.spi_chip_selects[1]),
        ],
    };
    pins.validate()
        .unwrap_or_else(|error| panic!("invalid default pin map: {error:?}"));
    fs::write(
        output.join("pin_config.rs"),
        format!(
            "const PIN_MAP: rp_extio_wire::PinMap = rp_extio_wire::PinMap {{\n\
             uart_tx: {}, uart_rx: {}, i2c_sda: {}, i2c_scl: {},\n\
             spi_miso: {}, spi_sck: {}, spi_mosi: {}, spi_chip_selects: [{}, {}],\n\
             }};\n",
            pins.uart_tx,
            pins.uart_rx,
            pins.i2c_sda,
            pins.i2c_scl,
            pins.spi_miso,
            pins.spi_sck,
            pins.spi_mosi,
            pins.spi_chip_selects[0],
            pins.spi_chip_selects[1],
        ),
    )
    .unwrap();
    println!(
        "cargo:warning=rp-extio default pins: UART GP{}/GP{}, I2C GP{}/GP{}, SPI MISO/SCK/MOSI GP{}/GP{}/GP{}, CS GP{}/GP{}",
        pins.uart_tx,
        pins.uart_rx,
        pins.i2c_sda,
        pins.i2c_scl,
        pins.spi_miso,
        pins.spi_sck,
        pins.spi_mosi,
        pins.spi_chip_selects[0],
        pins.spi_chip_selects[1],
    );
}
