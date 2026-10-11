mod tui;

use clap::{Args, Parser, Subcommand};
use rp_extio_wire::{PinMap, PinSignal};
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
    #[arg(long, value_parser = parse_pin)]
    uart_tx: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    uart_rx: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    i2c_sda: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    i2c_scl: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    spi_miso: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    spi_sck: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    spi_mosi: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    spi_cs0: Option<u8>,
    #[arg(long, value_parser = parse_pin)]
    spi_cs1: Option<u8>,
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
                (PinSignal::UartTx, arguments.uart_tx),
                (PinSignal::UartRx, arguments.uart_rx),
                (PinSignal::I2cSda, arguments.i2c_sda),
                (PinSignal::I2cScl, arguments.i2c_scl),
                (PinSignal::SpiMiso, arguments.spi_miso),
                (PinSignal::SpiSck, arguments.spi_sck),
                (PinSignal::SpiMosi, arguments.spi_mosi),
                (PinSignal::SpiCs0, arguments.spi_cs0),
                (PinSignal::SpiCs1, arguments.spi_cs1),
            ];
            if edits.iter().all(|(_, pin)| pin.is_none()) {
                return Err("set requires at least one pin option".into());
            }
            for (signal, pin) in edits {
                if let Some(pin) = pin {
                    map.set_pin(signal, pin);
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
        "{}  {:12}  generation={}  i2c={} spi={}",
        board.serial,
        if board.connected {
            "connected"
        } else {
            "disconnected"
        },
        board
            .generation
            .map_or_else(|| "-".into(), |value| value.to_string()),
        board
            .i2c_bus
            .map_or_else(|| "-".into(), |value| value.to_string()),
        board
            .spi_bus
            .map_or_else(|| "-".into(), |value| value.to_string()),
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
    for signal in PinSignal::ALL {
        println!("  {:9} GP{}", signal_name(signal), map.pin(signal));
    }
    let gpio = map.gpio_pins().unwrap();
    print!("  gpio      ");
    for (index, pin) in gpio.iter().enumerate() {
        if index != 0 {
            print!(",");
        }
        print!("GP{pin}");
    }
    println!();
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
