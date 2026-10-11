mod tui;

use clap::{Args, Parser, Subcommand};
use rp_extio_wire::{Peripheral, PinMap, PinSignal};
use rp_extiod::control::{BoardInfo, ControlRequest, ControlResponse, PinMapConfig, signal_name};
use std::{
    error::Error,
    io::{BufRead, BufReader, Write},
    os::unix::{fs::FileTypeExt, net::UnixStream},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

const CONTROL_DEVICE: &str = "/dev/rp-extio-control";
const CONTROL_SYSFS_DEVICE: &str = "/sys/class/misc/rp-extio-control";

#[derive(Parser)]
#[command(version, about = "Configure rp-extio Pico 2 pin routing")]
struct Cli {
    #[arg(long, default_value = "/run/rp-extiod.sock", global = true)]
    socket: PathBuf,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Load the kernel module and ensure its control device exists
    Prepare {
        /// Insert this development module instead of loading an installed module
        #[arg(long)]
        module: Option<PathBuf>,
    },
    List,
    Show {
        serial: String,
    },
    Set(SetArgs),
    Default {
        serial: String,
    },
    Reload,
    Tui {
        serial: Option<String>,
    },
}

#[derive(Args)]
struct SetArgs {
    serial: String,
    #[arg(long, alias = "uart-tx", value_parser = parse_pin)]
    uart0_tx: Option<u8>,
    #[arg(long, alias = "uart-rx", value_parser = parse_pin)]
    uart0_rx: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    uart1_tx: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    uart1_rx: Option<u8>,
    #[arg(long, alias = "i2c-sda", value_parser = parse_pin)]
    i2c0_sda: Option<u8>,
    #[arg(long, alias = "i2c-scl", value_parser = parse_pin)]
    i2c0_scl: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    i2c1_sda: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    i2c1_scl: Option<u8>,
    #[arg(long, alias = "spi-miso", value_parser = parse_pin)]
    spi0_miso: Option<u8>,
    #[arg(long, alias = "spi-sck", value_parser = parse_pin)]
    spi0_sck: Option<u8>,
    #[arg(long, alias = "spi-mosi", value_parser = parse_pin)]
    spi0_mosi: Option<u8>,
    #[arg(long, alias = "spi-cs0", value_parser = parse_pin)]
    spi0_cs0: Option<u8>,
    #[arg(long, alias = "spi-cs1", value_parser = parse_pin)]
    spi0_cs1: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    spi1_miso: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    spi1_sck: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    spi1_mosi: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    spi1_cs0: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    spi1_cs1: Option<u8>,
    #[arg(long, conflicts_with = "disable_uart0")]
    enable_uart0: bool,
    #[arg(long, conflicts_with = "enable_uart0")]
    disable_uart0: bool,
    #[arg(long, conflicts_with = "disable_uart1")]
    enable_uart1: bool,
    #[arg(long, conflicts_with = "enable_uart1")]
    disable_uart1: bool,
    #[arg(long, conflicts_with = "disable_i2c0")]
    enable_i2c0: bool,
    #[arg(long, conflicts_with = "enable_i2c0")]
    disable_i2c0: bool,
    #[arg(long, conflicts_with = "disable_i2c1")]
    enable_i2c1: bool,
    #[arg(long, conflicts_with = "enable_i2c1")]
    disable_i2c1: bool,
    #[arg(long, conflicts_with = "disable_spi0")]
    enable_spi0: bool,
    #[arg(long, conflicts_with = "enable_spi0")]
    disable_spi0: bool,
    #[arg(long, conflicts_with = "disable_spi1")]
    enable_spi1: bool,
    #[arg(long, conflicts_with = "enable_spi1")]
    disable_spi1: bool,
}

pub(crate) struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    fn connect(path: &Path) -> Result<Self, Box<dyn Error>> {
        let writer = UnixStream::connect(path)?;
        let reader = BufReader::new(writer.try_clone()?);
        Ok(Self { reader, writer })
    }

    pub(crate) fn request(
        &mut self,
        request: &ControlRequest,
    ) -> Result<ControlResponse, Box<dyn Error>> {
        serde_json::to_writer(&mut self.writer, request)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err("daemon closed the control socket".into());
        }
        Ok(serde_json::from_str(&line)?)
    }

    pub(crate) fn boards(&mut self) -> Result<Vec<BoardInfo>, Box<dyn Error>> {
        match self.request(&ControlRequest::List)? {
            ControlResponse::Boards { boards } => Ok(boards),
            response => Err(response_error(response).into()),
        }
    }

    pub(crate) fn board(&mut self, serial: &str) -> Result<BoardInfo, Box<dyn Error>> {
        match self.request(&ControlRequest::Show {
            serial: serial.into(),
        })? {
            ControlResponse::Board { board } => Ok(board),
            response => Err(response_error(response).into()),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    if let Command::Prepare { module } = &cli.command {
        prepare(module.as_deref())?;
        if cli.json {
            println!(r#"{{"result":"ready","control":"{CONTROL_DEVICE}"}}"#);
        } else {
            println!("ready: {CONTROL_DEVICE}");
        }
        return Ok(());
    }
    let mut client = Client::connect(&cli.socket)?;
    match cli.command {
        Command::Prepare { .. } => unreachable!(),
        Command::List => output(client.request(&ControlRequest::List)?, cli.json),
        Command::Show { serial } => {
            output(client.request(&ControlRequest::Show { serial })?, cli.json)
        }
        Command::Set(arguments) => {
            let board = client.board(&arguments.serial)?;
            let generation = board.generation.ok_or("board is disconnected")?;
            let active = board.active.ok_or("board has no active pin map")?;
            let mut map = PinMap::from(active);
            let edits = [
                (PinSignal::Uart0Tx, arguments.uart0_tx),
                (PinSignal::Uart0Rx, arguments.uart0_rx),
                (PinSignal::Uart1Tx, arguments.uart1_tx),
                (PinSignal::Uart1Rx, arguments.uart1_rx),
                (PinSignal::I2c0Sda, arguments.i2c0_sda),
                (PinSignal::I2c0Scl, arguments.i2c0_scl),
                (PinSignal::I2c1Sda, arguments.i2c1_sda),
                (PinSignal::I2c1Scl, arguments.i2c1_scl),
                (PinSignal::Spi0Miso, arguments.spi0_miso),
                (PinSignal::Spi0Sck, arguments.spi0_sck),
                (PinSignal::Spi0Mosi, arguments.spi0_mosi),
                (PinSignal::Spi0Cs0, arguments.spi0_cs0),
                (PinSignal::Spi0Cs1, arguments.spi0_cs1),
                (PinSignal::Spi1Miso, arguments.spi1_miso),
                (PinSignal::Spi1Sck, arguments.spi1_sck),
                (PinSignal::Spi1Mosi, arguments.spi1_mosi),
                (PinSignal::Spi1Cs0, arguments.spi1_cs0),
                (PinSignal::Spi1Cs1, arguments.spi1_cs1),
            ];
            let enable_edits = [
                (
                    Peripheral::Uart0,
                    arguments.enable_uart0,
                    arguments.disable_uart0,
                ),
                (
                    Peripheral::Uart1,
                    arguments.enable_uart1,
                    arguments.disable_uart1,
                ),
                (
                    Peripheral::I2c0,
                    arguments.enable_i2c0,
                    arguments.disable_i2c0,
                ),
                (
                    Peripheral::I2c1,
                    arguments.enable_i2c1,
                    arguments.disable_i2c1,
                ),
                (
                    Peripheral::Spi0,
                    arguments.enable_spi0,
                    arguments.disable_spi0,
                ),
                (
                    Peripheral::Spi1,
                    arguments.enable_spi1,
                    arguments.disable_spi1,
                ),
            ];
            if edits.iter().all(|(_, pin)| pin.is_none())
                && enable_edits
                    .iter()
                    .all(|(_, enable, disable)| !enable && !disable)
            {
                return Err("set requires at least one route or enable/disable option".into());
            }
            for (signal, pin) in edits {
                if let Some(pin) = pin {
                    map.set_pin(signal, pin);
                }
            }
            for (peripheral, enable, disable) in enable_edits {
                if enable || disable {
                    map.set_enabled(peripheral, enable);
                }
            }
            map.validate()
                .map_err(rp_extiod::control::format_validation)?;
            output(
                client.request(&ControlRequest::Set {
                    serial: arguments.serial,
                    generation,
                    map: map.into(),
                })?,
                cli.json,
            )
        }
        Command::Default { serial } => {
            let board = client.board(&serial)?;
            output(
                client.request(&ControlRequest::Default {
                    serial,
                    generation: board.generation.ok_or("board is disconnected")?,
                })?,
                cli.json,
            )
        }
        Command::Reload => output(client.request(&ControlRequest::Reload)?, cli.json),
        Command::Tui { serial } => tui::run(&mut client, serial.as_deref()),
    }
}

fn prepare(module: Option<&Path>) -> Result<(), Box<dyn Error>> {
    let control = Path::new(CONTROL_DEVICE);
    if is_character_device(control)? {
        return Ok(());
    }

    let sysfs = Path::new(CONTROL_SYSFS_DEVICE);
    if !sysfs.exists() {
        match module {
            Some(path) => run("insmod", [path.as_os_str()])?,
            None => run("modprobe", [std::ffi::OsStr::new("rp_extio")])?,
        }
    }
    if !sysfs.exists() {
        return Err(format!("rp_extio loaded without creating {CONTROL_SYSFS_DEVICE}").into());
    }

    if !is_character_device(control)? {
        run(
            "udevadm",
            [
                std::ffi::OsStr::new("trigger"),
                std::ffi::OsStr::new("--action=add"),
                std::ffi::OsStr::new("--settle"),
                sysfs.as_os_str(),
            ],
        )?;
    }
    if !is_character_device(control)? {
        return Err(
            format!("{CONTROL_DEVICE} is not a character device after udev settled").into(),
        );
    }
    Ok(())
}

fn is_character_device(path: &Path) -> std::io::Result<bool> {
    match path.symlink_metadata() {
        Ok(metadata) => Ok(metadata.file_type().is_char_device()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn run<I, S>(program: &str, arguments: I) -> Result<(), Box<dyn Error>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let status = ProcessCommand::new(program).args(arguments).status()?;
    if !status.success() {
        return Err(format!("{program} exited with {status}").into());
    }
    Ok(())
}

fn output(response: ControlResponse, json: bool) -> Result<(), Box<dyn Error>> {
    if json {
        println!("{}", serde_json::to_string(&response)?);
    } else {
        match &response {
            ControlResponse::Boards { boards } => {
                for board in boards {
                    println!("{}", board_summary(board));
                }
            }
            ControlResponse::Board { board } | ControlResponse::Applied { board } => {
                print_board(board)
            }
            ControlResponse::Ok => println!("ok"),
            ControlResponse::Error { message, .. } => eprintln!("{message}"),
        }
    }
    if matches!(response, ControlResponse::Error { .. }) {
        std::process::exit(1);
    }
    Ok(())
}

fn board_summary(board: &BoardInfo) -> String {
    format!(
        "{}  {:12}  generation={}  enabled={} i2c={} spi={}",
        board.serial,
        if board.connected {
            "connected"
        } else {
            "disconnected"
        },
        board
            .generation
            .map_or_else(|| "-".into(), |value| value.to_string()),
        board.active.or(board.desired).map_or_else(
            || "-".into(),
            |config| {
                let map = PinMap::from(config);
                Peripheral::ALL
                    .into_iter()
                    .filter(|&peripheral| map.is_enabled(peripheral))
                    .map(peripheral_name)
                    .collect::<Vec<_>>()
                    .join(",")
            }
        ),
        bus_numbers(board.i2c_buses),
        bus_numbers(board.spi_buses),
    )
}

fn print_board(board: &BoardInfo) {
    println!("{}", board_summary(board));
    if let Some(map) = board.active {
        print_map("active", map);
    }
    if let Some(map) = board.desired
        && Some(map) != board.active
    {
        print_map("desired", map);
    }
}

fn print_map(label: &str, config: PinMapConfig) {
    let map = PinMap::from(config);
    println!("{label}:");
    for peripheral in Peripheral::ALL {
        println!(
            "  {:5} {}",
            peripheral_name(peripheral),
            if map.is_enabled(peripheral) {
                "enabled"
            } else {
                "disabled"
            }
        );
        for &signal in peripheral.signals() {
            println!("    {:10} GP{}", signal_name(signal), map.pin(signal));
        }
    }
    let gpio = map.gpio_pins().unwrap();
    print!("  gpio      ");
    for (index, pin) in gpio.as_slice().iter().enumerate() {
        if index != 0 {
            print!(",");
        }
        print!("GP{pin}");
    }
    println!();
}

fn bus_numbers(buses: [Option<i32>; 2]) -> String {
    buses
        .into_iter()
        .enumerate()
        .map(|(instance, bus)| match bus {
            Some(bus) => format!("{instance}:{bus}"),
            None => format!("{instance}:-"),
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn peripheral_name(peripheral: Peripheral) -> &'static str {
    match peripheral {
        Peripheral::Uart0 => "uart0",
        Peripheral::Uart1 => "uart1",
        Peripheral::I2c0 => "i2c0",
        Peripheral::I2c1 => "i2c1",
        Peripheral::Spi0 => "spi0",
        Peripheral::Spi1 => "spi1",
    }
}

fn response_error(response: ControlResponse) -> String {
    match response {
        ControlResponse::Error { message, .. } => message,
        _ => "unexpected daemon response".into(),
    }
}

fn parse_pin(value: &str) -> Result<u8, String> {
    value
        .strip_prefix("GP")
        .or_else(|| value.strip_prefix("gp"))
        .unwrap_or(value)
        .parse()
        .map_err(|_| format!("expected a GPIO number, got {value:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_accepts_instance_routes_legacy_aliases_and_enable_flags() {
        let cli = Cli::try_parse_from([
            "rp-extioctl",
            "set",
            "ABC",
            "--uart-tx",
            "GP2",
            "--uart1-rx",
            "7",
            "--disable-uart0",
            "--enable-uart1",
        ])
        .unwrap();
        let Command::Set(arguments) = cli.command else {
            panic!("expected set command");
        };
        assert_eq!(arguments.uart0_tx, Some(2));
        assert_eq!(arguments.uart1_rx, Some(7));
        assert!(arguments.disable_uart0);
        assert!(arguments.enable_uart1);
    }

    #[test]
    fn set_rejects_conflicting_enable_flags() {
        assert!(
            Cli::try_parse_from([
                "rp-extioctl",
                "set",
                "ABC",
                "--enable-spi1",
                "--disable-spi1",
            ])
            .is_err()
        );
    }

    #[test]
    fn list_summary_identifies_enabled_hardware_instances() {
        let summary = board_summary(&BoardInfo {
            serial: "ABC".into(),
            connected: true,
            active: Some(rp_extio_wire::DEFAULT_PIN_MAP.into()),
            desired: None,
            generation: Some(3),
            i2c_buses: [Some(7), None],
            spi_buses: [Some(2), None],
            gpio_instance: Some(9),
        });
        assert!(summary.contains("enabled=uart0,i2c0,spi0"));
        assert!(summary.contains("i2c=0:7,1:-"));
        assert!(summary.contains("spi=0:2,1:-"));
    }
}
