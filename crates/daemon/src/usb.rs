use crate::{
	backend::{Backend, BackendReply, Transaction},
	control::{
		BoardInfo, ButtonConfig, ConfigStore, ControlRequest, ControlResponse, GpioBias, GpioInfo,
		GpioStartup, LedInfo, PwmInfo, validate_buttons_for, validate_gpio_startup_for,
		validate_pwm_startup_for,
	},
	kernel::{
		DeviceRegistration, I2cRegistration, Kernel, RegistrationConfig, SpiRegistration,
		UartRegistration,
	},
	serial::SerialBackend,
};
use async_trait::async_trait;
use futures_lite::StreamExt;
use nusb::{
	DeviceId, DeviceInfo, Endpoint,
	descriptors::TransferType,
	transfer::{Buffer, Bulk, Direction, In, Out},
};
use rp_extio_wire::{
	BOARD_CAPABILITY_VERSION, BoardKind, CAP_ADC, CAP_BOARD, CAP_CLOCK, CAP_GENERAL, CAP_GPIO,
	CAP_I2C, CAP_ONBOARD_LED, CAP_PIN_CONFIG, CAP_PWM, CAP_SPI, CAP_UART, DeviceIdentity, LED_GET,
	LED_SET, LedState, MAX_GPIO_LINES, MAX_PAYLOAD, OnboardLed, OnboardLedKind, Operation,
	PIN_MAP_LEN, Peripheral, PinMap, PinValidationError, PwmState, Status, TransportKind,
	USB_HEADER_LEN, UsbHeader,
};
use std::{
	collections::{HashMap, HashSet},
	fs, io,
	path::{Path, PathBuf},
	sync::{
		Arc,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
	time::Duration,
};
use tokio::sync::{Mutex, mpsc, oneshot};

pub const DEVELOPMENT_VID: u16 = 0xcafe;
pub const DEVELOPMENT_PID: u16 = 0x4010;
const ESP32_C3_BRIDGES: &[(u16, u16)] = &[(0x10c4, 0xea60)];
const USB_DEADLINE: Duration = Duration::from_secs(6);
const USB_DRAIN_QUIET: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NegotiatedConfig {
	pub board_kind: BoardKind,
	pub transport: TransportKind,
	pub identity: DeviceIdentity,
	pub onboard_led: Option<OnboardLed>,
	pub registration: RegistrationConfig,
	pub pin_map: PinMap,
	pub generation: u64,
}

#[derive(Debug)]
pub enum ConfigureError {
	Rejected(PinValidationError),
	Transport(io::Error),
}

impl std::fmt::Display for ConfigureError {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Rejected(error) => write!(formatter, "firmware rejected pin map: {error:?}"),
			Self::Transport(error) => error.fmt(formatter),
		}
	}
}

impl std::error::Error for ConfigureError {}

struct ConnectedDevice {
	serial: String,
	backend: Arc<ConnectedBackend>,
	registration: DeviceRegistration,
	gpio_poll: tokio::task::JoinHandle<()>,
	config: NegotiatedConfig,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DeviceKey {
	Usb(DeviceId),
	Serial(PathBuf),
}

enum ConnectedBackend {
	Usb(Arc<UsbBackend>),
	Serial(Arc<SerialBackend>),
}

impl ConnectedBackend {
	async fn configure_pins(
		&self,
		map: PinMap,
		generation: u64,
	) -> Result<NegotiatedConfig, ConfigureError> {
		match self {
			Self::Usb(backend) => backend.configure_pins(map, generation).await,
			Self::Serial(backend) => backend.configure_pins(map, generation).await,
		}
	}

	fn fail(&self) {
		match self {
			Self::Usb(backend) => backend.fail(),
			Self::Serial(backend) => backend.fail(),
		}
	}
}

#[async_trait]
impl Backend for ConnectedBackend {
	async fn transact(&self, transaction: Transaction) -> BackendReply {
		match self {
			Self::Usb(backend) => backend.transact(transaction).await,
			Self::Serial(backend) => backend.transact(transaction).await,
		}
	}

	async fn disconnect(&self) {
		match self {
			Self::Usb(backend) => backend.disconnect().await,
			Self::Serial(backend) => backend.disconnect().await,
		}
	}
}

pub struct ControlCall {
	pub request: ControlRequest,
	pub response: oneshot::Sender<ControlResponse>,
}

pub async fn run(
	kernel: Arc<Kernel>,
	config_path: PathBuf,
	strict: bool,
	serial_paths: Vec<PathBuf>,
	mut control: mpsc::Receiver<ControlCall>,
) -> io::Result<()> {
	let mut config = ConfigStore::load(config_path)?;
	let mut watch = nusb::watch_devices().map_err(nusb_error)?;
	let initial: Vec<_> = nusb::list_devices()
		.await
		.map_err(nusb_error)?
		.filter(matches_bridge)
		.collect();
	let (failed_tx, mut failed_rx) = mpsc::unbounded_channel();
	let (serial_result_tx, mut serial_result_rx) = mpsc::unbounded_channel();
	let mut devices = HashMap::new();
	let mut serial_in_flight = HashSet::new();
	let mut serial_probe_errors = HashMap::new();
	for info in initial {
		try_connect(&kernel, &failed_tx, &config, &mut devices, info).await;
	}
	for path in serial_candidates(&serial_paths) {
		start_serial_probe(path, &failed_tx, &serial_result_tx, &mut serial_in_flight);
	}
	if strict {
		while !serial_in_flight.is_empty() {
			let (path, result) = serial_result_rx.recv().await.ok_or_else(|| {
				io::Error::new(io::ErrorKind::BrokenPipe, "serial probe channel closed")
			})?;
			serial_in_flight.remove(&path);
			handle_serial_probe_result(
				&kernel,
				&config,
				&mut devices,
				&mut serial_probe_errors,
				path,
				result,
			)
			.await;
		}
		for serial in config.boards().keys() {
			if !devices.values().any(|device| &device.serial == serial) {
				return Err(io::Error::new(
					io::ErrorKind::NotFound,
					format!("configured rp-extio board {serial} is not connected"),
				));
			}
		}
	}
	let mut serial_retry = tokio::time::interval(Duration::from_secs(1));
	loop {
		tokio::select! {
			event = watch.next() => match event {
				Some(nusb::hotplug::HotplugEvent::Connected(info)) if matches_bridge(&info) => {
					try_connect(&kernel, &failed_tx, &config, &mut devices, info).await;
				}
				Some(nusb::hotplug::HotplugEvent::Disconnected(id)) => {
					remove_device(&mut devices, &DeviceKey::Usb(id)).await;
				}
				Some(nusb::hotplug::HotplugEvent::Connected(_)) => {}
				None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "USB hotplug watch ended")),
			},
			failed = failed_rx.recv() => match failed {
				Some(id) => remove_device(&mut devices, &id).await,
				None => return Err(io::Error::new(io::ErrorKind::BrokenPipe, "USB failure channel closed")),
			},
			result = serial_result_rx.recv() => match result {
				Some((path, result)) => {
					serial_in_flight.remove(&path);
					handle_serial_probe_result(
						&kernel,
						&config,
						&mut devices,
						&mut serial_probe_errors,
						path,
						result,
					).await;
				}
				None => return Err(io::Error::new(io::ErrorKind::BrokenPipe, "serial probe channel closed")),
			},
			_ = serial_retry.tick() => {
				for path in serial_candidates(&serial_paths) {
					if !devices.contains_key(&DeviceKey::Serial(path.clone())) && !serial_in_flight.contains(&path) {
						start_serial_probe(
							path,
							&failed_tx,
							&serial_result_tx,
							&mut serial_in_flight,
						);
					}
				}
			},
			call = control.recv() => match call {
				Some(call) => {
					let response = handle_control(&kernel, &mut config, &mut devices, call.request).await;
					let _ = call.response.send(response);
				}
				None => return Err(io::Error::new(io::ErrorKind::BrokenPipe, "control channel closed")),
			},
		}
	}
}

fn serial_candidates(explicit: &[PathBuf]) -> HashSet<PathBuf> {
	let mut paths: HashSet<_> = explicit.iter().cloned().collect();
	let Ok(entries) = fs::read_dir("/dev/serial/by-id") else {
		return paths;
	};
	for entry in entries.flatten() {
		let path = entry.path();
		if is_supported_serial_bridge(&path) {
			paths.insert(path);
		}
	}
	paths
}

fn is_supported_serial_bridge(path: &Path) -> bool {
	let Ok(device) = fs::canonicalize(path) else {
		return false;
	};
	let Some(tty) = device.file_name() else {
		return false;
	};
	let Ok(mut sysfs) = fs::canonicalize(Path::new("/sys/class/tty").join(tty).join("device"))
	else {
		return false;
	};
	loop {
		if let (Some(vendor), Some(product)) = (
			read_hex_u16(&sysfs.join("idVendor")),
			read_hex_u16(&sysfs.join("idProduct")),
		) {
			return ESP32_C3_BRIDGES.contains(&(vendor, product));
		}
		if !sysfs.pop() {
			return false;
		}
	}
}

fn read_hex_u16(path: &Path) -> Option<u16> {
	u16::from_str_radix(fs::read_to_string(path).ok()?.trim(), 16).ok()
}

fn matches_bridge(info: &DeviceInfo) -> bool {
	info.vendor_id() == DEVELOPMENT_VID
		&& info.product_id() == DEVELOPMENT_PID
		&& info.interfaces().any(|interface| {
			interface.class() == 0xff && interface.subclass() == 0 && interface.protocol() == 0
		})
}

async fn try_connect(
	kernel: &Arc<Kernel>,
	failed: &mpsc::UnboundedSender<DeviceKey>,
	desired: &ConfigStore,
	devices: &mut HashMap<DeviceKey, ConnectedDevice>,
	info: DeviceInfo,
) {
	let id = DeviceKey::Usb(info.id());
	if devices.contains_key(&id) {
		return;
	}
	let Some(serial) = info.serial_number().map(str::to_owned) else {
		eprintln!("refusing rp-extio USB device without stable serial number");
		return;
	};
	let (backend, mut config) = match UsbBackend::connect(&info, id.clone(), failed.clone()).await {
		Ok(connected) => connected,
		Err(error) => {
			eprintln!(
				"failed to connect rp-extio USB device {}: {error}",
				info.serial_number().unwrap_or("without serial number")
			);
			return;
		}
	};
	let backend = Arc::new(ConnectedBackend::Usb(backend));
	if config.transport != TransportKind::UsbBulk || config.identity.as_bytes() != serial.as_bytes()
	{
		eprintln!("refusing rp-extio USB device {serial}: firmware identity or transport mismatch");
		backend.disconnect().await;
		return;
	}
	if devices.values().any(|device| device.serial == serial) {
		eprintln!("refusing duplicate connected rp-extio identity {serial}");
		backend.disconnect().await;
		return;
	}
	if let Some(map) = desired.get(&serial)
		&& map != config.pin_map
	{
		config = match backend
			.configure_pins(map, config.generation.wrapping_add(1))
			.await
		{
			Ok(config) => config,
			Err(error) => {
				eprintln!("failed to configure rp-extio USB device {serial}: {error}");
				return;
			}
		};
	}
	if let Err(error) = apply_gpio_startup(
		backend.as_ref(),
		&config.pin_map,
		config.board_kind.profile(),
		desired.gpio_startup(&serial),
	)
	.await
	{
		eprintln!("failed to apply GPIO startup state on rp-extio USB device {serial}: {error:?}");
		backend.disconnect().await;
		return;
	}
	if let Err(error) = apply_pwm_startup(
		backend.as_ref(),
		&config.pin_map,
		config.board_kind.profile(),
		desired.pwm_startup(&serial),
	)
	.await
	{
		eprintln!("failed to apply PWM startup state on rp-extio USB device {serial}: {error:?}");
		backend.disconnect().await;
		return;
	}
	match register_device(kernel, backend.clone(), serial.clone(), config) {
		Ok(device) => {
			eprintln!(
				"rp-extio {serial}: generation {}, {:?}",
				config.generation, config.pin_map
			);
			devices.insert(id, device);
		}
		Err(error) => {
			eprintln!(
				"failed to register rp-extio USB device {}: {error}",
				info.serial_number().unwrap_or("without serial number")
			);
			backend.disconnect().await;
		}
	}
}

type SerialProbeResult = io::Result<(Arc<SerialBackend>, NegotiatedConfig)>;

fn start_serial_probe(
	path: PathBuf,
	failed: &mpsc::UnboundedSender<DeviceKey>,
	results: &mpsc::UnboundedSender<(PathBuf, SerialProbeResult)>,
	in_flight: &mut HashSet<PathBuf>,
) {
	if !in_flight.insert(path.clone()) {
		return;
	}
	let id = DeviceKey::Serial(path.clone());
	let failed = failed.clone();
	let results = results.clone();
	tokio::spawn(async move {
		let result = SerialBackend::connect(&path, id, failed).await;
		let _ = results.send((path, result));
	});
}

async fn handle_serial_probe_result(
	kernel: &Arc<Kernel>,
	desired: &ConfigStore,
	devices: &mut HashMap<DeviceKey, ConnectedDevice>,
	errors: &mut HashMap<PathBuf, String>,
	path: PathBuf,
	result: SerialProbeResult,
) {
	let (backend, config) = match result {
		Ok(connected) => {
			errors.remove(&path);
			connected
		}
		Err(error) if error.kind() == io::ErrorKind::NotFound => {
			errors.remove(&path);
			return;
		}
		Err(error) => {
			let message = error.to_string();
			if remember_serial_probe_error(errors, &path, &message) {
				eprintln!(
					"failed to probe rp-extio serial device {}: {message}",
					path.display()
				);
			}
			return;
		}
	};
	finish_connect_serial(kernel, desired, devices, path, backend, config).await;
}

fn remember_serial_probe_error(
	errors: &mut HashMap<PathBuf, String>,
	path: &Path,
	message: &str,
) -> bool {
	if errors.get(path).is_some_and(|previous| previous == message) {
		return false;
	}
	errors.insert(path.to_owned(), message.to_owned());
	true
}

async fn finish_connect_serial(
	kernel: &Arc<Kernel>,
	desired: &ConfigStore,
	devices: &mut HashMap<DeviceKey, ConnectedDevice>,
	path: PathBuf,
	backend: Arc<SerialBackend>,
	mut config: NegotiatedConfig,
) {
	let id = DeviceKey::Serial(path.clone());
	if devices.contains_key(&id) {
		backend.disconnect().await;
		return;
	}
	let serial = match core::str::from_utf8(config.identity.as_bytes()) {
		Ok(serial) => serial.to_owned(),
		Err(_) => {
			eprintln!(
				"refusing rp-extio serial device {} with non-text identity",
				path.display()
			);
			backend.disconnect().await;
			return;
		}
	};
	if devices.values().any(|device| device.serial == serial) {
		eprintln!("refusing duplicate connected rp-extio identity {serial}");
		backend.disconnect().await;
		return;
	}
	let backend = Arc::new(ConnectedBackend::Serial(backend));
	if let Some(map) = desired.get(&serial)
		&& map != config.pin_map
	{
		config = match backend
			.configure_pins(map, config.generation.wrapping_add(1))
			.await
		{
			Ok(config) => config,
			Err(error) => {
				eprintln!("failed to configure rp-extio serial device {serial}: {error}");
				backend.disconnect().await;
				return;
			}
		};
	}
	if apply_gpio_startup(
		backend.as_ref(),
		&config.pin_map,
		config.board_kind.profile(),
		desired.gpio_startup(&serial),
	)
	.await
	.is_err()
		|| apply_pwm_startup(
			backend.as_ref(),
			&config.pin_map,
			config.board_kind.profile(),
			desired.pwm_startup(&serial),
		)
		.await
		.is_err()
	{
		backend.disconnect().await;
		return;
	}
	match register_device(kernel, backend.clone(), serial.clone(), config) {
		Ok(device) => {
			eprintln!(
				"rp-extio {serial} via {}: generation {}",
				path.display(),
				config.generation
			);
			devices.insert(id, device);
		}
		Err(error) => {
			eprintln!("failed to register rp-extio serial device {serial}: {error}");
			backend.disconnect().await;
		}
	}
}

fn register_device(
	kernel: &Arc<Kernel>,
	backend: Arc<ConnectedBackend>,
	serial: String,
	config: NegotiatedConfig,
) -> io::Result<ConnectedDevice> {
	let registration = kernel.register_device(backend.clone(), config.registration)?;
	let gpio_instance_id = registration.gpio_instance_id();
	let gpio_poll = tokio::spawn(poll_gpio(
		backend.clone(),
		kernel.clone(),
		gpio_instance_id,
		config.registration.gpio_lines,
	));
	Ok(ConnectedDevice {
		serial,
		backend,
		registration,
		gpio_poll,
		config,
	})
}

async fn remove_device(devices: &mut HashMap<DeviceKey, ConnectedDevice>, id: &DeviceKey) {
	if let Some(device) = devices.remove(id) {
		device.gpio_poll.abort();
		device.backend.disconnect().await;
		let _ = device.registration.destroy();
	}
}

fn board_info(
	device: &ConnectedDevice,
	desired: Option<PinMap>,
	gpio_startup: &[GpioStartup],
	pwm_startup: &[PwmInfo],
	buttons: &[ButtonConfig],
) -> BoardInfo {
	BoardInfo {
		serial: device.serial.clone(),
		connected: true,
		board_kind: match device.config.board_kind {
			BoardKind::Pico2 => "pico2",
			BoardKind::Esp32C3DevKitM1 => "esp32_c3_devkitm_1",
		}
		.into(),
		board_name: device.config.board_kind.profile().name().into(),
		transport: match device.config.transport {
			TransportKind::UsbBulk => "usb_bulk",
			TransportKind::Serial => "serial",
		}
		.into(),
		supported_signals: device.config.board_kind.profile().supported_signals(),
		adc_channels: device.config.registration.adc.map_or(0, |adc| adc.channels),
		pwm_channels: device.config.registration.pwm.map_or(0, |pwm| pwm.channels),
		adc_reference_mv: device
			.config
			.registration
			.adc
			.map_or(0, |adc| adc.reference_mv),
		active: Some(device.config.pin_map.into()),
		desired: desired.map(Into::into),
		generation: Some(device.config.generation),
		i2c_buses: device.registration.buses.i2c,
		spi_buses: device.registration.buses.spi,
		uart_ttys: device.registration.buses.uart,
		gpio_instance: Some(device.registration.gpio_instance_id()),
		iio_device: device.registration.iio_device,
		pwm_chip: device.registration.pwm_chip,
		onboard_led: device.config.onboard_led.map(Into::into),
		gpio_startup: gpio_startup.to_vec(),
		pwm_startup: pwm_startup.to_vec(),
		buttons: buttons.to_vec(),
	}
}

fn disconnected_info(
	serial: String,
	board_kind: BoardKind,
	desired: PinMap,
	gpio_startup: &[GpioStartup],
	pwm_startup: &[PwmInfo],
	buttons: &[ButtonConfig],
) -> BoardInfo {
	let profile = board_kind.profile();
	BoardInfo {
		serial,
		connected: false,
		board_kind: match board_kind {
			BoardKind::Pico2 => "pico2",
			BoardKind::Esp32C3DevKitM1 => "esp32_c3_devkitm_1",
		}
		.into(),
		board_name: profile.name().into(),
		transport: match profile.transport() {
			TransportKind::UsbBulk => "usb_bulk",
			TransportKind::Serial => "serial",
		}
		.into(),
		supported_signals: profile.supported_signals(),
		adc_channels: profile.adc_channel_count() as u8,
		pwm_channels: profile.pwm_channel_count() as u8,
		adc_reference_mv: match board_kind {
			BoardKind::Pico2 => 3_300,
			BoardKind::Esp32C3DevKitM1 => 2_500,
		},
		active: None,
		desired: Some(desired.into()),
		generation: None,
		i2c_buses: [None; 2],
		spi_buses: [None; 2],
		uart_ttys: [None; 2],
		gpio_instance: None,
		iio_device: None,
		pwm_chip: None,
		onboard_led: None,
		gpio_startup: gpio_startup.to_vec(),
		pwm_startup: pwm_startup.to_vec(),
		buttons: buttons.to_vec(),
	}
}

fn find_device(devices: &HashMap<DeviceKey, ConnectedDevice>, serial: &str) -> Option<DeviceKey> {
	devices
		.iter()
		.find_map(|(id, device)| (device.serial == serial).then(|| id.clone()))
}

async fn handle_control(
	kernel: &Arc<Kernel>,
	store: &mut ConfigStore,
	devices: &mut HashMap<DeviceKey, ConnectedDevice>,
	request: ControlRequest,
) -> ControlResponse {
	match request {
		ControlRequest::List => {
			let mut boards: Vec<_> = devices
				.values()
				.map(|device| {
					board_info(
						device,
						store.get(&device.serial),
						store.gpio_startup(&device.serial),
						store.pwm_startup(&device.serial),
						store.buttons(&device.serial),
					)
				})
				.collect();
			for (serial, map) in store.boards() {
				if !devices.values().any(|device| device.serial == *serial) {
					boards.push(disconnected_info(
						serial.clone(),
						store.board_kind(serial),
						*map,
						store.gpio_startup(serial),
						store.pwm_startup(serial),
						store.buttons(serial),
					));
				}
			}
			boards.sort_by(|left, right| left.serial.cmp(&right.serial));
			ControlResponse::Boards { boards }
		}
		ControlRequest::Show { serial } => {
			if let Some(id) = find_device(devices, &serial) {
				ControlResponse::Board {
					board: board_info(
						&devices[&id],
						store.get(&serial),
						store.gpio_startup(&serial),
						store.pwm_startup(&serial),
						store.buttons(&serial),
					),
				}
			} else if let Some(map) = store.get(&serial) {
				ControlResponse::Board {
					board: disconnected_info(
						serial.clone(),
						store.board_kind(&serial),
						map,
						store.gpio_startup(&serial),
						store.pwm_startup(&serial),
						store.buttons(&serial),
					),
				}
			} else {
				ControlResponse::error("unknown_board", format!("unknown rp-extio board {serial}"))
			}
		}
		ControlRequest::Set {
			serial,
			generation,
			map,
		} => {
			let map = PinMap::from(map);
			let profile = find_device(devices, &serial)
				.map(|id| devices[&id].config.board_kind.profile())
				.unwrap_or_else(|| store.board_kind(&serial).profile());
			if let Err(error) = map.validate_for(profile) {
				return ControlResponse::validation(error);
			}
			if let Err(message) =
				validate_gpio_startup_for(&map, profile, store.gpio_startup(&serial))
			{
				return ControlResponse::error("gpio_startup_conflict", message);
			}
			if let Err(message) =
				validate_pwm_startup_for(&map, profile, store.pwm_startup(&serial))
			{
				return ControlResponse::error("pwm_startup_conflict", message);
			}
			if let Err(message) = validate_buttons_for(&map, profile, store.buttons(&serial)) {
				return ControlResponse::error("button_conflict", message);
			}
			change_map(kernel, store, devices, serial, generation, map, false).await
		}
		ControlRequest::Default { serial, generation } => {
			let default = find_device(devices, &serial)
				.map(|id| devices[&id].config.board_kind.profile().default_pin_map())
				.unwrap_or_else(|| store.board_kind(&serial).profile().default_pin_map());
			change_map(kernel, store, devices, serial, generation, default, true).await
		}
		ControlRequest::Reload => {
			if let Err(error) = store.reload() {
				return ControlResponse::error("configuration", error.to_string());
			}
			let changes: Vec<_> = devices
				.iter()
				.filter_map(|(id, device)| {
					let desired = store
						.get(&device.serial)
						.unwrap_or_else(|| device.config.board_kind.profile().default_pin_map());
					(desired != device.config.pin_map).then_some((id.clone(), desired))
				})
				.collect();
			for (id, map) in changes {
				if let Err(response) = reconfigure(
					kernel,
					devices,
					id.clone(),
					map,
					store.gpio_startup(&devices[&id].serial),
					store.pwm_startup(&devices[&id].serial),
				)
				.await
				{
					return response;
				}
			}
			for device in devices.values() {
				if let Err(response) = apply_gpio_startup(
					device.backend.as_ref(),
					&device.config.pin_map,
					device.config.board_kind.profile(),
					store.gpio_startup(&device.serial),
				)
				.await
				{
					return response;
				}
				if let Err(response) = apply_pwm_startup(
					device.backend.as_ref(),
					&device.config.pin_map,
					device.config.board_kind.profile(),
					store.pwm_startup(&device.serial),
				)
				.await
				{
					return response;
				}
			}
			ControlResponse::Ok
		}
		ControlRequest::AdcRead { serial, channel } => {
			let Some(id) = find_device(devices, &serial) else {
				return disconnected(&serial);
			};
			let device = &devices[&id];
			if channel as usize >= device.config.board_kind.profile().adc_channel_count()
				|| device.config.pin_map.adc_enabled & (1 << channel) == 0
			{
				return ControlResponse::error(
					"unsupported",
					format!("ADC{channel} is not enabled on {serial}"),
				);
			}
			let reply = device.backend.transact(Transaction::Adc { channel }).await;
			if let Some(error) = backend_error(reply.status, "ADC conversion") {
				return error;
			}
			if reply.completed != 1 || reply.data.len() != 2 {
				return ControlResponse::error("protocol", "invalid ADC conversion reply");
			}
			let registration = device.config.registration.adc.unwrap();
			ControlResponse::Adc {
				channel,
				raw: u16::from_le_bytes(reply.data.try_into().unwrap()),
				reference_mv: registration.reference_mv,
				resolution_bits: registration.resolution_bits,
			}
		}
		ControlRequest::PwmGet { serial, channel } => {
			control_pwm(
				devices,
				&serial,
				rp_extio_wire::PWM_GET,
				PwmInfo {
					channel,
					enabled: false,
					inverted: false,
					period_ns: 0,
					duty_ns: 0,
				},
			)
			.await
		}
		ControlRequest::PwmApply { serial, pwm } => {
			control_pwm(devices, &serial, rp_extio_wire::PWM_APPLY, pwm).await
		}
		ControlRequest::PwmStartupSet { serial, pwm } => {
			let Some(id) = find_device(devices, &serial) else {
				return disconnected(&serial);
			};
			let device = &devices[&id];
			if let Err(message) = validate_pwm_startup_for(
				&device.config.pin_map,
				device.config.board_kind.profile(),
				&[pwm],
			) {
				return ControlResponse::error("invalid_pwm", message);
			}
			let previous = store
				.pwm_startup(&serial)
				.iter()
				.copied()
				.find(|entry| entry.channel == pwm.channel);
			let had_board = store.contains_board(&serial);
			if let Err(error) =
				store.set_pwm_startup_for(serial.clone(), device.config.board_kind, pwm)
			{
				return ControlResponse::error("configuration", error.to_string());
			}
			if let Err(response) = apply_one_pwm_startup(
				device.backend.as_ref(),
				&device.config.pin_map,
				device.config.board_kind.profile(),
				pwm,
			)
			.await
			{
				let rollback = match previous {
					Some(previous) => store.set_pwm_startup_for(
						serial.clone(),
						device.config.board_kind,
						previous,
					),
					None if had_board => store.clear_pwm_startup(&serial, pwm.channel),
					None => store.remove(&serial),
				};
				if let Err(error) = rollback {
					return ControlResponse::error(
						"configuration",
						format!(
							"device application failed and configuration rollback failed: {error}"
						),
					);
				}
				return response;
			}
			ControlResponse::Board {
				board: board_info(
					device,
					store.get(&serial),
					store.gpio_startup(&serial),
					store.pwm_startup(&serial),
					store.buttons(&serial),
				),
			}
		}
		ControlRequest::PwmStartupClear { serial, channel } => {
			if find_device(devices, &serial).is_none() && !store.contains_board(&serial) {
				return ControlResponse::error(
					"unknown_board",
					format!("unknown rp-extio board {serial}"),
				);
			}
			if let Err(error) = store.clear_pwm_startup(&serial, channel) {
				return ControlResponse::error("configuration", error.to_string());
			}
			if let Some(id) = find_device(devices, &serial) {
				ControlResponse::Board {
					board: board_info(
						&devices[&id],
						store.get(&serial),
						store.gpio_startup(&serial),
						store.pwm_startup(&serial),
						store.buttons(&serial),
					),
				}
			} else {
				ControlResponse::Board {
					board: disconnected_info(
						serial.clone(),
						store.board_kind(&serial),
						store.get(&serial).unwrap(),
						store.gpio_startup(&serial),
						store.pwm_startup(&serial),
						store.buttons(&serial),
					),
				}
			}
		}
		ControlRequest::OnboardLedGet { serial } => {
			control_onboard_led(devices, &serial, LED_GET, LedState::OFF).await
		}
		ControlRequest::OnboardLedSet {
			serial,
			red,
			green,
			blue,
		} => {
			control_onboard_led(
				devices,
				&serial,
				LED_SET,
				LedState {
					index: 0,
					red,
					green,
					blue,
				},
			)
			.await
		}
		ControlRequest::GpioSnapshot { serial } => {
			let Some(id) = find_device(devices, &serial) else {
				return disconnected(&serial);
			};
			gpio_snapshot(&devices[&id]).await
		}
		ControlRequest::GpioOutput { serial, pin, value } => {
			let Some(id) = find_device(devices, &serial) else {
				return disconnected(&serial);
			};
			let device = &devices[&id];
			let Some(line) = gpio_line(device, pin) else {
				return ControlResponse::error(
					"unsupported",
					format!("GP{pin} is not configured as GPIO on {serial}"),
				);
			};
			let reply = device
				.backend
				.transact(Transaction::Gpio {
					command: rp_extio_wire::GPIO_DIRECTION_OUTPUT,
					line,
					value: u8::from(value),
				})
				.await;
			if let Some(error) = backend_error(reply.status, "GPIO output configuration") {
				return error;
			}
			gpio_result(device, pin).await
		}
		ControlRequest::GpioInput { serial, pin, bias } => {
			let Some(id) = find_device(devices, &serial) else {
				return disconnected(&serial);
			};
			let device = &devices[&id];
			let Some(line) = gpio_line(device, pin) else {
				return ControlResponse::error(
					"unsupported",
					format!("GP{pin} is not configured as GPIO on {serial}"),
				);
			};
			for (command, value) in [
				(rp_extio_wire::GPIO_DIRECTION_INPUT, 0),
				(rp_extio_wire::GPIO_SET_BIAS, bias.wire_value()),
			] {
				let reply = device
					.backend
					.transact(Transaction::Gpio {
						command,
						line,
						value,
					})
					.await;
				if let Some(error) = backend_error(reply.status, "GPIO input configuration") {
					return error;
				}
			}
			gpio_result(device, pin).await
		}
		ControlRequest::GpioStartupSet { serial, startup } => {
			let Some(id) = find_device(devices, &serial) else {
				return disconnected(&serial);
			};
			let device = &devices[&id];
			if gpio_line(device, startup.pin()).is_none() {
				return ControlResponse::error(
					"unsupported",
					format!("GP{} is not configured as GPIO on {serial}", startup.pin()),
				);
			}
			let previous = store
				.gpio_startup(&serial)
				.iter()
				.copied()
				.find(|entry| entry.pin() == startup.pin());
			let had_board = store.contains_board(&serial);
			if let Err(error) =
				store.set_gpio_startup_for(serial.clone(), device.config.board_kind, startup)
			{
				return ControlResponse::error("configuration", error.to_string());
			}
			if let Err(response) = apply_one_gpio_startup(
				device.backend.as_ref(),
				&device.config.pin_map,
				device.config.board_kind.profile(),
				startup,
			)
			.await
			{
				let rollback = match previous {
					Some(previous) => store.set_gpio_startup_for(
						serial.clone(),
						device.config.board_kind,
						previous,
					),
					None if had_board => store.clear_gpio_startup(&serial, startup.pin()),
					None => store.remove(&serial),
				};
				if let Err(error) = rollback {
					return ControlResponse::error(
						"configuration",
						format!(
							"device application failed and configuration rollback failed: {error}"
						),
					);
				}
				return response;
			}
			ControlResponse::Board {
				board: board_info(
					device,
					store.get(&serial),
					store.gpio_startup(&serial),
					store.pwm_startup(&serial),
					store.buttons(&serial),
				),
			}
		}
		ControlRequest::GpioStartupClear { serial, pin } => {
			if find_device(devices, &serial).is_none() && !store.contains_board(&serial) {
				return ControlResponse::error(
					"unknown_board",
					format!("unknown rp-extio board {serial}"),
				);
			}
			if let Err(error) = store.clear_gpio_startup(&serial, pin) {
				return ControlResponse::error("configuration", error.to_string());
			}
			if let Some(id) = find_device(devices, &serial) {
				ControlResponse::Board {
					board: board_info(
						&devices[&id],
						store.get(&serial),
						store.gpio_startup(&serial),
						store.pwm_startup(&serial),
						store.buttons(&serial),
					),
				}
			} else {
				ControlResponse::Board {
					board: disconnected_info(
						serial.clone(),
						store.board_kind(&serial),
						store.get(&serial).unwrap(),
						store.gpio_startup(&serial),
						store.pwm_startup(&serial),
						store.buttons(&serial),
					),
				}
			}
		}
		ControlRequest::ButtonSet { serial, button } => {
			if find_device(devices, &serial).is_none() && !store.contains_board(&serial) {
				return ControlResponse::error(
					"unknown_board",
					format!("unknown rp-extio board {serial}"),
				);
			}
			let kind = find_device(devices, &serial)
				.map(|id| devices[&id].config.board_kind)
				.unwrap_or_else(|| store.board_kind(&serial));
			if let Err(error) = store.set_button_for(serial.clone(), kind, button) {
				return ControlResponse::error("configuration", error.to_string());
			}
			button_board_response(store, devices, serial)
		}
		ControlRequest::ButtonClear { serial, pin } => {
			if find_device(devices, &serial).is_none() && !store.contains_board(&serial) {
				return ControlResponse::error(
					"unknown_board",
					format!("unknown rp-extio board {serial}"),
				);
			}
			if let Err(error) = store.clear_button(&serial, pin) {
				return ControlResponse::error("configuration", error.to_string());
			}
			button_board_response(store, devices, serial)
		}
	}
}

fn button_board_response(
	store: &ConfigStore,
	devices: &HashMap<DeviceKey, ConnectedDevice>,
	serial: String,
) -> ControlResponse {
	let board = if let Some(id) = find_device(devices, &serial) {
		board_info(
			&devices[&id],
			store.get(&serial),
			store.gpio_startup(&serial),
			store.pwm_startup(&serial),
			store.buttons(&serial),
		)
	} else {
		disconnected_info(
			serial.clone(),
			store.board_kind(&serial),
			store.get(&serial).unwrap(),
			store.gpio_startup(&serial),
			store.pwm_startup(&serial),
			store.buttons(&serial),
		)
	};
	ControlResponse::Board { board }
}

fn disconnected(serial: &str) -> ControlResponse {
	ControlResponse::error(
		"disconnected",
		format!("rp-extio board {serial} is not connected"),
	)
}

fn backend_error(status: i32, operation: &str) -> Option<ControlResponse> {
	let status = Status::try_from(status).unwrap_or(Status::BadFrame);
	(status != Status::Ok).then(|| {
		ControlResponse::error(
			match status {
				Status::Unsupported => "unsupported",
				Status::Busy => "busy",
				Status::Disconnected => "disconnected",
				Status::Timeout => "timeout",
				_ => "device",
			},
			format!("{operation} failed with {status:?}"),
		)
	})
}

async fn control_pwm(
	devices: &HashMap<DeviceKey, ConnectedDevice>,
	serial: &str,
	command: u8,
	pwm: PwmInfo,
) -> ControlResponse {
	let Some(id) = find_device(devices, serial) else {
		return disconnected(serial);
	};
	let device = &devices[&id];
	if pwm.channel as usize >= rp_extio_wire::PWM_CHANNEL_COUNT
		|| device.config.pin_map.pwm_enabled & (1 << pwm.channel) == 0
	{
		return ControlResponse::error(
			"unsupported",
			format!("PWM channel {} is not enabled on {serial}", pwm.channel),
		);
	}
	if pwm.duty_ns > pwm.period_ns || (pwm.enabled && pwm.period_ns == 0) {
		return ControlResponse::error(
			"invalid_pwm",
			"PWM duty must not exceed its nonzero enabled period",
		);
	}
	let reply = device
		.backend
		.transact(Transaction::Pwm {
			command,
			channel: pwm.channel,
			enabled: pwm.enabled,
			inverted: pwm.inverted,
			period_ns: pwm.period_ns,
			duty_ns: pwm.duty_ns,
		})
		.await;
	if let Some(error) = backend_error(reply.status, "PWM operation") {
		return error;
	}
	let Ok((reply_command, state)) = rp_extio_wire::PwmState::decode(&reply.data) else {
		return ControlResponse::error("protocol", "invalid PWM reply");
	};
	if reply.completed != 1 || reply_command != command {
		return ControlResponse::error("protocol", "mismatched PWM reply");
	}
	ControlResponse::Pwm { pwm: state.into() }
}

async fn control_onboard_led(
	devices: &HashMap<DeviceKey, ConnectedDevice>,
	serial: &str,
	command: u8,
	state: LedState,
) -> ControlResponse {
	let Some(id) = find_device(devices, serial) else {
		return disconnected(serial);
	};
	let device = &devices[&id];
	let Some(led) = device.config.onboard_led else {
		return ControlResponse::error("unsupported", format!("{serial} has no onboard LED"));
	};
	if device
		.config
		.board_kind
		.profile()
		.gpio_pins()
		.contains(&led.pin)
		&& gpio_line(device, led.pin).is_none()
	{
		return ControlResponse::error(
			"busy",
			format!(
				"onboard LED pin GPIO{} is routed to another signal",
				led.pin
			),
		);
	}
	if command == LED_SET
		&& led.kind == OnboardLedKind::Monochrome
		&& state != LedState::OFF
		&& (state.red, state.green, state.blue) != (255, 255, 255)
	{
		return ControlResponse::error(
			"unsupported",
			"monochrome onboard LEDs accept only off or white",
		);
	}
	let reply = device
		.backend
		.transact(Transaction::OnboardLed {
			command,
			index: state.index,
			red: state.red,
			green: state.green,
			blue: state.blue,
		})
		.await;
	if let Some(error) = backend_error(reply.status, "onboard LED operation") {
		return error;
	}
	let Ok((reply_command, state)) = LedState::decode(&reply.data) else {
		return ControlResponse::error("protocol", "invalid onboard LED reply");
	};
	if reply.completed != 1 || reply_command != command {
		return ControlResponse::error("protocol", "mismatched onboard LED reply");
	}
	ControlResponse::OnboardLed {
		led: LedInfo {
			red: state.red,
			green: state.green,
			blue: state.blue,
		},
	}
}

fn gpio_line(device: &ConnectedDevice, pin: u8) -> Option<u8> {
	device
		.config
		.pin_map
		.gpio_pins_for(device.config.board_kind.profile())
		.ok()?
		.as_slice()
		.iter()
		.position(|&candidate| candidate == pin)
		.map(|line| line as u8)
}

fn gpio_line_for_map(map: &PinMap, profile: rp_extio_wire::BoardProfile, pin: u8) -> Option<u8> {
	map.gpio_pins_for(profile)
		.ok()?
		.as_slice()
		.iter()
		.position(|&candidate| candidate == pin)
		.map(|line| line as u8)
}

async fn apply_one_gpio_startup(
	backend: &dyn Backend,
	map: &PinMap,
	profile: rp_extio_wire::BoardProfile,
	startup: GpioStartup,
) -> Result<(), ControlResponse> {
	let Some(line) = gpio_line_for_map(map, profile, startup.pin()) else {
		return Err(ControlResponse::error(
			"gpio_startup_conflict",
			format!("GP{} is not configured as GPIO", startup.pin()),
		));
	};
	let commands: &[(u8, u8)] = match startup {
		GpioStartup::Input { bias, .. } => &[
			(rp_extio_wire::GPIO_DIRECTION_INPUT, 0),
			(rp_extio_wire::GPIO_SET_BIAS, bias.wire_value()),
		],
		GpioStartup::Output { value, .. } => &[(rp_extio_wire::GPIO_DIRECTION_OUTPUT, value as u8)],
	};
	for &(command, value) in commands {
		let reply = backend
			.transact(Transaction::Gpio {
				command,
				line,
				value,
			})
			.await;
		if let Some(error) = backend_error(reply.status, "GPIO startup configuration") {
			return Err(error);
		}
	}
	Ok(())
}

async fn apply_gpio_startup(
	backend: &dyn Backend,
	map: &PinMap,
	profile: rp_extio_wire::BoardProfile,
	entries: &[GpioStartup],
) -> Result<(), ControlResponse> {
	for &entry in entries {
		apply_one_gpio_startup(backend, map, profile, entry).await?;
	}
	Ok(())
}

async fn apply_one_pwm_startup(
	backend: &dyn Backend,
	map: &PinMap,
	profile: rp_extio_wire::BoardProfile,
	pwm: PwmInfo,
) -> Result<(), ControlResponse> {
	validate_pwm_startup_for(map, profile, &[pwm])
		.map_err(|message| ControlResponse::error("pwm_startup_conflict", message))?;
	let reply = backend
		.transact(Transaction::Pwm {
			command: rp_extio_wire::PWM_APPLY,
			channel: pwm.channel,
			enabled: pwm.enabled,
			inverted: pwm.inverted,
			period_ns: pwm.period_ns,
			duty_ns: pwm.duty_ns,
		})
		.await;
	if let Some(error) = backend_error(reply.status, "PWM startup configuration") {
		return Err(error);
	}
	let Ok((command, applied)) = rp_extio_wire::PwmState::decode(&reply.data) else {
		return Err(ControlResponse::error(
			"protocol",
			"invalid PWM startup reply",
		));
	};
	if reply.completed != 1 || command != rp_extio_wire::PWM_APPLY || applied != pwm.into() {
		return Err(ControlResponse::error(
			"protocol",
			"mismatched PWM startup reply",
		));
	}
	Ok(())
}

async fn apply_pwm_startup(
	backend: &dyn Backend,
	map: &PinMap,
	profile: rp_extio_wire::BoardProfile,
	entries: &[PwmInfo],
) -> Result<(), ControlResponse> {
	validate_pwm_startup_for(map, profile, entries)
		.map_err(|message| ControlResponse::error("pwm_startup_conflict", message))?;
	let mut affected_timers = 0_u16;
	for entry in entries {
		affected_timers |= 1 << profile.pwm_timer(entry.channel as usize).unwrap();
	}
	for channel in 0..profile.pwm_channel_count() as u8 {
		if affected_timers & (1 << profile.pwm_timer(channel as usize).unwrap()) != 0
			&& map.pwm_enabled & (1 << channel) != 0
		{
			apply_one_pwm_startup(
				backend,
				map,
				profile,
				PwmInfo {
					channel,
					enabled: false,
					inverted: false,
					period_ns: 0,
					duty_ns: 0,
				},
			)
			.await?;
		}
	}
	for &entry in entries {
		apply_one_pwm_startup(backend, map, profile, entry).await?;
	}
	Ok(())
}

async fn gpio_result(device: &ConnectedDevice, pin: u8) -> ControlResponse {
	match gpio_snapshot(device).await {
		ControlResponse::GpioSnapshot { gpio } => {
			let Some(gpio) = gpio.into_iter().find(|gpio| gpio.pin == pin) else {
				return ControlResponse::error("protocol", "configured GPIO disappeared");
			};
			ControlResponse::Gpio { gpio }
		}
		response => response,
	}
}

async fn gpio_snapshot(device: &ConnectedDevice) -> ControlResponse {
	let reply = device
		.backend
		.transact(Transaction::Gpio {
			command: rp_extio_wire::GPIO_SNAPSHOT,
			line: 0,
			value: 0,
		})
		.await;
	if let Some(error) = backend_error(reply.status, "GPIO snapshot") {
		return error;
	}
	let Ok(data): Result<[u8; 16], _> = reply.data.try_into() else {
		return ControlResponse::error("protocol", "invalid GPIO snapshot reply");
	};
	let levels = u32::from_le_bytes(data[0..4].try_into().unwrap());
	let outputs = u32::from_le_bytes(data[4..8].try_into().unwrap());
	let pull_ups = u32::from_le_bytes(data[8..12].try_into().unwrap());
	let pull_downs = u32::from_le_bytes(data[12..16].try_into().unwrap());
	let pins = device
		.config
		.pin_map
		.gpio_pins_for(device.config.board_kind.profile())
		.unwrap();
	if reply.completed != pins.len() as u32 {
		return ControlResponse::error("protocol", "GPIO snapshot line count changed");
	}
	let gpio = pins
		.as_slice()
		.iter()
		.enumerate()
		.map(|(line, &pin)| {
			let mask = 1 << line;
			GpioInfo {
				line: line as u8,
				pin,
				level: levels & mask != 0,
				output: outputs & mask != 0,
				bias: if pull_ups & mask != 0 {
					GpioBias::PullUp
				} else if pull_downs & mask != 0 {
					GpioBias::PullDown
				} else {
					GpioBias::None
				},
			}
		})
		.collect();
	ControlResponse::GpioSnapshot { gpio }
}

async fn change_map(
	kernel: &Arc<Kernel>,
	store: &mut ConfigStore,
	devices: &mut HashMap<DeviceKey, ConnectedDevice>,
	serial: String,
	generation: u64,
	map: PinMap,
	remove_persistent: bool,
) -> ControlResponse {
	let Some(id) = find_device(devices, &serial) else {
		return ControlResponse::error(
			"disconnected",
			format!("rp-extio board {serial} is not connected"),
		);
	};
	let active = devices[&id].config;
	if active.generation != generation {
		return ControlResponse::error(
			"generation_conflict",
			format!(
				"rp-extio board {serial} changed from generation {generation} to {}",
				active.generation
			),
		);
	}
	let needs_reconfigure = active.pin_map != map
		|| (remove_persistent
			&& (!store.gpio_startup(&serial).is_empty() || !store.pwm_startup(&serial).is_empty()));
	if needs_reconfigure {
		let (gpio_startup, pwm_startup) = if remove_persistent {
			(&[][..], &[][..])
		} else {
			(store.gpio_startup(&serial), store.pwm_startup(&serial))
		};
		if let Err(response) =
			reconfigure(kernel, devices, id, map, gpio_startup, pwm_startup).await
		{
			return response;
		}
	}

	let persisted = if remove_persistent {
		store.remove(&serial)
	} else {
		store.set_for(serial.clone(), active.board_kind, map)
	};
	if let Err(error) = persisted {
		if needs_reconfigure && let Some(id) = find_device(devices, &serial) {
			let _ = reconfigure(
				kernel,
				devices,
				id,
				active.pin_map,
				store.gpio_startup(&serial),
				store.pwm_startup(&serial),
			)
			.await;
		}
		return ControlResponse::error("configuration", error.to_string());
	}
	let id = find_device(devices, &serial).unwrap();
	ControlResponse::Applied {
		board: board_info(
			&devices[&id],
			store.get(&serial),
			store.gpio_startup(&serial),
			store.pwm_startup(&serial),
			store.buttons(&serial),
		),
	}
}

async fn reconfigure(
	kernel: &Arc<Kernel>,
	devices: &mut HashMap<DeviceKey, ConnectedDevice>,
	id: DeviceKey,
	map: PinMap,
	gpio_startup: &[GpioStartup],
	pwm_startup: &[PwmInfo],
) -> Result<(), ControlResponse> {
	let device = devices.remove(&id).unwrap();
	let ConnectedDevice {
		serial,
		backend,
		registration,
		gpio_poll,
		config: old_config,
	} = device;
	gpio_poll.abort();
	if let Err(error) = registration.destroy() {
		if let Ok(restored) = register_device(kernel, backend, serial, old_config) {
			devices.insert(id, restored);
		}
		return Err(ControlResponse::error("kernel", error.to_string()));
	}
	let generation = old_config.generation.wrapping_add(1);
	let new_config = match backend.configure_pins(map, generation).await {
		Ok(config) => config,
		Err(ConfigureError::Rejected(error)) => {
			if let Ok(restored) = register_device(kernel, backend, serial, old_config) {
				devices.insert(id, restored);
			}
			return Err(ControlResponse::validation(error));
		}
		Err(ConfigureError::Transport(error)) => {
			backend.disconnect().await;
			return Err(ControlResponse::error("usb", error.to_string()));
		}
	};
	if let Err(response) = apply_gpio_startup(
		backend.as_ref(),
		&new_config.pin_map,
		new_config.board_kind.profile(),
		gpio_startup,
	)
	.await
	{
		backend.disconnect().await;
		return Err(response);
	}
	if let Err(response) = apply_pwm_startup(
		backend.as_ref(),
		&new_config.pin_map,
		new_config.board_kind.profile(),
		pwm_startup,
	)
	.await
	{
		backend.disconnect().await;
		return Err(response);
	}
	match register_device(kernel, backend.clone(), serial, new_config) {
		Ok(device) => {
			devices.insert(id, device);
			Ok(())
		}
		Err(error) => {
			backend.disconnect().await;
			Err(ControlResponse::error("kernel", error.to_string()))
		}
	}
}

async fn poll_gpio(
	backend: Arc<ConnectedBackend>,
	kernel: Arc<Kernel>,
	instance_id: u32,
	lines: u16,
) {
	let mut previous: Option<u32> = None;
	let mut interval = tokio::time::interval(Duration::from_millis(10));
	loop {
		interval.tick().await;
		let reply = backend
			.transact(Transaction::Gpio {
				command: rp_extio_wire::GPIO_SNAPSHOT,
				line: 0,
				value: 0,
			})
			.await;
		if reply.status != Status::Ok as i32
			|| reply.completed != lines as u32
			|| reply.data.len() != 16
		{
			backend.fail();
			return;
		}
		let values = u32::from_le_bytes(reply.data[..4].try_into().unwrap());
		if let Some(old) = previous {
			let mut changed = (old ^ values) & ((1_u32 << lines) - 1);
			while changed != 0 {
				let line = changed.trailing_zeros() as u8;
				let mask = 1_u32 << line;
				if kernel
					.gpio_event(
						instance_id,
						line,
						u8::from(old & mask != 0),
						u8::from(values & mask != 0),
					)
					.is_err()
				{
					backend.fail();
					return;
				}
				changed &= !mask;
			}
		}
		previous = Some(values);
	}
}

struct Transport {
	out_endpoint: Endpoint<Bulk, Out>,
	in_endpoint: Endpoint<Bulk, In>,
}

pub struct UsbBackend {
	transport: Mutex<Transport>,
	next_request_id: AtomicU64,
	connected: AtomicBool,
	device_id: DeviceKey,
	failed: mpsc::UnboundedSender<DeviceKey>,
}

impl UsbBackend {
	pub(crate) async fn connect(
		info: &DeviceInfo,
		device_id: DeviceKey,
		failed: mpsc::UnboundedSender<DeviceKey>,
	) -> io::Result<(Arc<Self>, NegotiatedConfig)> {
		let interface_number = info
			.interfaces()
			.find(|interface| {
				interface.class() == 0xff && interface.subclass() == 0 && interface.protocol() == 0
			})
			.map(|interface| interface.interface_number())
			.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "vendor interface not found"))?;
		let device = info.open().await.map_err(nusb_error)?;
		let interface = device
			.claim_interface(interface_number)
			.await
			.map_err(nusb_error)?;
		let descriptor = interface.descriptor().ok_or_else(|| {
			io::Error::new(io::ErrorKind::InvalidData, "active interface not found")
		})?;
		let mut out_address = None;
		let mut in_address = None;
		for endpoint in descriptor.endpoints() {
			if endpoint.transfer_type() != TransferType::Bulk {
				continue;
			}
			match endpoint.direction() {
				Direction::Out if out_address.replace(endpoint.address()).is_some() => {
					return Err(io::Error::new(
						io::ErrorKind::InvalidData,
						"multiple bulk OUT endpoints",
					));
				}
				Direction::In if in_address.replace(endpoint.address()).is_some() => {
					return Err(io::Error::new(
						io::ErrorKind::InvalidData,
						"multiple bulk IN endpoints",
					));
				}
				Direction::Out | Direction::In => {}
			}
		}
		let out_endpoint = interface
			.endpoint::<Bulk, Out>(out_address.ok_or_else(|| {
				io::Error::new(io::ErrorKind::NotFound, "bulk OUT endpoint not found")
			})?)
			.map_err(nusb_error)?;
		let mut in_endpoint = interface
			.endpoint::<Bulk, In>(in_address.ok_or_else(|| {
				io::Error::new(io::ErrorKind::NotFound, "bulk IN endpoint not found")
			})?)
			.map_err(nusb_error)?;
		// Replies completed after the previous host process released this interface
		// remain queued by the device and must not be mistaken for this session's HELLO.
		drain_stale_input(&mut in_endpoint).await?;
		let backend = Arc::new(Self {
			transport: Mutex::new(Transport {
				out_endpoint,
				in_endpoint,
			}),
			next_request_id: AtomicU64::new(2),
			connected: AtomicBool::new(true),
			device_id,
			failed,
		});
		let hello = backend
			.exchange(Operation::Hello, 1, Vec::new())
			.await
			.map_err(|failure| failure.error)?;
		if hello.header.status != Status::Ok {
			return Err(io::Error::other(format!(
				"HELLO failed with {:?}",
				hello.header.status
			)));
		}
		let capabilities = parse_capabilities(&hello.payload)?;
		Ok((backend, capabilities))
	}

	pub async fn configure_pins(
		&self,
		map: PinMap,
		generation: u64,
	) -> Result<NegotiatedConfig, ConfigureError> {
		map.validate().map_err(ConfigureError::Rejected)?;
		let mut payload = Vec::with_capacity(8 + PIN_MAP_LEN);
		payload.extend_from_slice(&generation.to_le_bytes());
		payload.extend_from_slice(&map.encode());
		let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
		let reply = self
			.exchange(Operation::ConfigurePins, request_id, payload)
			.await
			.map_err(|failure| ConfigureError::Transport(failure.error))?;
		let applied_map = match reply.header.status {
			Status::Ok => {
				if reply.payload.len() != 8 + PIN_MAP_LEN
					|| reply.payload[..8] != generation.to_le_bytes()
				{
					return Err(ConfigureError::Transport(invalid(
						"invalid configure-pins reply",
					)));
				}
				PinMap::decode(&reply.payload[8..])
					.map_err(|_| ConfigureError::Transport(invalid("invalid configured pin map")))?
			}
			Status::InvalidRoute | Status::PinConflict => {
				let error = PinValidationError::decode(&reply.payload)
					.map_err(|_| ConfigureError::Transport(invalid("invalid pin rejection")))?;
				return Err(ConfigureError::Rejected(error));
			}
			_ => {
				return Err(ConfigureError::Transport(io::Error::other(format!(
					"configure-pins failed with {:?}",
					reply.header.status
				))));
			}
		};
		let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
		let hello = self
			.exchange(Operation::Hello, request_id, Vec::new())
			.await
			.map_err(|failure| ConfigureError::Transport(failure.error))?;
		if hello.header.status != Status::Ok {
			return Err(ConfigureError::Transport(io::Error::other(format!(
				"HELLO after pin configuration failed with {:?}",
				hello.header.status
			))));
		}
		let negotiated = parse_capabilities(&hello.payload).map_err(ConfigureError::Transport)?;
		if negotiated.pin_map != applied_map || negotiated.generation != generation {
			return Err(ConfigureError::Transport(invalid(
				"firmware did not retain configured pin map",
			)));
		}
		Ok(negotiated)
	}

	fn fail(&self) {
		if self.connected.swap(false, Ordering::AcqRel) {
			let _ = self.failed.send(self.device_id.clone());
		}
	}

	async fn exchange(
		&self,
		operation: Operation,
		request_id: u64,
		payload: Vec<u8>,
	) -> Result<UsbReply, UsbFailure> {
		let header = UsbHeader {
			operation,
			reply: false,
			request_id,
			payload_len: payload.len() as u32,
			status: Status::Ok,
		};
		let mut frame = Vec::with_capacity(USB_HEADER_LEN + payload.len());
		frame.extend_from_slice(&header.encode());
		frame.extend_from_slice(&payload);
		let mut transport = self.transport.lock().await;
		match tokio::time::timeout(
			USB_DEADLINE,
			transfer(&mut transport, operation, request_id, frame),
		)
		.await
		{
			Ok(result) => result,
			Err(_) => Err(UsbFailure::new(
				Status::Timeout,
				io::Error::new(io::ErrorKind::TimedOut, "USB transaction timed out"),
			)),
		}
	}
}

#[async_trait]
impl Backend for UsbBackend {
	async fn transact(&self, transaction: Transaction) -> BackendReply {
		if !self.connected.load(Ordering::Acquire) {
			return BackendReply::error(Status::Disconnected);
		}
		let (operation, payload) = match encode_transaction(&transaction) {
			Ok(encoded) => encoded,
			Err(_) => return BackendReply::error(Status::BadFrame),
		};
		let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
		match self.exchange(operation, request_id, payload).await {
			Ok(reply) => match decode_reply(&transaction, reply) {
				Ok(reply) => {
					if reply.status == Status::Disconnected as i32 {
						self.fail();
					}
					reply
				}
				Err(_) => {
					self.fail();
					BackendReply::error(Status::BadFrame)
				}
			},
			Err(failure) => {
				self.fail();
				BackendReply::error(failure.status)
			}
		}
	}

	async fn disconnect(&self) {
		self.connected.store(false, Ordering::Release);
	}
}

async fn drain_stale_input(in_endpoint: &mut Endpoint<Bulk, In>) -> io::Result<()> {
	let packet_size = in_endpoint.max_packet_size();
	let max_packets = (USB_HEADER_LEN + MAX_PAYLOAD).div_ceil(packet_size) + 1;
	for _ in 0..max_packets {
		in_endpoint.submit(Buffer::new(packet_size));
		match tokio::time::timeout(USB_DRAIN_QUIET, in_endpoint.next_complete()).await {
			Ok(completion) => completion.status.map_err(io::Error::from)?,
			Err(_) => {
				in_endpoint.cancel_all();
				let completion = tokio::time::timeout(USB_DEADLINE, in_endpoint.next_complete())
					.await
					.map_err(|_| {
						io::Error::new(
							io::ErrorKind::TimedOut,
							"timed out cancelling USB input drain",
						)
					})?;
				match (completion.status, completion.actual_len) {
					(Err(nusb::transfer::TransferError::Cancelled), 0) => return Ok(()),
					(Err(nusb::transfer::TransferError::Cancelled), _) => continue,
					(Ok(()), _) => continue,
					(Err(error), _) => return Err(io::Error::from(error)),
				}
			}
		}
	}
	Err(invalid("too much stale USB input"))
}

pub(crate) struct UsbReply {
	pub(crate) header: UsbHeader,
	pub(crate) payload: Vec<u8>,
}

struct UsbFailure {
	status: Status,
	error: io::Error,
}

impl UsbFailure {
	fn new(status: Status, error: io::Error) -> Self {
		Self { status, error }
	}
}

async fn transfer(
	transport: &mut Transport,
	operation: Operation,
	request_id: u64,
	frame: Vec<u8>,
) -> Result<UsbReply, UsbFailure> {
	transport.out_endpoint.submit(frame.into());
	let completion = transport.out_endpoint.next_complete().await;
	let actual_len = completion.actual_len;
	let expected_len = completion.buffer.len();
	completion.status.map_err(transfer_error)?;
	if actual_len != expected_len {
		return Err(protocol_error("short USB OUT transfer"));
	}

	let packet_size = transport.in_endpoint.max_packet_size();
	let mut received = Vec::new();
	let mut expected = None;
	loop {
		transport.in_endpoint.submit(Buffer::new(packet_size));
		let packet = transport
			.in_endpoint
			.next_complete()
			.await
			.into_result()
			.map_err(transfer_error)?;
		if packet.is_empty() {
			return Err(protocol_error("empty USB packet before complete frame"));
		}
		received.extend_from_slice(&packet);
		if expected.is_none() && received.len() >= USB_HEADER_LEN {
			let header = UsbHeader::decode(&received[..USB_HEADER_LEN])
				.map_err(|_| protocol_error("invalid USB reply header"))?;
			if !header.reply || header.operation != operation || header.request_id != request_id {
				return Err(protocol_error("USB reply does not match request"));
			}
			expected = Some(USB_HEADER_LEN + header.payload_len as usize);
		}
		if let Some(expected) = expected {
			if received.len() > expected {
				return Err(protocol_error("extra bytes after USB frame"));
			}
			if received.len() == expected {
				let header = UsbHeader::decode(&received[..USB_HEADER_LEN])
					.map_err(|_| protocol_error("invalid USB reply header"))?;
				return Ok(UsbReply {
					header,
					payload: received.split_off(USB_HEADER_LEN),
				});
			}
		}
	}
}

pub(crate) fn encode_transaction(transaction: &Transaction) -> io::Result<(Operation, Vec<u8>)> {
	let mut payload = Vec::new();
	match transaction {
		Transaction::I2c { bus_id, messages } => {
			payload.push(*bus_id);
			payload.push(
				messages
					.len()
					.try_into()
					.map_err(|_| invalid("too many I2C messages"))?,
			);
			payload.extend_from_slice(&[0; 2]);
			for message in messages {
				let read = message.flags & rp_extio_wire::I2C_READ != 0;
				let length = if read {
					message.read_len
				} else {
					message
						.data
						.len()
						.try_into()
						.map_err(|_| invalid("I2C message too large"))?
				};
				payload.extend_from_slice(&message.address.to_le_bytes());
				payload.extend_from_slice(&message.flags.to_le_bytes());
				payload.extend_from_slice(&length.to_le_bytes());
				payload.extend_from_slice(&[0; 2]);
				if !read {
					payload.extend_from_slice(&message.data);
				}
			}
			if payload.len() > MAX_PAYLOAD {
				return Err(invalid("I2C transaction too large"));
			}
			Ok((Operation::I2cTransfer, payload))
		}
		Transaction::Spi {
			bus_id,
			chip_select,
			mode,
			segments,
		} => {
			payload.push(*bus_id);
			payload.push(*chip_select);
			payload.extend_from_slice(&mode.to_le_bytes());
			payload.push(
				segments
					.len()
					.try_into()
					.map_err(|_| invalid("too many SPI segments"))?,
			);
			payload.extend_from_slice(&[0; 3]);
			for segment in segments {
				let length: u16 = segment
					.tx
					.len()
					.try_into()
					.map_err(|_| invalid("SPI segment too large"))?;
				payload.extend_from_slice(&segment.speed_hz.to_le_bytes());
				payload.extend_from_slice(&length.to_le_bytes());
				payload.extend_from_slice(&segment.flags.to_le_bytes());
				payload.extend_from_slice(&segment.word_delay_ns.to_le_bytes());
				payload.extend_from_slice(&segment.delay_ns.to_le_bytes());
				payload.extend_from_slice(&segment.cs_change_delay_ns.to_le_bytes());
				payload.extend_from_slice(&segment.tx);
			}
			if payload.len() > MAX_PAYLOAD {
				return Err(invalid("SPI transaction too large"));
			}
			Ok((Operation::SpiMessage, payload))
		}
		Transaction::Uart {
			bus_id,
			baud,
			data_bits,
			stop_bits,
			parity,
			rx_len,
			tx,
		} => {
			let tx_len: u16 = tx
				.len()
				.try_into()
				.map_err(|_| invalid("UART write is too large"))?;
			payload.extend_from_slice(&[*bus_id, *data_bits, *stop_bits, *parity]);
			payload.extend_from_slice(&baud.to_le_bytes());
			payload.extend_from_slice(&rx_len.to_le_bytes());
			payload.extend_from_slice(&tx_len.to_le_bytes());
			payload.extend_from_slice(tx);
			if payload.len() > MAX_PAYLOAD {
				return Err(invalid("UART transaction is too large"));
			}
			Ok((Operation::UartTransfer, payload))
		}
		Transaction::Gpio {
			command,
			line,
			value,
		} => Ok((Operation::GpioCommand, vec![*command, *line, *value, 0])),
		Transaction::Adc { channel } => Ok((Operation::AdcRead, vec![*channel, 0, 0, 0])),
		Transaction::Pwm { command, .. } => Ok((
			Operation::Pwm,
			transaction
				.pwm_state()
				.expect("PWM transaction has state")
				.encode(*command)
				.to_vec(),
		)),
		Transaction::OnboardLed { command, .. } => Ok((
			Operation::OnboardLed,
			transaction
				.led_state()
				.expect("onboard LED transaction has state")
				.encode(*command)
				.to_vec(),
		)),
	}
}

pub(crate) fn decode_reply(transaction: &Transaction, reply: UsbReply) -> io::Result<BackendReply> {
	if reply.header.status != Status::Ok {
		if reply.payload.len() != 4 {
			return Err(invalid("USB error reply has trailing data"));
		}
		return Ok(BackendReply {
			status: reply.header.status as i32,
			completed: u32::from_le_bytes(reply.payload[..4].try_into().unwrap()),
			data: Vec::new(),
			effective_speeds: Vec::new(),
		});
	}
	match transaction {
		Transaction::I2c { messages, .. } => {
			if reply.payload.len() < 4 {
				return Err(invalid("USB I2C reply is missing completion count"));
			}
			let completed = u32::from_le_bytes(reply.payload[..4].try_into().unwrap());
			let expected: usize = messages
				.iter()
				.map(|message| message.read_len as usize)
				.sum();
			if completed != messages.len() as u32 || reply.payload.len() != 4 + expected {
				return Err(invalid("invalid successful I2C reply"));
			}
			Ok(BackendReply {
				status: Status::Ok as i32,
				completed,
				data: reply.payload[4..].to_vec(),
				effective_speeds: Vec::new(),
			})
		}
		Transaction::Spi { segments, .. } => {
			if reply.payload.len() < 4 {
				return Err(invalid("USB SPI reply is missing completion count"));
			}
			let completed = u32::from_le_bytes(reply.payload[..4].try_into().unwrap());
			let data_len: usize = segments.iter().map(|segment| segment.tx.len()).sum();
			let metadata_len = 8 + segments.len() * 4;
			if completed != data_len as u32
				|| reply.payload.len() != metadata_len + data_len
				|| reply.payload[4] as usize != segments.len()
				|| reply.payload[5..8] != [0; 3]
			{
				return Err(invalid("invalid successful SPI reply"));
			}
			let effective_speeds = reply.payload[8..metadata_len]
				.as_chunks::<4>()
				.0
				.iter()
				.map(|bytes| u32::from_le_bytes(*bytes))
				.collect();
			Ok(BackendReply {
				status: Status::Ok as i32,
				completed,
				data: reply.payload[metadata_len..].to_vec(),
				effective_speeds,
			})
		}
		Transaction::Uart { rx_len, tx, .. } => {
			if reply.payload.len() < rp_extio_wire::UART_REPLY_HEADER_LEN {
				return Err(invalid("UART reply is missing counts"));
			}
			let written = u16::from_le_bytes(reply.payload[..2].try_into().unwrap());
			let read = u16::from_le_bytes(reply.payload[2..4].try_into().unwrap());
			if written as usize > tx.len()
				|| read > *rx_len
				|| reply.payload.len() != rp_extio_wire::UART_REPLY_HEADER_LEN + read as usize
			{
				return Err(invalid("invalid successful UART reply"));
			}
			Ok(BackendReply {
				status: Status::Ok as i32,
				completed: written.into(),
				data: reply.payload[rp_extio_wire::UART_REPLY_HEADER_LEN..].to_vec(),
				effective_speeds: Vec::new(),
			})
		}
		Transaction::Gpio { command, .. } => {
			if reply.payload.len() < 4 {
				return Err(invalid("USB GPIO reply is missing completion count"));
			}
			let completed = u32::from_le_bytes(reply.payload[..4].try_into().unwrap());
			if *command == rp_extio_wire::GPIO_SNAPSHOT {
				if reply.payload.len() != 20 {
					return Err(invalid("invalid GPIO snapshot reply"));
				}
				return Ok(BackendReply {
					status: Status::Ok as i32,
					completed,
					data: reply.payload[4..20].to_vec(),
					effective_speeds: Vec::new(),
				});
			}
			if completed != 1
				|| reply.payload.len() != 8
				|| reply.payload[4]
					> if *command == rp_extio_wire::GPIO_SET_BIAS {
						rp_extio_wire::GPIO_BIAS_PULL_DOWN
					} else {
						1
					} || reply.payload[5..8] != [0; 3]
			{
				return Err(invalid("invalid successful GPIO reply"));
			}
			Ok(BackendReply {
				status: Status::Ok as i32,
				completed,
				data: vec![reply.payload[4]],
				effective_speeds: Vec::new(),
			})
		}
		Transaction::Adc { channel } => {
			if reply.payload.len() != 4 || reply.payload[2] != 12 || reply.payload[3] != *channel {
				return Err(invalid("invalid ADC reply"));
			}
			Ok(BackendReply {
				status: Status::Ok as i32,
				completed: 1,
				data: reply.payload[..2].to_vec(),
				effective_speeds: Vec::new(),
			})
		}
		Transaction::Pwm { command, .. } => {
			let (reply_command, state) =
				PwmState::decode(&reply.payload).map_err(|_| invalid("invalid PWM reply"))?;
			if reply_command != *command
				|| Some(state.channel) != transaction.pwm_state().map(|state| state.channel)
			{
				return Err(invalid("mismatched PWM reply"));
			}
			Ok(BackendReply {
				status: Status::Ok as i32,
				completed: 1,
				data: state.encode(reply_command).to_vec(),
				effective_speeds: Vec::new(),
			})
		}
		Transaction::OnboardLed { command, .. } => {
			let (reply_command, state) = LedState::decode(&reply.payload)
				.map_err(|_| invalid("invalid onboard LED reply"))?;
			if reply_command != *command
				|| Some(state.index) != transaction.led_state().map(|state| state.index)
			{
				return Err(invalid("mismatched onboard LED reply"));
			}
			Ok(BackendReply {
				status: Status::Ok as i32,
				completed: 1,
				data: state.encode(reply_command).to_vec(),
				effective_speeds: Vec::new(),
			})
		}
	}
}

pub(crate) fn parse_capabilities(payload: &[u8]) -> io::Result<NegotiatedConfig> {
	let mut offset = 0;
	let mut generation = None;
	let mut board = None;
	let mut i2c: [Option<(u32, u16, [u8; 2])>; 2] = [None; 2];
	let mut spi: [Option<(u32, u32, u32, u16, Vec<u8>)>; 2] = [None, None];
	let mut uart: [Option<([u8; 2], u8, u32, u32)>; 2] = [None; 2];
	let mut pin_map = None;
	let mut gpio_pins: Option<Vec<u8>> = None;
	let mut adc = None;
	let mut pwm = None;
	let mut clocks = None;
	let mut onboard_led = None;
	while offset < payload.len() {
		if payload.len() - offset < 4 {
			return Err(invalid("truncated capability record"));
		}
		let kind = u16::from_le_bytes(payload[offset..offset + 2].try_into().unwrap());
		let length =
			u16::from_le_bytes(payload[offset + 2..offset + 4].try_into().unwrap()) as usize;
		offset += 4;
		let value = payload
			.get(offset..offset + length)
			.ok_or_else(|| invalid("truncated capability value"))?;
		offset += length;
		match kind {
			CAP_GENERAL => {
				if generation.is_some()
					|| value.len() != 16
					|| value[0] > rp_extio_wire::VERSION_MAJOR
					|| value[1] < rp_extio_wire::VERSION_MAJOR
					|| value[3] != 0
					|| u32::from_le_bytes(value[4..8].try_into().unwrap()) < MAX_PAYLOAD as u32
				{
					return Err(invalid("incompatible general capability"));
				}
				generation = Some(u64::from_le_bytes(value[8..16].try_into().unwrap()));
			}
			CAP_BOARD => {
				let identity_len = value.get(3).copied().unwrap_or(0) as usize;
				if board.is_some()
					|| value.len() != 12 + identity_len
					|| value.first().copied() != Some(BOARD_CAPABILITY_VERSION)
				{
					return Err(invalid("invalid board capability"));
				}
				let kind =
					BoardKind::try_from(value[1]).map_err(|_| invalid("unsupported board kind"))?;
				let transport = TransportKind::try_from(value[2])
					.map_err(|_| invalid("unsupported transport kind"))?;
				let profile = kind.profile();
				let supported = u64::from_le_bytes(value[4..12].try_into().unwrap());
				if transport != profile.transport() || supported != profile.supported_signals() {
					return Err(invalid("board capability does not match known profile"));
				}
				let identity = DeviceIdentity::new(&value[12..])
					.map_err(|_| invalid("invalid board identity"))?;
				board = Some((kind, transport, identity));
			}
			CAP_I2C => {
				let bus = value.first().copied().unwrap_or(u8::MAX) as usize;
				let flags = value
					.get(2..4)
					.map(|bytes| u16::from_le_bytes(bytes.try_into().unwrap()))
					.unwrap_or(0);
				if value.len() != 16
					|| bus >= i2c.len()
					|| i2c[bus].is_some()
					|| value[1] != 0
					|| flags & rp_extio_wire::I2C_READ == 0
					|| flags & !(rp_extio_wire::I2C_READ | rp_extio_wire::I2C_TEN_BIT) != 0
					|| value[14..16] != [0; 2]
				{
					return Err(invalid("invalid I2C capability"));
				}
				let minimum = u32::from_le_bytes(value[4..8].try_into().unwrap());
				let maximum = u32::from_le_bytes(value[8..12].try_into().unwrap());
				if minimum == 0 || minimum > maximum {
					return Err(invalid("invalid I2C speed range"));
				}
				i2c[bus] = Some((maximum, flags, [value[12], value[13]]));
			}
			CAP_SPI => {
				let bus = value.first().copied().unwrap_or(u8::MAX) as usize;
				if value.len() < 16
					|| bus >= spi.len()
					|| spi[bus].is_some()
					|| !(1..=2).contains(&value[1])
					|| value.len() != 15 + value[1] as usize
				{
					return Err(invalid("invalid SPI capability"));
				}
				let mode_bits = u16::from_le_bytes(value[2..4].try_into().unwrap()) as u32;
				let minimum = u32::from_le_bytes(value[4..8].try_into().unwrap());
				let maximum = u32::from_le_bytes(value[8..12].try_into().unwrap());
				if mode_bits & !0x4f != 0 || minimum == 0 || minimum > maximum {
					return Err(invalid("invalid SPI capability values"));
				}
				spi[bus] = Some((
					mode_bits,
					minimum,
					maximum,
					value[1] as u16,
					value[12..].to_vec(),
				));
			}
			CAP_UART => {
				let bus = value.first().copied().unwrap_or(u8::MAX) as usize;
				let flags = value.get(3).copied().unwrap_or(u8::MAX);
				let default_baud = value
					.get(4..8)
					.map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
					.unwrap_or(0);
				let max_baud = value
					.get(8..12)
					.map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
					.unwrap_or(0);
				if value.len() != 12
					|| bus >= uart.len()
					|| uart[bus].is_some()
					|| flags & !rp_extio_wire::UART_CAP_DAEMON_BACKED != 0
					|| default_baud == 0
					|| (flags & rp_extio_wire::UART_CAP_DAEMON_BACKED != 0
						&& max_baud < default_baud)
					|| (flags == 0 && max_baud != 0)
				{
					return Err(invalid("invalid UART capability"));
				}
				uart[bus] = Some(([value[1], value[2]], flags, default_baud, max_baud));
			}
			CAP_PIN_CONFIG => {
				if pin_map.is_some() {
					return Err(invalid("duplicate pin configuration capability"));
				}
				pin_map = Some(
					PinMap::decode(value)
						.map_err(|_| invalid("invalid pin configuration capability"))?,
				);
			}
			CAP_GPIO => {
				if gpio_pins.is_some()
					|| value.len() < 4
					|| value[0] == 0
					|| value[0] as usize > MAX_GPIO_LINES
					|| value[1..4] != [0; 3]
					|| value.len() != 4 + value[0] as usize
				{
					return Err(invalid("invalid GPIO capability"));
				}
				gpio_pins = Some(value[4..].to_vec());
			}
			CAP_ADC => {
				let channels = value.first().copied().unwrap_or(0) as usize;
				if adc.is_some()
					|| channels == 0
					|| channels > rp_extio_wire::ADC_CHANNEL_COUNT
					|| value.len() != 4 + channels
					|| value[1] == 0
					|| u16::from_le_bytes(value[2..4].try_into().unwrap()) == 0
				{
					return Err(invalid("invalid ADC capability"));
				}
				adc = Some((
					value[1],
					u16::from_le_bytes(value[2..4].try_into().unwrap()),
					value[4..].to_vec(),
				));
			}
			CAP_PWM => {
				let channels = value.first().copied().unwrap_or(0) as usize;
				if pwm.is_some()
					|| channels == 0
					|| channels > rp_extio_wire::PWM_CHANNEL_COUNT
					|| value.len() != 7 + channels
				{
					return Err(invalid("invalid PWM capability"));
				}
				pwm = Some((
					u32::from_le_bytes(value[1..5].try_into().unwrap()),
					u16::from_le_bytes(value[5..7].try_into().unwrap()),
					value[7..].to_vec(),
				));
			}
			CAP_CLOCK => {
				if clocks.is_some()
					|| value.len() != 14
					|| value[0] as usize != rp_extio_wire::CLOCK_OUTPUT_COUNT
				{
					return Err(invalid("invalid clock capability"));
				}
				clocks = Some(value.to_vec());
			}
			CAP_ONBOARD_LED => {
				if onboard_led.is_some() || value.len() != 4 || value[0] != 1 || value[3] != 0 {
					return Err(invalid("invalid onboard LED capability"));
				}
				onboard_led = Some(OnboardLed {
					kind: OnboardLedKind::try_from(value[1])
						.map_err(|_| invalid("unsupported onboard LED kind"))?,
					pin: value[2],
				});
			}
			_ => {}
		}
	}
	let gpio_pins = gpio_pins.ok_or_else(|| invalid("GPIO capability missing"))?;
	let generation = generation.ok_or_else(|| invalid("general capability missing"))?;
	let (board_kind, transport, identity) =
		board.ok_or_else(|| invalid("board capability missing"))?;
	let profile = board_kind.profile();
	let pin_map = pin_map.ok_or_else(|| invalid("pin configuration capability missing"))?;
	let (adc_bits, adc_reference_mv, adc_pins) =
		adc.ok_or_else(|| invalid("ADC capability missing"))?;
	let (pwm_clock_hz, pwm_enabled, pwm_pins) =
		pwm.ok_or_else(|| invalid("PWM capability missing"))?;
	let clocks = clocks.ok_or_else(|| invalid("clock capability missing"))?;
	if onboard_led.is_some_and(|led| led != profile.onboard_led()) {
		return Err(invalid(
			"onboard LED capability does not match board profile",
		));
	}
	pin_map
		.validate_for(profile)
		.map_err(|_| invalid("invalid peripheral pin map"))?;
	for bus in 0..2 {
		let expected_i2c = pin_map.is_enabled([Peripheral::I2c0, Peripheral::I2c1][bus]);
		let expected_spi = pin_map.is_enabled([Peripheral::Spi0, Peripheral::Spi1][bus]);
		let expected_uart = pin_map.is_enabled([Peripheral::Uart0, Peripheral::Uart1][bus]);
		if expected_i2c != i2c[bus].is_some()
			|| expected_spi != spi[bus].is_some()
			|| expected_uart != uart[bus].is_some()
		{
			return Err(invalid("capabilities do not match enabled peripherals"));
		}
		if i2c[bus].is_some_and(|(_, _, pins)| pins != [pin_map.i2c_sda[bus], pin_map.i2c_scl[bus]])
			|| spi[bus].as_ref().is_some_and(|(_, _, _, count, pins)| {
				let mut expected = vec![
					pin_map.spi_sck[bus],
					pin_map.spi_mosi[bus],
					pin_map.spi_miso[bus],
					pin_map.spi_chip_selects[bus][0],
				];
				if pin_map.spi_cs1_enabled(bus) {
					expected.push(pin_map.spi_chip_selects[bus][1]);
				}
				*count != u16::from(pin_map.spi_chip_select_count(bus)) || *pins != expected
			}) || uart[bus]
			.is_some_and(|(pins, _, _, _)| pins != [pin_map.uart_tx[bus], pin_map.uart_rx[bus]])
		{
			return Err(invalid("capability pins do not match pin configuration"));
		}
	}
	let expected_gpio = pin_map.gpio_pins_for(profile).unwrap();
	if expected_gpio.as_slice() != gpio_pins.as_slice() {
		return Err(invalid("GPIO capability does not match peripheral pins"));
	}
	if adc_pins.len() != profile.adc_channel_count()
		|| adc_pins.as_slice() != &pin_map.adc_pins[..profile.adc_channel_count()]
		|| pwm_pins.len() != profile.pwm_channel_count()
		|| pwm_enabled != pin_map.pwm_enabled
		|| pwm_pins.as_slice() != &pin_map.pwm_pins[..profile.pwm_channel_count()]
		|| pwm_clock_hz == 0
		|| clocks[1] != pin_map.clock_enabled
	{
		return Err(invalid(
			"auxiliary capabilities do not match pin configuration",
		));
	}
	for output in 0..rp_extio_wire::CLOCK_OUTPUT_COUNT {
		let offset = 2 + output * 6;
		if clocks[offset] != pin_map.clock_pins[output]
			|| clocks[offset + 1] != pin_map.clock_sources[output] as u8
			|| u32::from_le_bytes(clocks[offset + 2..offset + 6].try_into().unwrap())
				!= pin_map.clock_hz[output]
		{
			return Err(invalid("clock capability does not match pin configuration"));
		}
	}
	let mut gpio_array = [0; MAX_GPIO_LINES];
	gpio_array[..gpio_pins.len()].copy_from_slice(&gpio_pins);
	Ok(NegotiatedConfig {
		board_kind,
		transport,
		identity,
		onboard_led,
		registration: RegistrationConfig {
			i2c: i2c.map(|capability| {
				capability.map(|(speed_hz, flags, _)| I2cRegistration {
					speed_hz,
					ten_bit_addresses: flags & rp_extio_wire::I2C_TEN_BIT != 0,
				})
			}),
			spi: spi.map(|capability| {
				capability.map(
					|(spi_mode_bits, spi_min_speed_hz, spi_max_speed_hz, spi_chip_selects, _)| {
						SpiRegistration {
							spi_mode_bits,
							spi_min_speed_hz,
							spi_max_speed_hz,
							spi_chip_selects,
						}
					},
				)
			}),
			uart: uart.map(|capability| {
				capability.and_then(|(_, flags, default_baud, max_baud)| {
					(flags & rp_extio_wire::UART_CAP_DAEMON_BACKED != 0).then_some(
						UartRegistration {
							default_baud,
							max_baud,
						},
					)
				})
			}),
			gpio_lines: gpio_pins.len() as u16,
			gpio_pins: gpio_array,
			adc: Some(crate::kernel::AdcRegistration {
				channels: profile.adc_channel_count() as u8,
				resolution_bits: adc_bits,
				reference_mv: adc_reference_mv,
			}),
			pwm: Some(crate::kernel::PwmRegistration {
				channels: profile.pwm_channel_count() as u8,
				enabled: pwm_enabled,
				clock_hz: pwm_clock_hz,
			}),
		},
		pin_map,
		generation,
	})
}

fn nusb_error(error: nusb::Error) -> io::Error {
	io::Error::other(error)
}

fn transfer_error(error: nusb::transfer::TransferError) -> UsbFailure {
	UsbFailure::new(Status::Disconnected, io::Error::from(error))
}

fn protocol_error(message: &'static str) -> UsbFailure {
	UsbFailure::new(Status::BadFrame, invalid(message))
}

fn invalid(message: &'static str) -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::backend::{I2cMessage, SpiSegment};
	use crate::sim::{Command as SimCommand, Response as SimResponse, Simulator};

	#[test]
	fn repeated_serial_probe_errors_are_suppressed_until_they_change() {
		let path = Path::new("/dev/serial/by-id/example");
		let mut errors = HashMap::new();
		assert!(remember_serial_probe_error(&mut errors, path, "timeout"));
		assert!(!remember_serial_probe_error(&mut errors, path, "timeout"));
		assert!(remember_serial_probe_error(&mut errors, path, "bad frame"));
	}

	#[tokio::test]
	async fn serial_probe_tasks_are_deduplicated_by_path() {
		let (failed, _failed_rx) = mpsc::unbounded_channel();
		let (results, mut result_rx) = mpsc::unbounded_channel();
		let mut in_flight = HashSet::new();
		let path = PathBuf::from("/dev/rp-extio-definitely-missing-for-test");
		start_serial_probe(path.clone(), &failed, &results, &mut in_flight);
		start_serial_probe(path.clone(), &failed, &results, &mut in_flight);
		assert_eq!(in_flight, HashSet::from([path.clone()]));

		let (result_path, result) = tokio::time::timeout(Duration::from_secs(1), result_rx.recv())
			.await
			.unwrap()
			.unwrap();
		assert_eq!(result_path, path);
		assert!(result.is_err());
		assert!(result_rx.try_recv().is_err());
	}

	fn capability_record(payload: &mut Vec<u8>, kind: u16, value: &[u8]) {
		payload.extend_from_slice(&kind.to_le_bytes());
		payload.extend_from_slice(&(value.len() as u16).to_le_bytes());
		payload.extend_from_slice(value);
	}

	fn capabilities(map: PinMap) -> Vec<u8> {
		let mut payload = Vec::new();
		let mut general = vec![
			rp_extio_wire::VERSION_MAJOR,
			rp_extio_wire::VERSION_MAJOR,
			rp_extio_wire::VERSION_MINOR,
			0,
		];
		general.extend_from_slice(&(MAX_PAYLOAD as u32).to_le_bytes());
		general.extend_from_slice(&0_u64.to_le_bytes());
		capability_record(&mut payload, CAP_GENERAL, &general);
		let profile = BoardKind::Pico2.profile();
		let identity = b"TEST";
		let mut board = vec![
			BOARD_CAPABILITY_VERSION,
			BoardKind::Pico2 as u8,
			TransportKind::UsbBulk as u8,
			identity.len() as u8,
		];
		board.extend_from_slice(&profile.supported_signals().to_le_bytes());
		board.extend_from_slice(identity);
		capability_record(&mut payload, CAP_BOARD, &board);
		let led = profile.onboard_led();
		capability_record(
			&mut payload,
			CAP_ONBOARD_LED,
			&[1, led.kind as u8, led.pin, 0],
		);
		for bus in 0..2 {
			if map.is_enabled([Peripheral::I2c0, Peripheral::I2c1][bus]) {
				let mut value = vec![bus as u8, 0];
				value.extend_from_slice(&3_u16.to_le_bytes());
				value.extend_from_slice(&100_000_u32.to_le_bytes());
				value.extend_from_slice(&100_000_u32.to_le_bytes());
				value.extend_from_slice(&[map.i2c_sda[bus], map.i2c_scl[bus], 0, 0]);
				capability_record(&mut payload, CAP_I2C, &value);
			}
			if map.is_enabled([Peripheral::Spi0, Peripheral::Spi1][bus]) {
				let chip_selects = map.spi_chip_select_count(bus);
				let mut value = vec![bus as u8, chip_selects];
				value.extend_from_slice(&0x4f_u16.to_le_bytes());
				value.extend_from_slice(&50_000_u32.to_le_bytes());
				value.extend_from_slice(&20_000_000_u32.to_le_bytes());
				value.extend_from_slice(&[
					map.spi_sck[bus],
					map.spi_mosi[bus],
					map.spi_miso[bus],
					map.spi_chip_selects[bus][0],
				]);
				if map.spi_cs1_enabled(bus) {
					value.push(map.spi_chip_selects[bus][1]);
				}
				capability_record(&mut payload, CAP_SPI, &value);
			}
			if map.is_enabled([Peripheral::Uart0, Peripheral::Uart1][bus]) {
				let mut value = vec![bus as u8, map.uart_tx[bus], map.uart_rx[bus], 0];
				value.extend_from_slice(&115_200_u32.to_le_bytes());
				value.extend_from_slice(&0_u32.to_le_bytes());
				capability_record(&mut payload, CAP_UART, &value);
			}
		}
		capability_record(&mut payload, CAP_PIN_CONFIG, &map.encode());
		let gpio = map.gpio_pins().unwrap();
		let mut value = vec![gpio.len() as u8, 0, 0, 0];
		value.extend_from_slice(gpio.as_slice());
		capability_record(&mut payload, CAP_GPIO, &value);
		let mut value = vec![profile.adc_channel_count() as u8, 12];
		value.extend_from_slice(&3_300_u16.to_le_bytes());
		value.extend_from_slice(&map.adc_pins[..profile.adc_channel_count()]);
		capability_record(&mut payload, CAP_ADC, &value);
		let mut value = vec![profile.pwm_channel_count() as u8];
		value.extend_from_slice(&150_000_000_u32.to_le_bytes());
		value.extend_from_slice(&map.pwm_enabled.to_le_bytes());
		value.extend_from_slice(&map.pwm_pins[..profile.pwm_channel_count()]);
		capability_record(&mut payload, CAP_PWM, &value);
		let mut value = vec![rp_extio_wire::CLOCK_OUTPUT_COUNT as u8, map.clock_enabled];
		for output in 0..rp_extio_wire::CLOCK_OUTPUT_COUNT {
			value.push(map.clock_pins[output]);
			value.push(map.clock_sources[output] as u8);
			value.extend_from_slice(&map.clock_hz[output].to_le_bytes());
		}
		capability_record(&mut payload, CAP_CLOCK, &value);
		payload
	}

	#[test]
	fn parses_firmware_capabilities() {
		assert_eq!(
			parse_capabilities(&capabilities(rp_extio_wire::DEFAULT_PIN_MAP)).unwrap(),
			NegotiatedConfig {
				board_kind: BoardKind::Pico2,
				transport: TransportKind::UsbBulk,
				identity: DeviceIdentity::new(b"TEST").unwrap(),
				onboard_led: Some(BoardKind::Pico2.profile().onboard_led()),
				registration: RegistrationConfig::PICO2,
				pin_map: rp_extio_wire::DEFAULT_PIN_MAP,
				generation: 0,
			}
		);
	}

	#[test]
	fn parses_all_enabled_and_all_disabled_capabilities() {
		let mut map = rp_extio_wire::DEFAULT_PIN_MAP;
		map.enabled = 0x3f;
		let all_enabled = parse_capabilities(&capabilities(map)).unwrap();
		assert!(
			all_enabled
				.registration
				.i2c
				.into_iter()
				.all(|bus| bus.is_some())
		);
		assert!(
			all_enabled
				.registration
				.spi
				.into_iter()
				.all(|bus| bus.is_some())
		);
		assert_eq!(all_enabled.registration.gpio_lines, 8);

		map.enabled = 0;
		let all_disabled = parse_capabilities(&capabilities(map)).unwrap();
		assert_eq!(all_disabled.registration.i2c, [None; 2]);
		assert_eq!(all_disabled.registration.spi, [None; 2]);
		assert_eq!(all_disabled.registration.gpio_lines, 26);
	}

	#[test]
	fn parses_spi_with_only_cs0_enabled() {
		let mut map = rp_extio_wire::DEFAULT_PIN_MAP;
		map.set_spi_cs1_enabled(0, false);
		let negotiated = parse_capabilities(&capabilities(map)).unwrap();
		assert_eq!(negotiated.registration.spi[0].unwrap().spi_chip_selects, 1);
		assert!(
			negotiated.registration.gpio_pins[..negotiated.registration.gpio_lines as usize]
				.contains(&map.spi_chip_selects[0][1])
		);
	}

	#[test]
	fn transaction_and_reply_codecs_preserve_bus_data() {
		let transaction = Transaction::I2c {
			bus_id: 0,
			messages: vec![
				I2cMessage {
					address: 0x50,
					flags: 0,
					data: vec![0xaa],
					read_len: 0,
				},
				I2cMessage {
					address: 0x50,
					flags: 1,
					data: vec![],
					read_len: 2,
				},
			],
		};
		let (operation, payload) = encode_transaction(&transaction).unwrap();
		assert_eq!(operation, Operation::I2cTransfer);
		assert_eq!(&payload[..4], &[0, 2, 0, 0]);
		let reply = decode_reply(
			&transaction,
			UsbReply {
				header: UsbHeader {
					operation,
					reply: true,
					request_id: 7,
					payload_len: 6,
					status: Status::Ok,
				},
				payload: vec![2, 0, 0, 0, 3, 4],
			},
		)
		.unwrap();
		assert_eq!(reply.data, [3, 4]);

		let spi = Transaction::Spi {
			bus_id: 0,
			chip_select: 1,
			mode: 3,
			segments: vec![SpiSegment {
				speed_hz: 1_000_000,
				flags: 0,
				word_delay_ns: 1,
				delay_ns: 2,
				cs_change_delay_ns: 3,
				tx: vec![0xaa],
			}],
		};
		assert_eq!(encode_transaction(&spi).unwrap().0, Operation::SpiMessage);
	}

	#[test]
	fn gpio_transaction_and_reply_codecs_validate_commands_and_snapshots() {
		let command = Transaction::Gpio {
			command: rp_extio_wire::GPIO_GET,
			line: 4,
			value: 0,
		};
		assert_eq!(
			encode_transaction(&command).unwrap(),
			(
				Operation::GpioCommand,
				vec![rp_extio_wire::GPIO_GET, 4, 0, 0]
			)
		);
		let reply = decode_reply(
			&command,
			UsbReply {
				header: UsbHeader {
					operation: Operation::GpioCommand,
					reply: true,
					request_id: 8,
					payload_len: 8,
					status: Status::Ok,
				},
				payload: vec![1, 0, 0, 0, 1, 0, 0, 0],
			},
		)
		.unwrap();
		assert_eq!(reply.completed, 1);
		assert_eq!(reply.data, [1]);

		let snapshot = Transaction::Gpio {
			command: rp_extio_wire::GPIO_SNAPSHOT,
			line: 0,
			value: 0,
		};
		let reply = decode_reply(
			&snapshot,
			UsbReply {
				header: UsbHeader {
					operation: Operation::GpioCommand,
					reply: true,
					request_id: 9,
					payload_len: 20,
					status: Status::Ok,
				},
				payload: vec![
					17, 0, 0, 0, // line count
					0x34, 0x12, 0, 0, // levels
					4, 0, 0, 0, // outputs
					8, 0, 0, 0, // pull-ups
					16, 0, 0, 0, // pull-downs
				],
			},
		)
		.unwrap();
		assert_eq!(reply.completed, 17);
		assert_eq!(
			reply.data,
			[0x34, 0x12, 0, 0, 4, 0, 0, 0, 8, 0, 0, 0, 16, 0, 0, 0]
		);

		let malformed = UsbReply {
			header: UsbHeader {
				operation: Operation::GpioCommand,
				reply: true,
				request_id: 10,
				payload_len: 8,
				status: Status::Ok,
			},
			payload: vec![1, 0, 0, 0, 1, 1, 0, 0],
		};
		assert!(decode_reply(&command, malformed).is_err());
	}

	#[tokio::test]
	async fn applies_persisted_gpio_and_pwm_startup_state() {
		let simulator = Arc::new(Simulator::default());
		assert!(matches!(
			simulator
				.command(SimCommand::Create {
					device: "startup".into(),
				})
				.await,
			SimResponse::Ok
		));
		let backend = simulator.device("startup");
		let mut map = rp_extio_wire::DEFAULT_PIN_MAP;
		map.pwm_enabled |= (1 << 6) | (1 << 7);
		map.pwm_pins[6] = 22;
		apply_gpio_startup(
			&backend,
			&map,
			BoardKind::Pico2.profile(),
			&[
				GpioStartup::Output {
					pin: 20,
					value: true,
				},
				GpioStartup::Input {
					pin: 28,
					bias: GpioBias::PullDown,
				},
			],
		)
		.await
		.unwrap();
		for channel in [6, 7] {
			let seeded = backend
				.transact(Transaction::Pwm {
					command: rp_extio_wire::PWM_APPLY,
					channel,
					enabled: true,
					inverted: false,
					period_ns: 2_000_000,
					duty_ns: 1_000_000,
				})
				.await;
			assert_eq!(seeded.status, Status::Ok as i32);
		}
		apply_pwm_startup(
			&backend,
			&map,
			BoardKind::Pico2.profile(),
			&[
				PwmInfo {
					channel: 6,
					enabled: true,
					inverted: true,
					period_ns: 1_000_000,
					duty_ns: 250_000,
				},
				PwmInfo {
					channel: 7,
					enabled: true,
					inverted: false,
					period_ns: 1_000_000,
					duty_ns: 750_000,
				},
			],
		)
		.await
		.unwrap();

		let snapshot = backend
			.transact(Transaction::Gpio {
				command: rp_extio_wire::GPIO_SNAPSHOT,
				line: 0,
				value: 0,
			})
			.await;
		let output_line = gpio_line_for_map(&map, BoardKind::Pico2.profile(), 20).unwrap();
		let input_line = gpio_line_for_map(&map, BoardKind::Pico2.profile(), 28).unwrap();
		let levels = u32::from_le_bytes(snapshot.data[0..4].try_into().unwrap());
		let outputs = u32::from_le_bytes(snapshot.data[4..8].try_into().unwrap());
		let pull_downs = u32::from_le_bytes(snapshot.data[12..16].try_into().unwrap());
		assert_ne!(levels & (1 << output_line), 0);
		assert_ne!(outputs & (1 << output_line), 0);
		assert_eq!(outputs & (1 << input_line), 0);
		assert_ne!(pull_downs & (1 << input_line), 0);
		let pwm = backend
			.transact(Transaction::Pwm {
				command: rp_extio_wire::PWM_GET,
				channel: 6,
				enabled: false,
				inverted: false,
				period_ns: 0,
				duty_ns: 0,
			})
			.await;
		assert_eq!(
			rp_extio_wire::PwmState::decode(&pwm.data).unwrap().1,
			rp_extio_wire::PwmState {
				channel: 6,
				enabled: true,
				inverted: true,
				period_ns: 1_000_000,
				duty_ns: 250_000,
			}
		);
		let sibling = backend
			.transact(Transaction::Pwm {
				command: rp_extio_wire::PWM_GET,
				channel: 7,
				enabled: false,
				inverted: false,
				period_ns: 0,
				duty_ns: 0,
			})
			.await;
		assert_eq!(
			rp_extio_wire::PwmState::decode(&sibling.data).unwrap().1,
			rp_extio_wire::PwmState {
				channel: 7,
				enabled: true,
				inverted: false,
				period_ns: 1_000_000,
				duty_ns: 750_000,
			}
		);
	}

	#[test]
	fn adc_and_pwm_transaction_codecs_preserve_results() {
		let adc = Transaction::Adc { channel: 2 };
		assert_eq!(
			encode_transaction(&adc).unwrap(),
			(Operation::AdcRead, vec![2, 0, 0, 0])
		);
		let adc_reply = decode_reply(
			&adc,
			UsbReply {
				header: UsbHeader {
					operation: Operation::AdcRead,
					reply: true,
					request_id: 11,
					payload_len: 4,
					status: Status::Ok,
				},
				payload: vec![0x34, 0x0a, 12, 2],
			},
		)
		.unwrap();
		assert_eq!(adc_reply.data, [0x34, 0x0a]);

		let pwm = Transaction::Pwm {
			command: rp_extio_wire::PWM_APPLY,
			channel: 3,
			enabled: true,
			inverted: false,
			period_ns: 20_000_000,
			duty_ns: 1_500_000,
		};
		let (operation, payload) = encode_transaction(&pwm).unwrap();
		assert_eq!(operation, Operation::Pwm);
		assert_eq!(
			rp_extio_wire::PwmState::decode(&payload).unwrap().1,
			pwm.pwm_state().unwrap()
		);
		let pwm_reply = decode_reply(
			&pwm,
			UsbReply {
				header: UsbHeader {
					operation,
					reply: true,
					request_id: 12,
					payload_len: rp_extio_wire::PWM_STATE_LEN as u32,
					status: Status::Ok,
				},
				payload,
			},
		)
		.unwrap();
		assert_eq!(pwm_reply.data.len(), rp_extio_wire::PWM_STATE_LEN);
	}

	#[test]
	fn rejects_missing_and_malformed_capabilities() {
		assert!(parse_capabilities(&[]).is_err());
		assert!(parse_capabilities(&[1, 0, 8, 0, 1]).is_err());

		let mut payload = Vec::new();
		payload.extend_from_slice(&CAP_GPIO.to_le_bytes());
		payload.extend_from_slice(&21_u16.to_le_bytes());
		payload.extend_from_slice(&[17, 0, 0, 0]);
		let mut pins = rp_extio_firmware_gpio_pins();
		pins[0] = 0;
		payload.extend_from_slice(&pins);
		assert!(parse_capabilities(&payload).is_err());
	}

	fn rp_extio_firmware_gpio_pins() -> [u8; 17] {
		[2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 20, 22, 26, 27, 28]
	}

	#[tokio::test]
	#[ignore = "requires a connected rp-extio Pico 2"]
	async fn negotiates_connected_bridge_and_reads_gpio_snapshot() {
		let info = nusb::list_devices()
			.await
			.unwrap()
			.find(matches_bridge)
			.expect("rp-extio Pico 2 not found");
		let (failed, _failures) = mpsc::unbounded_channel();
		let (backend, config) = UsbBackend::connect(&info, DeviceKey::Usb(info.id()), failed)
			.await
			.unwrap();
		assert_eq!(config.registration.i2c, RegistrationConfig::PICO2.i2c);
		assert_eq!(config.registration.spi[0].unwrap().spi_chip_selects, 2);
		assert_eq!(config.registration.gpio_lines, 17);

		let snapshot = backend
			.transact(Transaction::Gpio {
				command: rp_extio_wire::GPIO_SNAPSHOT,
				line: 0,
				value: 0,
			})
			.await;
		assert_eq!(snapshot.status, Status::Ok as i32);
		assert_eq!(snapshot.completed, 17);
		assert_eq!(snapshot.data.len(), 16);
		backend.disconnect().await;
	}
}
