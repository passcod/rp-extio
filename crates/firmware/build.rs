use std::{env, fs, path::PathBuf};

const HEADER_PINS: [u8; 26] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 26, 27, 28,
];

struct Pins {
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

fn configured(name: &str, default: u8) -> u8 {
    println!("cargo:rerun-if-env-changed={name}");
    env::var(name).map_or(default, |value| {
        value
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be a GPIO number, got {value:?}"))
    })
}

fn require(name: &str, pin: u8, valid: &[u8]) {
    assert!(
        HEADER_PINS.contains(&pin),
        "{name}=GP{pin} is not an externally usable Pico 2 header pin"
    );
    assert!(
        valid.contains(&pin),
        "{name}=GP{pin} is not a valid RP2350A route"
    );
}

fn pin_macro(name: &str, pin: u8) -> String {
    format!("macro_rules! {name} {{ ($p:ident) => {{ $p.PIN_{pin} }}; }}\n")
}

fn main() {
    println!("cargo:rerun-if-changed=memory.x");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", output.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", output.display());
    let pins = Pins {
        uart_tx: configured("RP_EXTIO_UART_TX", 0),
        uart_rx: configured("RP_EXTIO_UART_RX", 1),
        i2c_sda: configured("RP_EXTIO_I2C_SDA", 4),
        i2c_scl: configured("RP_EXTIO_I2C_SCL", 5),
        spi_miso: configured("RP_EXTIO_SPI_MISO", 16),
        spi_sck: configured("RP_EXTIO_SPI_SCK", 18),
        spi_mosi: configured("RP_EXTIO_SPI_MOSI", 19),
        spi_cs0: configured("RP_EXTIO_SPI_CS0", 17),
        spi_cs1: configured("RP_EXTIO_SPI_CS1", 21),
    };

    require(
        "RP_EXTIO_UART_TX",
        pins.uart_tx,
        &[0, 2, 12, 14, 16, 18, 28],
    );
    require("RP_EXTIO_UART_RX", pins.uart_rx, &[1, 3, 13, 15, 17, 19]);
    require("RP_EXTIO_I2C_SDA", pins.i2c_sda, &[0, 4, 8, 12, 16, 20]);
    require("RP_EXTIO_I2C_SCL", pins.i2c_scl, &[1, 5, 9, 13, 17, 21]);
    assert_eq!(
        pins.i2c_scl,
        pins.i2c_sda + 1,
        "I2C0 SDA and SCL must use a matching RP2350 pin pair"
    );
    require("RP_EXTIO_SPI_MISO", pins.spi_miso, &[0, 4, 16, 20]);
    require("RP_EXTIO_SPI_SCK", pins.spi_sck, &[2, 6, 18, 22]);
    require("RP_EXTIO_SPI_MOSI", pins.spi_mosi, &[3, 7, 19]);
    require("RP_EXTIO_SPI_CS0", pins.spi_cs0, &HEADER_PINS);
    require("RP_EXTIO_SPI_CS1", pins.spi_cs1, &HEADER_PINS);

    let assigned = [
        pins.uart_tx,
        pins.uart_rx,
        pins.i2c_sda,
        pins.i2c_scl,
        pins.spi_miso,
        pins.spi_sck,
        pins.spi_mosi,
        pins.spi_cs0,
        pins.spi_cs1,
    ];
    for (index, pin) in assigned.iter().enumerate() {
        assert!(
            !assigned[..index].contains(pin),
            "GP{pin} is assigned to more than one peripheral signal"
        );
    }
    let gpio: Vec<_> = HEADER_PINS
        .into_iter()
        .filter(|pin| !assigned.contains(pin))
        .collect();
    assert_eq!(gpio.len(), 17);

    let mut generated = String::new();
    generated.push_str(&format!(
        "const PIN_CONFIG: rp_extio_firmware::PinConfig = rp_extio_firmware::PinConfig {{\n\
         uart_tx: {}, uart_rx: {}, i2c_sda: {}, i2c_scl: {},\n\
         spi_miso: {}, spi_sck: {}, spi_mosi: {}, spi_chip_selects: [{}, {}],\n\
         gpio_pins: {:?},\n}};\n",
        pins.uart_tx,
        pins.uart_rx,
        pins.i2c_sda,
        pins.i2c_scl,
        pins.spi_miso,
        pins.spi_sck,
        pins.spi_mosi,
        pins.spi_cs0,
        pins.spi_cs1,
        gpio,
    ));
    for (name, pin) in [
        ("configured_uart_tx", pins.uart_tx),
        ("configured_uart_rx", pins.uart_rx),
        ("configured_i2c_sda", pins.i2c_sda),
        ("configured_i2c_scl", pins.i2c_scl),
        ("configured_spi_miso", pins.spi_miso),
        ("configured_spi_sck", pins.spi_sck),
        ("configured_spi_mosi", pins.spi_mosi),
        ("configured_spi_cs0", pins.spi_cs0),
        ("configured_spi_cs1", pins.spi_cs1),
    ] {
        generated.push_str(&pin_macro(name, pin));
    }
    generated.push_str("macro_rules! configured_gpio { ($p:ident) => { [\n");
    for pin in &gpio {
        generated.push_str(&format!("embassy_rp::gpio::Flex::new($p.PIN_{pin}),\n"));
    }
    generated.push_str("] }; }\n");

    fs::write(output.join("pin_config.rs"), generated).unwrap();
    println!(
        "cargo:warning=rp-extio pins: UART GP{}/GP{}, I2C GP{}/GP{}, SPI MISO/SCK/MOSI GP{}/GP{}/GP{}, CS GP{}/GP{}",
        pins.uart_tx,
        pins.uart_rx,
        pins.i2c_sda,
        pins.i2c_scl,
        pins.spi_miso,
        pins.spi_sck,
        pins.spi_mosi,
        pins.spi_cs0,
        pins.spi_cs1,
    );
}
