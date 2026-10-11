mod tui;

use clap::{Args, Parser, Subcommand, ValueEnum};
use rp_extio_wire::{ClockSource, Peripheral, PinMap, PinSignal};
use rp_extiod::control::{
    BoardInfo, ControlRequest, ControlResponse, GpioBias, GpioInfo, GpioStartup, PinMapConfig,
    PwmInfo, signal_name,
};
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
    /// Read one ADC channel directly through the daemon
    Adc {
        serial: String,
        channel: u8,
    },
    /// Query or drive a routed PWM channel
    Pwm {
        #[command(subcommand)]
        command: PwmCommand,
    },
    /// Inspect or configure pins currently available as GPIO
    Gpio {
        #[command(subcommand)]
        command: GpioCommand,
    },
    /// Drive an LED and retain the output until changed or disconnected
    Led {
        serial: String,
        #[arg(value_parser = parse_pin)]
        pin: u8,
        state: OnOff,
        #[arg(long)]
        active_low: bool,
    },
    /// Configure a button input and wait for a level or transition
    Button {
        serial: String,
        #[arg(value_parser = parse_pin)]
        pin: u8,
        #[arg(long, value_enum, default_value_t = ButtonWait::Pressed)]
        wait: ButtonWait,
        #[arg(long, value_enum, default_value_t = BiasArg::PullUp)]
        pull: BiasArg,
        /// Override the saved button configuration and interpret low as pressed
        #[arg(long, conflicts_with = "active_high")]
        active_low: bool,
        /// Override the saved button configuration and interpret high as pressed
        #[arg(long, conflicts_with = "active_low")]
        active_high: bool,
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
}

#[derive(Subcommand)]
enum PwmCommand {
    Get {
        serial: String,
        channel: u8,
    },
    Set {
        serial: String,
        channel: u8,
        #[arg(long)]
        frequency_hz: u64,
        #[arg(long, default_value_t = 50.0)]
        duty_percent: f64,
        #[arg(long)]
        inverted: bool,
        /// Save and reapply this PWM state whenever the daemon connects
        #[arg(long)]
        persist: bool,
    },
    Disable {
        serial: String,
        channel: u8,
        /// Save and reapply the disabled state whenever the daemon connects
        #[arg(long)]
        persist: bool,
    },
    /// Forget a saved PWM startup state without changing the live channel
    Forget {
        serial: String,
        channel: u8,
    },
}

#[derive(Subcommand)]
enum GpioCommand {
    Snapshot {
        serial: String,
    },
    Set {
        serial: String,
        #[arg(value_parser = parse_pin)]
        pin: u8,
        level: LevelArg,
        /// Save and reapply this output level whenever the daemon connects
        #[arg(long)]
        persist: bool,
    },
    Input {
        serial: String,
        #[arg(value_parser = parse_pin)]
        pin: u8,
        #[arg(long, value_enum, default_value_t = BiasArg::None)]
        pull: BiasArg,
        /// Save and reapply this input bias whenever the daemon connects
        #[arg(long)]
        persist: bool,
    },
    /// Forget a saved startup state without changing the live pin
    Forget {
        serial: String,
        #[arg(value_parser = parse_pin)]
        pin: u8,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum LevelArg {
    Low,
    High,
}

#[derive(Clone, Copy, ValueEnum)]
enum OnOff {
    Off,
    On,
}

#[derive(Clone, Copy, ValueEnum)]
enum BiasArg {
    None,
    PullUp,
    PullDown,
}

impl From<BiasArg> for GpioBias {
    fn from(bias: BiasArg) -> Self {
        match bias {
            BiasArg::None => Self::None,
            BiasArg::PullUp => Self::PullUp,
            BiasArg::PullDown => Self::PullDown,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, ValueEnum)]
enum ButtonWait {
    Pressed,
    Released,
    Rising,
    Falling,
    Either,
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
    #[arg(long, conflicts_with = "disable_spi0_cs1")]
    enable_spi0_cs1: bool,
    #[arg(long, conflicts_with = "enable_spi0_cs1")]
    disable_spi0_cs1: bool,
    #[arg(long, conflicts_with = "disable_spi1_cs1")]
    enable_spi1_cs1: bool,
    #[arg(long, conflicts_with = "enable_spi1_cs1")]
    disable_spi1_cs1: bool,
    /// Route any signal by name, for example pwm1a=GP18 or adc0=GP26
    #[arg(long = "route", value_parser = parse_route)]
    routes: Vec<(PinSignal, u8)>,
    /// Enable any signal (serial groups enable together)
    #[arg(long = "enable", value_parser = parse_signal)]
    enable_signals: Vec<PinSignal>,
    /// Disable any signal (serial groups disable together)
    #[arg(long = "disable", value_parser = parse_signal)]
    disable_signals: Vec<PinSignal>,
    /// Select a clock source, for example 0=system or 1=crystal
    #[arg(long = "clock-source", value_parser = parse_clock_source)]
    clock_sources: Vec<(usize, ClockSource)>,
    /// Set a clock target in hertz, for example 0=1000000
    #[arg(long = "clock-hz", value_parser = parse_clock_hz)]
    clock_hz: Vec<(usize, u32)>,
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
                && !arguments.enable_spi0_cs1
                && !arguments.disable_spi0_cs1
                && !arguments.enable_spi1_cs1
                && !arguments.disable_spi1_cs1
                && arguments.routes.is_empty()
                && arguments.enable_signals.is_empty()
                && arguments.disable_signals.is_empty()
                && arguments.clock_sources.is_empty()
                && arguments.clock_hz.is_empty()
            {
                return Err("set requires at least one route or enable/disable option".into());
            }
            for (signal, pin) in edits {
                if let Some(pin) = pin {
                    map.set_pin(signal, pin);
                }
            }
            for (signal, pin) in arguments.routes {
                map.set_pin(signal, pin);
            }
            for (peripheral, enable, disable) in enable_edits {
                if enable || disable {
                    map.set_enabled(peripheral, enable);
                }
            }
            for (bus, enable, disable) in [
                (0, arguments.enable_spi0_cs1, arguments.disable_spi0_cs1),
                (1, arguments.enable_spi1_cs1, arguments.disable_spi1_cs1),
            ] {
                if enable || disable {
                    map.set_spi_cs1_enabled(bus, enable);
                }
            }
            for signal in arguments.enable_signals {
                if arguments.disable_signals.contains(&signal) {
                    return Err(format!(
                        "{} cannot be both enabled and disabled",
                        signal_name(signal)
                    )
                    .into());
                }
                map.set_signal_enabled(signal, true);
            }
            for signal in arguments.disable_signals {
                map.set_signal_enabled(signal, false);
            }
            for (output, source) in arguments.clock_sources {
                map.clock_sources[output] = source;
            }
            for (output, frequency) in arguments.clock_hz {
                map.clock_hz[output] = frequency;
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
        Command::Adc { serial, channel } => output(
            client.request(&ControlRequest::AdcRead { serial, channel })?,
            cli.json,
        ),
        Command::Pwm { command } => {
            let request = match command {
                PwmCommand::Get { serial, channel } => ControlRequest::PwmGet { serial, channel },
                PwmCommand::Set {
                    serial,
                    channel,
                    frequency_hz,
                    duty_percent,
                    inverted,
                    persist,
                } => {
                    if frequency_hz == 0 {
                        return Err("PWM frequency must be greater than zero".into());
                    }
                    if !(0.0..=100.0).contains(&duty_percent) || !duty_percent.is_finite() {
                        return Err("PWM duty percent must be between 0 and 100".into());
                    }
                    let period_ns = 1_000_000_000_u64
                        .checked_add(frequency_hz / 2)
                        .and_then(|value| value.checked_div(frequency_hz))
                        .ok_or("PWM frequency is out of range")?;
                    if period_ns == 0 {
                        return Err("PWM frequency is too high".into());
                    }
                    let pwm = PwmInfo {
                        channel,
                        enabled: true,
                        inverted,
                        period_ns,
                        duty_ns: (period_ns as f64 * duty_percent / 100.0).round() as u64,
                    };
                    if persist {
                        ControlRequest::PwmStartupSet { serial, pwm }
                    } else {
                        ControlRequest::PwmApply { serial, pwm }
                    }
                }
                PwmCommand::Disable {
                    serial,
                    channel,
                    persist,
                } => {
                    let pwm = PwmInfo {
                        channel,
                        enabled: false,
                        inverted: false,
                        period_ns: 0,
                        duty_ns: 0,
                    };
                    if persist {
                        ControlRequest::PwmStartupSet { serial, pwm }
                    } else {
                        ControlRequest::PwmApply { serial, pwm }
                    }
                }
                PwmCommand::Forget { serial, channel } => {
                    ControlRequest::PwmStartupClear { serial, channel }
                }
            };
            output(client.request(&request)?, cli.json)
        }
        Command::Gpio { command } => {
            let request = match command {
                GpioCommand::Snapshot { serial } => ControlRequest::GpioSnapshot { serial },
                GpioCommand::Set {
                    serial,
                    pin,
                    level,
                    persist,
                } => {
                    let value = matches!(level, LevelArg::High);
                    if persist {
                        ControlRequest::GpioStartupSet {
                            serial,
                            startup: GpioStartup::Output { pin, value },
                        }
                    } else {
                        ControlRequest::GpioOutput { serial, pin, value }
                    }
                }
                GpioCommand::Input {
                    serial,
                    pin,
                    pull,
                    persist,
                } => {
                    let bias = pull.into();
                    if persist {
                        ControlRequest::GpioStartupSet {
                            serial,
                            startup: GpioStartup::Input { pin, bias },
                        }
                    } else {
                        ControlRequest::GpioInput { serial, pin, bias }
                    }
                }
                GpioCommand::Forget { serial, pin } => {
                    ControlRequest::GpioStartupClear { serial, pin }
                }
            };
            output(client.request(&request)?, cli.json)
        }
        Command::Led {
            serial,
            pin,
            state,
            active_low,
        } => {
            let on = matches!(state, OnOff::On);
            output(
                client.request(&ControlRequest::GpioOutput {
                    serial,
                    pin,
                    value: on != active_low,
                })?,
                cli.json,
            )
        }
        Command::Button {
            serial,
            pin,
            wait,
            pull,
            active_low,
            active_high,
            timeout_ms,
        } => {
            let active_low =
                resolve_button_polarity(&mut client, &serial, pin, active_low, active_high)?;
            wait_for_button(
                &mut client,
                &serial,
                pin,
                wait,
                pull.into(),
                active_low,
                timeout_ms,
                cli.json,
            )
        }
    }
}

fn resolve_button_polarity(
    client: &mut Client,
    serial: &str,
    pin: u8,
    active_low: bool,
    active_high: bool,
) -> Result<bool, Box<dyn Error>> {
    if active_low || active_high {
        return Ok(active_low);
    }
    let board = match client.request(&ControlRequest::Show {
        serial: serial.into(),
    })? {
        ControlResponse::Board { board } => board,
        response => return Err(response_error(response).into()),
    };
    board
        .buttons
        .iter()
        .find(|button| button.pin == pin)
        .map(|button| button.active_low)
        .ok_or_else(|| {
            format!(
                "GP{pin} has no saved button configuration; configure it in the TUI or pass --active-low/--active-high"
            )
            .into()
        })
}

#[allow(clippy::too_many_arguments)]
fn wait_for_button(
    client: &mut Client,
    serial: &str,
    pin: u8,
    wait: ButtonWait,
    bias: GpioBias,
    active_low: bool,
    timeout_ms: Option<u64>,
    json: bool,
) -> Result<(), Box<dyn Error>> {
    match client.request(&ControlRequest::GpioInput {
        serial: serial.into(),
        pin,
        bias,
    })? {
        ControlResponse::Gpio { .. } => {}
        response => return Err(response_error(response).into()),
    }
    let started = std::time::Instant::now();
    let mut previous = None;
    loop {
        let gpio = snapshot_pin(client, serial, pin)?;
        let logical = gpio.level != active_low;
        let triggered = match (wait, previous) {
            (ButtonWait::Pressed, _) => logical,
            (ButtonWait::Released, _) => !logical,
            (ButtonWait::Rising, Some(false)) => gpio.level,
            (ButtonWait::Falling, Some(true)) => !gpio.level,
            (ButtonWait::Either, Some(previous)) => previous != gpio.level,
            _ => false,
        };
        if triggered {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "result": "button",
                        "pin": pin,
                        "level": gpio.level,
                        "pressed": logical,
                    })
                );
            } else {
                println!(
                    "GP{pin}: {} ({})",
                    if logical { "pressed" } else { "released" },
                    if gpio.level { "high" } else { "low" }
                );
            }
            return Ok(());
        }
        previous = Some(gpio.level);
        if timeout_ms.is_some_and(|timeout| started.elapsed().as_millis() >= timeout.into()) {
            return Err(format!("timed out waiting for GP{pin}").into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn snapshot_pin(client: &mut Client, serial: &str, pin: u8) -> Result<GpioInfo, Box<dyn Error>> {
    match client.request(&ControlRequest::GpioSnapshot {
        serial: serial.into(),
    })? {
        ControlResponse::GpioSnapshot { gpio } => gpio
            .into_iter()
            .find(|gpio| gpio.pin == pin)
            .ok_or_else(|| format!("GP{pin} is not configured as GPIO").into()),
        response => Err(response_error(response).into()),
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
            ControlResponse::Adc {
                channel,
                raw,
                reference_mv,
                resolution_bits,
            } => {
                let millivolts = adc_millivolts(*raw, *reference_mv, *resolution_bits);
                println!("ADC{channel}: {raw} raw, {millivolts} mV");
            }
            ControlResponse::Pwm { pwm } => {
                println!(
                    "PWM{}: {} period={}ns duty={}ns polarity={}",
                    pwm.channel,
                    if pwm.enabled { "enabled" } else { "disabled" },
                    pwm.period_ns,
                    pwm.duty_ns,
                    if pwm.inverted { "inverted" } else { "normal" }
                );
            }
            ControlResponse::GpioSnapshot { gpio } => {
                for gpio in gpio {
                    println!(
                        "GP{:<2} {:<6} {} bias={:?}",
                        gpio.pin,
                        if gpio.output { "output" } else { "input" },
                        if gpio.level { "high" } else { "low" },
                        gpio.bias
                    );
                }
            }
            ControlResponse::Gpio { gpio } => println!(
                "GP{}: {} {} bias={:?}",
                gpio.pin,
                if gpio.output { "output" } else { "input" },
                if gpio.level { "high" } else { "low" },
                gpio.bias
            ),
            ControlResponse::Error { message, .. } => eprintln!("{message}"),
        }
    }
    if matches!(response, ControlResponse::Error { .. }) {
        std::process::exit(1);
    }
    Ok(())
}

fn adc_millivolts(raw: u16, reference_mv: u16, resolution_bits: u8) -> u32 {
    u32::from(raw) * u32::from(reference_mv) / (1_u32 << resolution_bits)
}

fn board_summary(board: &BoardInfo) -> String {
    format!(
        "{}  {:12}  generation={}  enabled={} i2c={} spi={} iio={} pwm={}",
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
                    .chain(
                        PinSignal::ALL
                            .into_iter()
                            .filter(|signal| signal.serial_peripheral().is_none())
                            .filter(|&signal| map.is_signal_enabled(signal))
                            .map(signal_name),
                    )
                    .collect::<Vec<_>>()
                    .join(",")
            }
        ),
        bus_numbers(board.i2c_buses),
        bus_numbers(board.spi_buses),
        board
            .iio_device
            .map_or_else(|| "-".into(), |number| format!("iio:device{number}")),
        board
            .pwm_chip
            .map_or_else(|| "-".into(), |number| format!("pwmchip{number}")),
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
    if !board.gpio_startup.is_empty() {
        println!("GPIO startup:");
        for startup in &board.gpio_startup {
            match startup {
                GpioStartup::Output { pin, value } => println!(
                    "  GP{pin:<2} output {}",
                    if *value { "high" } else { "low" }
                ),
                GpioStartup::Input { pin, bias } => {
                    println!("  GP{pin:<2} input bias={}", gpio_bias_name(*bias))
                }
            }
        }
    }
    if !board.pwm_startup.is_empty() {
        println!("PWM startup:");
        for pwm in &board.pwm_startup {
            println!(
                "  PWM{} {} period={}ns duty={}ns polarity={}",
                pwm.channel,
                if pwm.enabled { "enabled" } else { "disabled" },
                pwm.period_ns,
                pwm.duty_ns,
                if pwm.inverted { "inverted" } else { "normal" }
            );
        }
    }
    if !board.buttons.is_empty() {
        println!("Buttons:");
        for button in &board.buttons {
            println!(
                "  GP{:<2} active {}",
                button.pin,
                if button.active_low { "low" } else { "high" }
            );
        }
    }
}

fn gpio_bias_name(bias: GpioBias) -> &'static str {
    match bias {
        GpioBias::None => "none",
        GpioBias::PullUp => "pull-up",
        GpioBias::PullDown => "pull-down",
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
            let state = match signal {
                PinSignal::Spi0Cs1 => {
                    if map.spi_cs1_enabled(0) {
                        " enabled"
                    } else {
                        " disabled"
                    }
                }
                PinSignal::Spi1Cs1 => {
                    if map.spi_cs1_enabled(1) {
                        " enabled"
                    } else {
                        " disabled"
                    }
                }
                _ => "",
            };
            println!(
                "    {:10} GP{}{}",
                signal_name(signal),
                map.pin(signal),
                state
            );
        }
    }
    for signal in PinSignal::ALL
        .into_iter()
        .filter(|signal| signal.serial_peripheral().is_none())
    {
        let details = signal.clock_output().map_or_else(String::new, |output| {
            format!(
                " source={:?} target={}Hz",
                map.clock_sources[output], map.clock_hz[output]
            )
        });
        println!(
            "  {:10} {:8} GP{}{}",
            signal_name(signal),
            if map.is_signal_enabled(signal) {
                "enabled"
            } else {
                "disabled"
            },
            map.pin(signal),
            details
        );
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

fn parse_signal(value: &str) -> Result<PinSignal, String> {
    let normalized = value.to_ascii_lowercase().replace('-', "_");
    PinSignal::ALL
        .into_iter()
        .find(|&signal| signal_name(signal) == normalized)
        .ok_or_else(|| format!("unknown pin signal {value:?}"))
}

fn parse_route(value: &str) -> Result<(PinSignal, u8), String> {
    let (signal, pin) = value
        .split_once('=')
        .ok_or_else(|| format!("expected SIGNAL=PIN, got {value:?}"))?;
    Ok((parse_signal(signal)?, parse_pin(pin)?))
}

fn parse_clock_output(value: &str) -> Result<usize, String> {
    match value
        .strip_prefix("clock")
        .or_else(|| value.strip_prefix("CLOCK"))
        .unwrap_or(value)
    {
        "0" => Ok(0),
        "1" => Ok(1),
        _ => Err(format!("expected clock output 0 or 1, got {value:?}")),
    }
}

fn parse_clock_source(value: &str) -> Result<(usize, ClockSource), String> {
    let (output, source) = value
        .split_once('=')
        .ok_or_else(|| format!("expected OUTPUT=SOURCE, got {value:?}"))?;
    let source = match source.to_ascii_lowercase().replace('-', "_").as_str() {
        "system" | "sys" => ClockSource::System,
        "reference" | "ref" => ClockSource::Reference,
        "usb" => ClockSource::Usb,
        "adc" => ClockSource::Adc,
        "crystal" | "xosc" => ClockSource::Crystal,
        "ring_oscillator" | "rosc" => ClockSource::RingOscillator,
        "low_power_oscillator" | "lposc" => ClockSource::LowPowerOscillator,
        _ => return Err(format!("unknown clock source {source:?}")),
    };
    Ok((parse_clock_output(output)?, source))
}

fn parse_clock_hz(value: &str) -> Result<(usize, u32), String> {
    let (output, frequency) = value
        .split_once('=')
        .ok_or_else(|| format!("expected OUTPUT=HZ, got {value:?}"))?;
    let frequency = frequency
        .parse()
        .map_err(|_| format!("invalid clock frequency {frequency:?}"))?;
    if frequency == 0 {
        return Err("clock frequency must be greater than zero".into());
    }
    Ok((parse_clock_output(output)?, frequency))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adc_conversion_uses_the_full_code_range() {
        assert_eq!(adc_millivolts(0, 3_300, 12), 0);
        assert_eq!(adc_millivolts(2_048, 3_300, 12), 1_650);
        assert_eq!(adc_millivolts(4_095, 3_300, 12), 3_299);
    }

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
            "--disable-spi0-cs1",
        ])
        .unwrap();
        let Command::Set(arguments) = cli.command else {
            panic!("expected set command");
        };
        assert_eq!(arguments.uart0_tx, Some(2));
        assert_eq!(arguments.uart1_rx, Some(7));
        assert!(arguments.disable_uart0);
        assert!(arguments.enable_uart1);
        assert!(arguments.disable_spi0_cs1);
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
        assert!(
            Cli::try_parse_from([
                "rp-extioctl",
                "set",
                "ABC",
                "--enable-spi0-cs1",
                "--disable-spi0-cs1",
            ])
            .is_err()
        );
    }

    #[test]
    fn parses_auxiliary_routes_clocks_and_convenience_commands() {
        let cli = Cli::try_parse_from([
            "rp-extioctl",
            "set",
            "ABC",
            "--route",
            "pwm1a=GP18",
            "--enable",
            "adc0",
            "--clock-source",
            "0=xosc",
            "--clock-hz",
            "clock0=2000000",
        ])
        .unwrap();
        let Command::Set(arguments) = cli.command else {
            panic!("expected set command");
        };
        assert_eq!(arguments.routes, [(PinSignal::Pwm1A, 18)]);
        assert_eq!(arguments.enable_signals, [PinSignal::Adc0]);
        assert_eq!(arguments.clock_sources, [(0, ClockSource::Crystal)]);
        assert_eq!(arguments.clock_hz, [(0, 2_000_000)]);

        assert!(
            Cli::try_parse_from([
                "rp-extioctl",
                "pwm",
                "set",
                "ABC",
                "3",
                "--frequency-hz",
                "50",
                "--duty-percent",
                "7.5",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from(["rp-extioctl", "button", "ABC", "GP22", "--active-low"]).is_ok()
        );
        assert!(
            Cli::try_parse_from(["rp-extioctl", "button", "ABC", "GP22", "--active-high"]).is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "rp-extioctl",
                "button",
                "ABC",
                "GP22",
                "--active-low",
                "--active-high",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "rp-extioctl",
                "pwm",
                "set",
                "ABC",
                "6",
                "--frequency-hz",
                "1000",
                "--duty-percent",
                "25",
                "--persist",
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["rp-extioctl", "pwm", "forget", "ABC", "6"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "rp-extioctl",
                "button",
                "ABC",
                "GP22",
                "--wait",
                "falling",
                "--timeout-ms",
                "1000",
            ])
            .is_ok()
        );
    }

    #[test]
    fn parses_persisted_gpio_commands() {
        let cli = Cli::try_parse_from([
            "rp-extioctl",
            "gpio",
            "set",
            "ABC",
            "GP22",
            "high",
            "--persist",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Gpio {
                command: GpioCommand::Set {
                    pin: 22,
                    persist: true,
                    ..
                }
            }
        ));

        assert!(
            Cli::try_parse_from([
                "rp-extioctl",
                "gpio",
                "input",
                "ABC",
                "GP28",
                "--pull",
                "pull-up",
                "--persist",
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["rp-extioctl", "gpio", "forget", "ABC", "GP22"]).is_ok());
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
            iio_device: Some(4),
            pwm_chip: Some(5),
            gpio_startup: vec![],
            pwm_startup: vec![],
            buttons: vec![],
        });
        assert!(summary.contains("enabled=uart0,i2c0,spi0"));
        assert!(summary.contains("i2c=0:7,1:-"));
        assert!(summary.contains("spi=0:2,1:-"));
        assert!(summary.contains("iio=iio:device4"));
        assert!(summary.contains("pwm=pwmchip5"));
    }
}
