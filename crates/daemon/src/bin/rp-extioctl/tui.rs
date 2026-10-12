use super::Client;
use crossterm::{
	event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
	execute,
	terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
	Frame, Terminal,
	backend::CrosstermBackend,
	layout::{Constraint, Direction, Layout, Rect},
	style::{Color, Modifier, Style},
	text::{Line, Span},
	widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use rp_extio_wire::{
	ClockSource, DEFAULT_PIN_MAP, Peripheral, PinMap, PinSignal, PinValidationError,
};
use rp_extiod::control::{
	BoardInfo, ButtonConfig, ControlRequest, ControlResponse, GpioBias, GpioInfo, GpioStartup,
	PwmInfo, signal_name,
};
use std::{
	error::Error,
	io::stdout,
	time::{Duration, Instant},
};

const HEADER_ROWS: usize = 20;
const MIN_WIDTH: u16 = 54;
const MIN_HEIGHT: u16 = 14;
const WIDE_WIDTH: u16 = 92;
const CENTER_WIDTH: u16 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeaderPin {
	physical: u8,
	label: &'static str,
	gpio: Option<u8>,
}

impl HeaderPin {
	const fn gpio(physical: u8, gpio: u8, label: &'static str) -> Self {
		Self {
			physical,
			label,
			gpio: Some(gpio),
		}
	}

	const fn fixed(physical: u8, label: &'static str) -> Self {
		Self {
			physical,
			label,
			gpio: None,
		}
	}
}

const LEFT_HEADER: [HeaderPin; HEADER_ROWS] = [
	HeaderPin::gpio(1, 0, "GP0"),
	HeaderPin::gpio(2, 1, "GP1"),
	HeaderPin::fixed(3, "GND"),
	HeaderPin::gpio(4, 2, "GP2"),
	HeaderPin::gpio(5, 3, "GP3"),
	HeaderPin::gpio(6, 4, "GP4"),
	HeaderPin::gpio(7, 5, "GP5"),
	HeaderPin::fixed(8, "GND"),
	HeaderPin::gpio(9, 6, "GP6"),
	HeaderPin::gpio(10, 7, "GP7"),
	HeaderPin::gpio(11, 8, "GP8"),
	HeaderPin::gpio(12, 9, "GP9"),
	HeaderPin::fixed(13, "GND"),
	HeaderPin::gpio(14, 10, "GP10"),
	HeaderPin::gpio(15, 11, "GP11"),
	HeaderPin::gpio(16, 12, "GP12"),
	HeaderPin::gpio(17, 13, "GP13"),
	HeaderPin::fixed(18, "GND"),
	HeaderPin::gpio(19, 14, "GP14"),
	HeaderPin::gpio(20, 15, "GP15"),
];

const RIGHT_HEADER: [HeaderPin; HEADER_ROWS] = [
	HeaderPin::fixed(40, "VBUS"),
	HeaderPin::fixed(39, "VSYS"),
	HeaderPin::fixed(38, "GND"),
	HeaderPin::fixed(37, "3V3_EN"),
	HeaderPin::fixed(36, "3V3_OUT"),
	HeaderPin::fixed(35, "ADC_VREF"),
	HeaderPin::gpio(34, 28, "GP28/ADC2"),
	HeaderPin::fixed(33, "AGND"),
	HeaderPin::gpio(32, 27, "GP27/ADC1"),
	HeaderPin::gpio(31, 26, "GP26/ADC0"),
	HeaderPin::fixed(30, "RUN"),
	HeaderPin::gpio(29, 22, "GP22"),
	HeaderPin::fixed(28, "GND"),
	HeaderPin::gpio(27, 21, "GP21"),
	HeaderPin::gpio(26, 20, "GP20"),
	HeaderPin::gpio(25, 19, "GP19"),
	HeaderPin::gpio(24, 18, "GP18"),
	HeaderPin::fixed(23, "GND"),
	HeaderPin::gpio(22, 17, "GP17"),
	HeaderPin::gpio(21, 16, "GP16"),
];

struct TerminalGuard;

impl TerminalGuard {
	fn enter() -> Result<Self, Box<dyn Error>> {
		enable_raw_mode()?;
		execute!(stdout(), EnterAlternateScreen)?;
		Ok(Self)
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UiMode {
	Route,
	Operate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperateSelection {
	Pin,
	Signal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AdcReading {
	channel: u8,
	raw: u16,
	reference_mv: u16,
	resolution_bits: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PwmField {
	Frequency,
	Duty,
}

#[derive(Clone, Debug, PartialEq)]
struct PwmEditor {
	channel: u8,
	frequency_hz: String,
	duty_percent: String,
	enabled: bool,
	inverted: bool,
	field: PwmField,
}

impl Drop for TerminalGuard {
	fn drop(&mut self) {
		let _ = disable_raw_mode();
		let _ = execute!(stdout(), LeaveAlternateScreen);
	}
}

struct App {
	boards: Vec<BoardInfo>,
	board: usize,
	signal: usize,
	cursor_row: usize,
	cursor_side: usize,
	edited: Option<PinMap>,
	grouped: bool,
	flipped: bool,
	mode: UiMode,
	operate_selection: OperateSelection,
	gpio: Vec<GpioInfo>,
	adc: Option<AdcReading>,
	pwm: Option<PwmInfo>,
	pwm_editor: Option<PwmEditor>,
	telemetry_error: Option<String>,
	last_refresh: Instant,
	default_pending: bool,
	message: String,
	message_error: bool,
}

impl App {
	fn current(&self) -> Option<&BoardInfo> {
		self.boards.get(self.board)
	}

	fn active_map(&self) -> Option<PinMap> {
		self.current()
			.and_then(|board| board.active.map(Into::into))
	}

	fn selected_signal(&self) -> PinSignal {
		PinSignal::ALL[self.signal]
	}

	fn selected_pin(&self) -> HeaderPin {
		if self.cursor_side == 0 {
			LEFT_HEADER[self.cursor_row]
		} else {
			RIGHT_HEADER[self.cursor_row]
		}
	}

	fn selected_gpio(&self) -> Option<GpioInfo> {
		let pin = self.selected_pin().gpio?;
		self.gpio.iter().find(|gpio| gpio.pin == pin).copied()
	}

	fn selected_gpio_startup(&self) -> Option<GpioStartup> {
		let pin = self.selected_pin().gpio?;
		self.current()?
			.gpio_startup
			.iter()
			.find(|startup| startup.pin() == pin)
			.copied()
	}

	fn selected_button(&self) -> Option<ButtonConfig> {
		let pin = self.selected_pin().gpio?;
		self.current()?
			.buttons
			.iter()
			.find(|button| button.pin == pin)
			.copied()
	}

	fn selected_pwm_startup(&self) -> Option<PwmInfo> {
		let channel = self.operation_signal()?.pwm_channel()? as u8;
		self.current()?
			.pwm_startup
			.iter()
			.find(|pwm| pwm.channel == channel)
			.copied()
	}

	fn operation_signal(&self) -> Option<PinSignal> {
		if self.operate_selection == OperateSelection::Signal {
			return Some(self.selected_signal());
		}
		let pin = self.selected_pin().gpio?;
		let map = self.active_map()?;
		PinSignal::ALL.into_iter().find(|&signal| {
			signal.serial_peripheral().is_none()
				&& map.is_signal_enabled(signal)
				&& map.pin(signal) == pin
		})
	}

	fn screen_cursor_row(&self) -> usize {
		self.cursor_row
	}

	fn dirty(&self) -> bool {
		self.default_pending || self.edited != self.active_map()
	}

	fn clear_message(&mut self) {
		self.message.clear();
		self.message_error = false;
	}

	fn toggle_mode(&mut self) {
		self.mode = match self.mode {
			UiMode::Route => UiMode::Operate,
			UiMode::Operate => UiMode::Route,
		};
		if self.mode == UiMode::Operate {
			self.operate_selection = OperateSelection::Pin;
		}
		self.clear_message();
	}

	fn load_current(&mut self) {
		self.edited = self.active_map();
		self.default_pending = false;
		self.move_to_signal_pin();
		self.clear_message();
	}

	fn switch_board(&mut self, delta: isize) {
		if self.boards.is_empty() {
			return;
		}
		self.board = (self.board as isize + delta).rem_euclid(self.boards.len() as isize) as usize;
		self.load_current();
		self.clear_telemetry();
	}

	fn cycle_signal(&mut self, delta: isize) {
		self.signal =
			(self.signal as isize + delta).rem_euclid(PinSignal::ALL.len() as isize) as usize;
		if self.mode == UiMode::Operate {
			self.operate_selection = OperateSelection::Signal;
		}
		self.move_to_signal_pin();
		self.clear_message();
		self.adc = None;
		self.pwm = None;
	}

	fn move_to_signal_pin(&mut self) {
		let Some(map) = self.edited else {
			return;
		};
		let gpio = map.pin(self.selected_signal());
		if let Some((row, side)) = find_gpio(gpio) {
			self.cursor_row = row;
			self.cursor_side = side;
		}
	}

	fn move_cursor(&mut self, row_delta: isize, side_delta: isize) {
		let side_delta = if self.flipped {
			-side_delta
		} else {
			side_delta
		};
		self.cursor_row =
			(self.cursor_row as isize + row_delta).clamp(0, HEADER_ROWS as isize - 1) as usize;
		self.cursor_side = (self.cursor_side as isize + side_delta).clamp(0, 1) as usize;
		if self.mode == UiMode::Operate {
			self.operate_selection = OperateSelection::Pin;
			self.adc = None;
			self.pwm = None;
		}
		self.clear_message();
	}

	fn clear_telemetry(&mut self) {
		self.gpio.clear();
		self.adc = None;
		self.pwm = None;
		self.telemetry_error = None;
		self.last_refresh = Instant::now() - Duration::from_secs(1);
	}

	fn assign_selected(&mut self) {
		let pin = self.selected_pin();
		let signal = self.selected_signal();
		let Some(gpio) = pin.gpio else {
			self.message = format!(
				"physical pin {} ({}) cannot carry a route",
				pin.physical, pin.label
			);
			self.message_error = true;
			return;
		};
		if !signal.valid_pins().contains(&gpio) {
			self.message = format!("GP{gpio} is not a valid route for {}", signal_name(signal));
			self.message_error = true;
			return;
		}
		let Some(mut map) = self.edited else {
			self.message = "board is disconnected".into();
			self.message_error = true;
			return;
		};
		let auto_enabled = !map.is_signal_enabled(signal);
		if auto_enabled {
			map.set_signal_enabled(signal, true);
		}
		let (candidate, moved_group) = match preferred_routed_map(map, signal, gpio, self.grouped) {
			Ok(candidate) => candidate,
			Err(RouteError::Incomplete(message)) => {
				self.message = message.into();
				self.message_error = true;
				return;
			}
			Err(RouteError::Invalid(error)) => {
				self.message = if auto_enabled {
					format!(
						"cannot assign and enable {}: {}",
						enable_target(signal),
						rp_extiod::control::format_validation(error)
					)
				} else {
					format!(
						"grouped move rejected: {}",
						rp_extiod::control::format_validation(error)
					)
				};
				self.message_error = true;
				return;
			}
		};
		if auto_enabled && let Err(error) = candidate.validate() {
			self.message = format!(
				"cannot assign and enable {}: {}",
				enable_target(signal),
				rp_extiod::control::format_validation(error)
			);
			self.message_error = true;
			return;
		}
		self.edited = Some(candidate);
		self.default_pending = false;
		if let Err(error) = candidate.validate() {
			self.message = rp_extiod::control::format_validation(error);
			self.message_error = true;
		} else {
			self.message = if auto_enabled {
				format!(
					"{} routed and enabled; press a to apply",
					enable_target(signal)
				)
			} else if moved_group {
				"draft route group updated; press a to apply".into()
			} else if self.grouped && coupled_signal(signal) {
				"group could not move together; selected route updated individually; press a to apply"
                    .into()
			} else {
				"draft route updated; press a to apply".into()
			};
			self.message_error = false;
		}
	}

	fn toggle_grouped(&mut self) {
		self.grouped = !self.grouped;
		self.message = if self.grouped {
			"grouped assignment enabled"
		} else {
			"individual assignment enabled; the draft may be temporarily invalid"
		}
		.into();
		self.message_error = false;
	}

	fn toggle_selected_enabled(&mut self) {
		let signal = self.selected_signal();
		let Some(mut candidate) = self.edited else {
			self.message = "board is disconnected".into();
			self.message_error = true;
			return;
		};
		let enabled = signal_toggle_enabled(candidate, signal);
		candidate.set_signal_enabled(signal, !enabled);
		if !enabled && let Err(error) = candidate.validate() {
			self.message = format!(
				"cannot enable {}: {}",
				enable_target(signal),
				rp_extiod::control::format_validation(error)
			);
			self.message_error = true;
			return;
		}
		self.edited = Some(candidate);
		self.default_pending = false;
		self.message = format!(
			"{} {}; press a to apply",
			enable_target(signal),
			if enabled { "disabled" } else { "enabled" }
		);
		self.message_error = false;
	}

	fn flip(&mut self) {
		self.flipped = !self.flipped;
		self.message = if self.flipped {
			"board view flipped to back (belly-up) side"
		} else {
			"board view restored to front (component) side"
		}
		.into();
		self.message_error = false;
	}

	fn cycle_clock_source(&mut self) {
		let Some(output) = self.selected_signal().clock_output() else {
			self.message = "select CLOCK0 or CLOCK1 to change its source".into();
			self.message_error = true;
			return;
		};
		let Some(mut map) = self.edited else {
			return;
		};
		const SOURCES: [ClockSource; 7] = [
			ClockSource::System,
			ClockSource::Reference,
			ClockSource::Usb,
			ClockSource::Adc,
			ClockSource::Crystal,
			ClockSource::RingOscillator,
			ClockSource::LowPowerOscillator,
		];
		let current = SOURCES
			.iter()
			.position(|&source| source == map.clock_sources[output])
			.unwrap();
		map.clock_sources[output] = SOURCES[(current + 1) % SOURCES.len()];
		self.edited = Some(map);
		self.default_pending = false;
		self.message = format!(
			"CLOCK{output} source set to {:?}; press a to apply",
			map.clock_sources[output]
		);
		self.message_error = false;
	}

	fn adjust_clock_frequency(&mut self, increase: bool) {
		let Some(output) = self.selected_signal().clock_output() else {
			self.message = "select CLOCK0 or CLOCK1 to change its frequency".into();
			self.message_error = true;
			return;
		};
		let Some(mut map) = self.edited else {
			return;
		};
		map.clock_hz[output] = if increase {
			map.clock_hz[output].saturating_mul(2)
		} else {
			(map.clock_hz[output] / 2).max(1)
		};
		self.edited = Some(map);
		self.default_pending = false;
		self.message = format!(
			"CLOCK{output} target set to {}Hz; press a to apply",
			map.clock_hz[output]
		);
		self.message_error = false;
	}

	fn open_pwm_editor(&mut self) {
		let Some(signal) = self.operation_signal() else {
			self.message = "select a PWM signal or its routed pin".into();
			self.message_error = true;
			return;
		};
		let Some(channel) = signal.pwm_channel() else {
			self.message = "select a PWM signal to edit its output".into();
			self.message_error = true;
			return;
		};
		if !self
			.active_map()
			.is_some_and(|map| map.is_signal_enabled(signal))
		{
			self.message = format!("{} is not routed", display_signal(signal));
			self.message_error = true;
			return;
		}
		let pwm = self.pwm.filter(|pwm| pwm.channel == channel as u8);
		let period_ns = pwm.map_or(1_000_000, |pwm| pwm.period_ns.max(1));
		let frequency_hz = 1_000_000_000_u64
			.saturating_add(period_ns / 2)
			.checked_div(period_ns)
			.unwrap_or(1);
		let duty = pwm.map_or(50.0, |pwm| {
			pwm.duty_ns as f64 * 100.0 / pwm.period_ns.max(1) as f64
		});
		self.pwm_editor = Some(PwmEditor {
			channel: channel as u8,
			frequency_hz: frequency_hz.to_string(),
			duty_percent: format!("{duty:.1}"),
			enabled: pwm.is_some_and(|pwm| pwm.enabled),
			inverted: pwm.is_some_and(|pwm| pwm.inverted),
			field: PwmField::Frequency,
		});
		self.clear_message();
	}

	fn load_default(&mut self) {
		if self.edited.is_none() {
			self.message = "board is disconnected".into();
			self.message_error = true;
			return;
		}
		self.edited = Some(DEFAULT_PIN_MAP);
		self.default_pending = true;
		self.move_to_signal_pin();
		self.message = "firmware defaults loaded into draft; press a to apply".into();
		self.message_error = false;
	}

	fn undo(&mut self) {
		self.edited = self.active_map();
		self.default_pending = false;
		self.move_to_signal_pin();
		self.message = "draft restored from active map".into();
		self.message_error = false;
	}

	fn refresh(
		&mut self,
		client: &mut Client,
		keep_serial: Option<&str>,
	) -> Result<(), Box<dyn Error>> {
		let serial = keep_serial
			.map(str::to_owned)
			.or_else(|| self.current().map(|board| board.serial.clone()));
		self.boards = client.boards()?;
		self.board = serial
			.and_then(|serial| self.boards.iter().position(|board| board.serial == serial))
			.unwrap_or_else(|| {
				self.boards
					.iter()
					.position(|board| board.connected)
					.unwrap_or(0)
			});
		self.load_current();
		Ok(())
	}
}

pub fn run(client: &mut Client, selected: Option<&str>) -> Result<(), Box<dyn Error>> {
	let boards = client.boards()?;
	let board = selected
		.and_then(|serial| boards.iter().position(|board| board.serial == serial))
		.or_else(|| boards.iter().position(|board| board.connected))
		.unwrap_or(0);
	if let Some(serial) = selected
		&& !boards.iter().any(|board| board.serial == serial)
	{
		return Err(format!("unknown rp-extio board {serial}").into());
	}
	let mut app = App {
		boards,
		board,
		signal: 0,
		cursor_row: 0,
		cursor_side: 0,
		edited: None,
		grouped: true,
		flipped: false,
		mode: UiMode::Route,
		operate_selection: OperateSelection::Pin,
		gpio: Vec::new(),
		adc: None,
		pwm: None,
		pwm_editor: None,
		telemetry_error: None,
		last_refresh: Instant::now() - Duration::from_secs(1),
		default_pending: false,
		message: String::new(),
		message_error: false,
	};
	app.load_current();
	refresh_telemetry(client, &mut app);

	let _guard = TerminalGuard::enter()?;
	let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
	let mut redraw = true;
	loop {
		if redraw {
			terminal.draw(|frame| draw(frame, &app))?;
			redraw = false;
		}
		if !event::poll(Duration::from_millis(200))? {
			if app.mode == UiMode::Operate
				&& app.pwm_editor.is_none()
				&& app.last_refresh.elapsed() >= Duration::from_millis(250)
			{
				redraw = refresh_telemetry(client, &mut app);
			}
			continue;
		}
		let Event::Key(key) = event::read()? else {
			continue;
		};
		if key.kind != KeyEventKind::Press {
			continue;
		}
		if app.pwm_editor.is_some() {
			handle_pwm_editor_key(client, &mut app, key.code)?;
			redraw = true;
			continue;
		}
		if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
			break;
		}
		match key.code {
			KeyCode::Char('q') | KeyCode::Esc => break,
			KeyCode::Char('[') => {
				app.switch_board(-1);
				refresh_telemetry(client, &mut app);
			}
			KeyCode::Char(']') => {
				app.switch_board(1);
				refresh_telemetry(client, &mut app);
			}
			KeyCode::Up => {
				app.move_cursor(-1, 0);
				if app.mode == UiMode::Operate {
					refresh_telemetry(client, &mut app);
				}
			}
			KeyCode::Down => {
				app.move_cursor(1, 0);
				if app.mode == UiMode::Operate {
					refresh_telemetry(client, &mut app);
				}
			}
			KeyCode::Left => {
				app.move_cursor(0, -1);
				if app.mode == UiMode::Operate {
					refresh_telemetry(client, &mut app);
				}
			}
			KeyCode::Right => {
				app.move_cursor(0, 1);
				if app.mode == UiMode::Operate {
					refresh_telemetry(client, &mut app);
				}
			}
			KeyCode::Tab => {
				app.cycle_signal(1);
				refresh_telemetry(client, &mut app);
			}
			KeyCode::BackTab => {
				app.cycle_signal(-1);
				refresh_telemetry(client, &mut app);
			}
			KeyCode::Char('v') => {
				app.toggle_mode();
				refresh_telemetry(client, &mut app);
			}
			KeyCode::Char('f') => app.flip(),
			KeyCode::Enter | KeyCode::Char(' ') if app.mode == UiMode::Route => {
				app.assign_selected()
			}
			KeyCode::Char('g') if app.mode == UiMode::Route => app.toggle_grouped(),
			KeyCode::Char('e') if app.mode == UiMode::Route => app.toggle_selected_enabled(),
			KeyCode::Char('s') if app.mode == UiMode::Route => app.cycle_clock_source(),
			KeyCode::Char('+') | KeyCode::Char('=') if app.mode == UiMode::Route => {
				app.adjust_clock_frequency(true)
			}
			KeyCode::Char('-') if app.mode == UiMode::Route => app.adjust_clock_frequency(false),
			KeyCode::Char('u') if app.mode == UiMode::Route => app.undo(),
			KeyCode::Char('d') if app.mode == UiMode::Route => app.load_default(),
			KeyCode::Char('a') if app.mode == UiMode::Route => apply(client, &mut app)?,
			KeyCode::Char('0') if app.mode == UiMode::Operate => {
				set_gpio_output(client, &mut app, false)?
			}
			KeyCode::Char('1') if app.mode == UiMode::Operate => {
				set_gpio_output(client, &mut app, true)?
			}
			KeyCode::Char('t') | KeyCode::Enter | KeyCode::Char(' ')
				if app.mode == UiMode::Operate =>
			{
				operate_default(client, &mut app)?
			}
			KeyCode::Char('i') if app.mode == UiMode::Operate => {
				cycle_gpio_input(client, &mut app)?
			}
			KeyCode::Char('w') if app.mode == UiMode::Operate => save_startup(client, &mut app)?,
			KeyCode::Char('x') if app.mode == UiMode::Operate => clear_startup(client, &mut app)?,
			KeyCode::Char('b') if app.mode == UiMode::Operate => {
				cycle_button_config(client, &mut app)?
			}
			KeyCode::Char('p') if app.mode == UiMode::Operate => app.open_pwm_editor(),
			KeyCode::Char('r') => {
				let serial = app.current().map(|board| board.serial.clone());
				app.refresh(client, serial.as_deref())?;
				refresh_telemetry(client, &mut app);
				app.message = "refreshed".into();
				app.message_error = false;
			}
			KeyCode::Char('l') => match client.request(&ControlRequest::Reload)? {
				ControlResponse::Ok => {
					app.refresh(client, None)?;
					refresh_telemetry(client, &mut app);
					app.message = "configuration reloaded".into();
					app.message_error = false;
				}
				ControlResponse::Error { message, .. } => {
					app.message = message;
					app.message_error = true;
				}
				_ => {
					app.message = "unexpected daemon response".into();
					app.message_error = true;
				}
			},
			_ => {}
		}
		redraw = true;
	}
	Ok(())
}

fn apply(client: &mut Client, app: &mut App) -> Result<(), Box<dyn Error>> {
	let Some(board) = app.current().cloned() else {
		return Ok(());
	};
	let Some(map) = app.edited else {
		return Ok(());
	};
	if !app.dirty() {
		app.message = "draft already matches the active map".into();
		app.message_error = false;
		return Ok(());
	}
	if let Err(error) = map.validate() {
		app.message = rp_extiod::control::format_validation(error);
		app.message_error = true;
		return Ok(());
	}
	let Some(generation) = board.generation else {
		app.message = "board is disconnected".into();
		app.message_error = true;
		return Ok(());
	};
	let default_pending = app.default_pending;
	let request = if default_pending {
		ControlRequest::Default {
			serial: board.serial.clone(),
			generation,
		}
	} else {
		ControlRequest::Set {
			serial: board.serial.clone(),
			generation,
			map: map.into(),
		}
	};
	match client.request(&request)? {
		ControlResponse::Applied { .. } => {
			app.refresh(client, Some(&board.serial))?;
			app.message = if default_pending {
				"firmware defaults restored"
			} else {
				"pin map applied"
			}
			.into();
			app.message_error = false;
		}
		ControlResponse::Error { message, .. } => {
			app.message = message;
			app.message_error = true;
		}
		_ => {
			app.message = "unexpected daemon response".into();
			app.message_error = true;
		}
	}
	Ok(())
}

fn refresh_telemetry(client: &mut Client, app: &mut App) -> bool {
	let previous = (
		app.gpio.clone(),
		app.adc,
		app.pwm,
		app.telemetry_error.clone(),
	);
	app.last_refresh = Instant::now();
	app.telemetry_error = None;
	let Some(board) = app.current().cloned().filter(|board| board.connected) else {
		app.gpio.clear();
		app.adc = None;
		app.pwm = None;
		return previous
			!= (
				app.gpio.clone(),
				app.adc,
				app.pwm,
				app.telemetry_error.clone(),
			);
	};
	match client.request(&ControlRequest::GpioSnapshot {
		serial: board.serial.clone(),
	}) {
		Ok(ControlResponse::GpioSnapshot { gpio }) => app.gpio = gpio,
		Ok(ControlResponse::Error { message, .. }) => app.telemetry_error = Some(message),
		Ok(_) => app.telemetry_error = Some("unexpected GPIO snapshot response".into()),
		Err(error) => app.telemetry_error = Some(error.to_string()),
	}
	let Some(map) = board.active.map(PinMap::from) else {
		return previous
			!= (
				app.gpio.clone(),
				app.adc,
				app.pwm,
				app.telemetry_error.clone(),
			);
	};
	let signal = app.operation_signal();
	app.adc = None;
	app.pwm = None;
	if let Some(signal) = signal
		&& let Some(channel) = signal.adc_channel()
		&& map.is_signal_enabled(signal)
	{
		match client.request(&ControlRequest::AdcRead {
			serial: board.serial,
			channel: channel as u8,
		}) {
			Ok(ControlResponse::Adc {
				channel,
				raw,
				reference_mv,
				resolution_bits,
			}) => {
				app.adc = Some(AdcReading {
					channel,
					raw,
					reference_mv,
					resolution_bits,
				});
			}
			Ok(ControlResponse::Error { message, .. }) => app.telemetry_error = Some(message),
			Ok(_) => app.telemetry_error = Some("unexpected ADC response".into()),
			Err(error) => app.telemetry_error = Some(error.to_string()),
		}
	} else if let Some(signal) = signal
		&& let Some(channel) = signal.pwm_channel()
		&& map.is_signal_enabled(signal)
	{
		match client.request(&ControlRequest::PwmGet {
			serial: board.serial,
			channel: channel as u8,
		}) {
			Ok(ControlResponse::Pwm { pwm }) => app.pwm = Some(pwm),
			Ok(ControlResponse::Error { message, .. }) => app.telemetry_error = Some(message),
			Ok(_) => app.telemetry_error = Some("unexpected PWM response".into()),
			Err(error) => app.telemetry_error = Some(error.to_string()),
		}
	}
	previous
		!= (
			app.gpio.clone(),
			app.adc,
			app.pwm,
			app.telemetry_error.clone(),
		)
}

fn set_gpio_output(client: &mut Client, app: &mut App, value: bool) -> Result<(), Box<dyn Error>> {
	let Some(pin) = app.selected_pin().gpio else {
		set_app_error(app, "select a GPIO pin");
		return Ok(());
	};
	let Some(serial) = app.current().map(|board| board.serial.clone()) else {
		set_app_error(app, "no board selected");
		return Ok(());
	};
	handle_gpio_response(
		app,
		client.request(&ControlRequest::GpioOutput { serial, pin, value })?,
		format!("GP{pin} driven {}", if value { "high" } else { "low" }),
	);
	refresh_telemetry(client, app);
	Ok(())
}

fn cycle_gpio_input(client: &mut Client, app: &mut App) -> Result<(), Box<dyn Error>> {
	let Some(pin) = app.selected_pin().gpio else {
		set_app_error(app, "select a GPIO pin");
		return Ok(());
	};
	let bias = match app.selected_gpio().map(|gpio| gpio.bias) {
		Some(GpioBias::None) | None => GpioBias::PullUp,
		Some(GpioBias::PullUp) => GpioBias::PullDown,
		Some(GpioBias::PullDown) => GpioBias::None,
	};
	let Some(serial) = app.current().map(|board| board.serial.clone()) else {
		set_app_error(app, "no board selected");
		return Ok(());
	};
	handle_gpio_response(
		app,
		client.request(&ControlRequest::GpioInput { serial, pin, bias })?,
		format!("GP{pin} configured as input with {}", bias_name(bias)),
	);
	refresh_telemetry(client, app);
	Ok(())
}

fn cycle_button_config(client: &mut Client, app: &mut App) -> Result<(), Box<dyn Error>> {
	let Some(gpio) = app.selected_gpio() else {
		set_app_error(app, "highlight a configured GPIO to set its button role");
		return Ok(());
	};
	if gpio.output {
		set_app_error(
			app,
			format!("GP{} must be an input to act as a button", gpio.pin),
		);
		return Ok(());
	}
	let Some(serial) = app.current().map(|board| board.serial.clone()) else {
		set_app_error(app, "no board selected");
		return Ok(());
	};
	let (request, success) = match app.selected_button() {
		None => (
			ControlRequest::ButtonSet {
				serial,
				button: ButtonConfig {
					pin: gpio.pin,
					active_low: true,
				},
			},
			format!("GP{} saved as an active-low button", gpio.pin),
		),
		Some(ButtonConfig {
			active_low: true, ..
		}) => (
			ControlRequest::ButtonSet {
				serial,
				button: ButtonConfig {
					pin: gpio.pin,
					active_low: false,
				},
			},
			format!("GP{} saved as an active-high button", gpio.pin),
		),
		Some(_) => (
			ControlRequest::ButtonClear {
				serial,
				pin: gpio.pin,
			},
			format!("GP{} button role cleared", gpio.pin),
		),
	};
	handle_startup_response(app, client.request(&request)?, success);
	Ok(())
}

fn save_startup(client: &mut Client, app: &mut App) -> Result<(), Box<dyn Error>> {
	if let Some(channel) = app.operation_signal().and_then(PinSignal::pwm_channel) {
		let Some(pwm) = app.pwm.filter(|pwm| pwm.channel == channel as u8) else {
			set_app_error(
				app,
				format!("PWM{channel} is not enabled or has no live state"),
			);
			return Ok(());
		};
		let Some(serial) = app.current().map(|board| board.serial.clone()) else {
			set_app_error(app, "no board selected");
			return Ok(());
		};
		handle_startup_response(
			app,
			client.request(&ControlRequest::PwmStartupSet { serial, pwm })?,
			format!("PWM{channel} live state saved for daemon startup"),
		);
		return Ok(());
	}
	if let Some(output) = app.operation_signal().and_then(PinSignal::clock_output) {
		app.message = format!(
			"CLOCK{output} enable, route, source, and frequency are already saved with the pin map"
		);
		app.message_error = false;
		return Ok(());
	}
	let Some(gpio) = app.selected_gpio() else {
		set_app_error(app, "highlight a configured GPIO to save its live state");
		return Ok(());
	};
	let Some(serial) = app.current().map(|board| board.serial.clone()) else {
		set_app_error(app, "no board selected");
		return Ok(());
	};
	let startup = if gpio.output {
		GpioStartup::Output {
			pin: gpio.pin,
			value: gpio.level,
		}
	} else {
		GpioStartup::Input {
			pin: gpio.pin,
			bias: gpio.bias,
		}
	};
	handle_startup_response(
		app,
		client.request(&ControlRequest::GpioStartupSet { serial, startup })?,
		format!("GP{} live state saved for daemon startup", gpio.pin),
	);
	Ok(())
}

fn clear_startup(client: &mut Client, app: &mut App) -> Result<(), Box<dyn Error>> {
	if let Some(channel) = app.operation_signal().and_then(PinSignal::pwm_channel) {
		let Some(serial) = app.current().map(|board| board.serial.clone()) else {
			set_app_error(app, "no board selected");
			return Ok(());
		};
		handle_startup_response(
			app,
			client.request(&ControlRequest::PwmStartupClear {
				serial,
				channel: channel as u8,
			})?,
			format!("PWM{channel} startup state forgotten"),
		);
		return Ok(());
	}
	if let Some(output) = app.operation_signal().and_then(PinSignal::clock_output) {
		app.message = format!(
			"CLOCK{output} has no separate startup state; disable it in route mode and apply"
		);
		app.message_error = false;
		return Ok(());
	}
	let Some(pin) = app.selected_pin().gpio else {
		set_app_error(app, "select a GPIO pin");
		return Ok(());
	};
	let Some(serial) = app.current().map(|board| board.serial.clone()) else {
		set_app_error(app, "no board selected");
		return Ok(());
	};
	handle_startup_response(
		app,
		client.request(&ControlRequest::GpioStartupClear { serial, pin })?,
		format!("GP{pin} startup state forgotten"),
	);
	Ok(())
}

fn handle_startup_response(app: &mut App, response: ControlResponse, success: String) {
	match response {
		ControlResponse::Board { board } => {
			if let Some(current) = app.boards.get_mut(app.board) {
				*current = board;
			}
			app.message = success;
			app.message_error = false;
		}
		ControlResponse::Error { message, .. } => set_app_error(app, message),
		_ => set_app_error(app, "unexpected GPIO startup response"),
	}
}

fn operate_default(client: &mut Client, app: &mut App) -> Result<(), Box<dyn Error>> {
	if app
		.operation_signal()
		.and_then(PinSignal::pwm_channel)
		.is_some()
	{
		app.open_pwm_editor();
	} else if app
		.operation_signal()
		.and_then(PinSignal::adc_channel)
		.is_some()
	{
		refresh_telemetry(client, app);
		if app.adc.is_some() {
			app.message = "ADC sample refreshed".into();
			app.message_error = false;
		}
	} else if let Some(gpio) = app.selected_gpio() {
		set_gpio_output(client, app, !gpio.level)?;
	} else {
		set_app_error(
			app,
			"highlight a configured GPIO, or select an ADC/PWM signal",
		);
	}
	Ok(())
}

fn handle_gpio_response(app: &mut App, response: ControlResponse, success: String) {
	match response {
		ControlResponse::Gpio { gpio } => {
			if let Some(current) = app.gpio.iter_mut().find(|current| current.pin == gpio.pin) {
				*current = gpio;
			}
			app.message = success;
			app.message_error = false;
		}
		ControlResponse::Error { message, .. } => set_app_error(app, message),
		_ => set_app_error(app, "unexpected GPIO response"),
	}
}

fn set_app_error(app: &mut App, message: impl Into<String>) {
	app.message = message.into();
	app.message_error = true;
}

fn handle_pwm_editor_key(
	client: &mut Client,
	app: &mut App,
	key: KeyCode,
) -> Result<(), Box<dyn Error>> {
	match key {
		KeyCode::Esc => {
			app.pwm_editor = None;
			app.message = "PWM edit cancelled".into();
			app.message_error = false;
		}
		KeyCode::Tab | KeyCode::BackTab => {
			let editor = app.pwm_editor.as_mut().unwrap();
			editor.field = match editor.field {
				PwmField::Frequency => PwmField::Duty,
				PwmField::Duty => PwmField::Frequency,
			};
		}
		KeyCode::Char('e') => {
			let editor = app.pwm_editor.as_mut().unwrap();
			editor.enabled = !editor.enabled;
		}
		KeyCode::Char('i') => {
			let editor = app.pwm_editor.as_mut().unwrap();
			editor.inverted = !editor.inverted;
		}
		KeyCode::Backspace => {
			let editor = app.pwm_editor.as_mut().unwrap();
			match editor.field {
				PwmField::Frequency => {
					editor.frequency_hz.pop();
				}
				PwmField::Duty => {
					editor.duty_percent.pop();
				}
			}
		}
		KeyCode::Char(character) if character.is_ascii_digit() || character == '.' => {
			let editor = app.pwm_editor.as_mut().unwrap();
			let field = match editor.field {
				PwmField::Frequency => &mut editor.frequency_hz,
				PwmField::Duty => &mut editor.duty_percent,
			};
			if character != '.' || (editor.field == PwmField::Duty && !field.contains('.')) {
				field.push(character);
			}
		}
		KeyCode::Enter => apply_pwm_editor(client, app)?,
		_ => {}
	}
	Ok(())
}

fn apply_pwm_editor(client: &mut Client, app: &mut App) -> Result<(), Box<dyn Error>> {
	let editor = app.pwm_editor.as_ref().unwrap();
	let frequency_hz = match editor.frequency_hz.parse::<u64>() {
		Ok(frequency) if frequency > 0 => frequency,
		_ => {
			set_app_error(app, "PWM frequency must be a positive integer");
			return Ok(());
		}
	};
	let duty_percent = match editor.duty_percent.parse::<f64>() {
		Ok(duty) if duty.is_finite() && (0.0..=100.0).contains(&duty) => duty,
		_ => {
			set_app_error(app, "PWM duty must be between 0 and 100 percent");
			return Ok(());
		}
	};
	let period_ns = (1_000_000_000_f64 / frequency_hz as f64).round() as u64;
	if period_ns == 0 {
		set_app_error(app, "PWM frequency is too high");
		return Ok(());
	}
	let pwm = PwmInfo {
		channel: editor.channel,
		enabled: editor.enabled,
		inverted: editor.inverted,
		period_ns,
		duty_ns: (period_ns as f64 * duty_percent / 100.0).round() as u64,
	};
	let Some(serial) = app.current().map(|board| board.serial.clone()) else {
		set_app_error(app, "no board selected");
		return Ok(());
	};
	match client.request(&ControlRequest::PwmApply { serial, pwm })? {
		ControlResponse::Pwm { pwm } => {
			app.pwm = Some(pwm);
			app.pwm_editor = None;
			app.message = format!("PWM{} output applied", pwm.channel);
			app.message_error = false;
			refresh_telemetry(client, app);
		}
		ControlResponse::Error { message, .. } => set_app_error(app, message),
		_ => set_app_error(app, "unexpected PWM response"),
	}
	Ok(())
}

fn draw(frame: &mut Frame<'_>, app: &App) {
	let area = frame.area();
	if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
		frame.render_widget(
			Paragraph::new(format!(
				"Terminal too small for the Pico 2 pinout\nneed at least {MIN_WIDTH}×{MIN_HEIGHT}, have {}×{}",
				area.width, area.height
			))
			.block(Block::default().title("rp-extio").borders(Borders::ALL)),
			area,
		);
		return;
	}

	let rows = Layout::default()
		.direction(Direction::Vertical)
		.constraints([
			Constraint::Length(3),
			Constraint::Min(5),
			Constraint::Length(4),
			Constraint::Length(3),
		])
		.split(area);
	draw_header(frame, rows[0], app);

	if area.width >= WIDE_WIDTH {
		let columns = Layout::default()
			.direction(Direction::Horizontal)
			.constraints([Constraint::Length(31), Constraint::Min(MIN_WIDTH)])
			.split(rows[1]);
		draw_legend(frame, columns[0], app);
		draw_pinout(frame, columns[1], app);
	} else {
		draw_pinout(frame, rows[1], app);
	}
	draw_status(frame, rows[2], app);
	let assignment_mode = if app.grouped { "grouped" } else { "single" };
	let help = if app.mode == UiMode::Operate && area.width >= 72 {
		"arrows pin  Tab signal  Enter/t act  0/1 output  i input/bias  p PWM\nw save startup  x forget startup  b button role  v route  f flip  r refresh  q quit".into()
	} else if app.mode == UiMode::Operate {
		"arrows/Tab select  Enter/t act  0/1 output  i input\nw save  x forget  p PWM  v route  r refresh  q quit".into()
	} else if area.width >= 72 {
		format!(
			"arrows pin  Tab signal  Enter assign  e enable  g {assignment_mode}  a apply\nv operate mode  f flip  s clock source  +/- clock Hz  d defaults  u undo  r refresh  q quit"
		)
	} else {
		format!(
			"arrows/Tab select  Enter assign  e enable  g {assignment_mode}\na apply  v operate  f flip  d defaults  u undo  r refresh  q quit"
		)
	};
	frame.render_widget(Paragraph::new(help), rows[3]);
	if app.pwm_editor.is_some() {
		draw_pwm_editor(frame, app);
	}
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let board = app.current();
	let selected = app.selected_signal();
	let route = app
		.edited
		.map(|map| {
			let clock = selected.clock_output().map_or_else(String::new, |output| {
				format!(
					", {:?} {}Hz",
					map.clock_sources[output], map.clock_hz[output]
				)
			});
			format!(
				"GP{} ({}{})",
				map.pin(selected),
				if signal_toggle_enabled(map, selected) {
					"enabled"
				} else {
					"disabled"
				},
				clock,
			)
		})
		.unwrap_or_else(|| "-".into());
	let selection = if app.mode == UiMode::Operate && app.operate_selection == OperateSelection::Pin
	{
		let pin = app.selected_pin();
		match pin.gpio {
			Some(gpio) => format!(
				"GP{gpio} → {}",
				app.operation_signal().map_or_else(
					|| "GPIO".to_owned(),
					|signal| display_signal(signal).to_owned(),
				)
			),
			None => format!("pin {} → {}", pin.physical, pin.label),
		}
	} else {
		format!("{} → {route}", display_signal(selected))
	};
	let summary = board.map_or_else(
		|| "no boards".into(),
		|board| {
			format!(
				"{} {}  gen {}  I²C {}  SPI {}",
				if board.connected { "●" } else { "○" },
				board.serial,
				option(board.generation),
				bus_numbers(board.i2c_buses),
				bus_numbers(board.spi_buses),
			)
		},
	);
	let dirty = if app.dirty() { " • DRAFT" } else { "" };
	let mode = match app.mode {
		UiMode::Route => "ROUTE",
		UiMode::Operate => "OPERATE",
	};
	frame.render_widget(
		Paragraph::new(format!("{mode}  {summary}  │  {selection}{dirty}",)).block(
			Block::default()
				.title("rp-extio · Pico 2")
				.borders(Borders::ALL),
		),
		area,
	);
}

fn draw_legend(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let active = app.active_map();
	let visible = usize::from(area.height.saturating_sub(2)).max(1);
	let start = app
		.signal
		.saturating_sub(visible / 2)
		.min(PinSignal::ALL.len().saturating_sub(visible));
	let items = PinSignal::ALL
		.iter()
		.enumerate()
		.skip(start)
		.take(visible)
		.map(|(index, &signal)| {
			let draft = app.edited.map(|map| map.pin(signal));
			let active = active.map(|map| map.pin(signal));
			let enabled = app
				.edited
				.is_some_and(|map| signal_toggle_enabled(map, signal));
			let route = match (active, draft) {
				(Some(old), Some(new)) if old != new => format!("GP{old} → GP{new}"),
				(_, Some(pin)) => format!("GP{pin}"),
				_ => "-".into(),
			};
			let style = if index == app.signal {
				signal_style(signal).add_modifier(Modifier::REVERSED | Modifier::BOLD)
			} else {
				signal_style(signal)
			};
			ListItem::new(format!(
				"{} {:<10} {route}",
				if enabled { "●" } else { "○" },
				display_signal(signal)
			))
			.style(style)
		})
		.collect::<Vec<_>>();
	frame.render_widget(
		List::new(items).block(
			Block::default()
				.title("Signals · Tab selects")
				.borders(Borders::ALL),
		),
		area,
	);
}

fn draw_pinout(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let block = Block::default()
		.title(if app.flipped {
			"Pico 2 · back (belly up) · USB end ▲"
		} else {
			"Pico 2 · front (components up) · USB end ▲"
		})
		.borders(Borders::ALL);
	let inner = block.inner(area);
	frame.render_widget(block, area);
	if inner.width < MIN_WIDTH - 2 || inner.height < 3 {
		frame.render_widget(Paragraph::new("pinout area too small"), inner);
		return;
	}

	let visible = usize::from(inner.height.saturating_sub(2)).min(HEADER_ROWS);
	let start = app
		.screen_cursor_row()
		.saturating_sub(visible / 2)
		.min(HEADER_ROWS - visible);
	let end = start + visible;
	let mut lines = Vec::with_capacity(visible + 2);
	let top_end = "USB CONNECTOR";
	let bottom_end = "SWD DEBUG";
	let top = if visible == HEADER_ROWS {
		top_end.into()
	} else if start == 0 {
		format!("{top_end} · rows 1–{end} of {HEADER_ROWS}")
	} else {
		format!("▲ more · header rows {}–{end} of {HEADER_ROWS}", start + 1)
	};
	lines.push(centered_line(
		inner.width,
		top,
		Style::default().fg(Color::Cyan),
	));
	for row in start..end {
		lines.push(pin_row(inner.width, row, app));
	}
	lines.push(centered_line(
		inner.width,
		if end == HEADER_ROWS {
			bottom_end.into()
		} else {
			"▼ more header rows".into()
		},
		Style::default().fg(Color::DarkGray),
	));
	frame.render_widget(Paragraph::new(lines), inner);
}

fn pin_row(width: u16, screen_row: usize, app: &App) -> Line<'static> {
	let cell_width = usize::from((width - CENTER_WIDTH) / 2);
	let remainder = usize::from(width - CENTER_WIDTH) - cell_width * 2;
	let left_width = cell_width;
	let right_width = cell_width + remainder;
	let center = match screen_row {
		8 => "│ PICO │",
		9 => "│  2   │",
		_ => "│      │",
	};
	let logical_row = screen_row;
	let (left, left_side, right, right_side) = if app.flipped {
		(RIGHT_HEADER[logical_row], 1, LEFT_HEADER[logical_row], 0)
	} else {
		(LEFT_HEADER[logical_row], 0, RIGHT_HEADER[logical_row], 1)
	};
	Line::from(vec![
		pin_cell(left, left_width, logical_row, left_side, app, false),
		Span::styled(center, Style::default().fg(Color::DarkGray)),
		pin_cell(right, right_width, logical_row, right_side, app, true),
	])
}

fn pin_cell(
	pin: HeaderPin,
	width: usize,
	row: usize,
	side: usize,
	app: &App,
	right: bool,
) -> Span<'static> {
	let role_width = width.saturating_sub(16);
	let (role, conflict, changed) = pin_role(pin, app);
	let marker = if changed { "*" } else { " " };
	let role = fit(&format!("{marker}{role}"), role_width);
	let text = if right {
		format!(
			"● {role:<role_width$} {:>10} {:>2}",
			pin.label, pin.physical
		)
	} else {
		format!(
			"{:>2} {:<10} {role:<role_width$} ●",
			pin.physical, pin.label
		)
	};
	let mut style = pin_style(pin, app, conflict);
	if row == app.cursor_row && side == app.cursor_side {
		style = style
			.bg(Color::White)
			.fg(Color::Black)
			.add_modifier(Modifier::BOLD);
	}
	Span::styled(fit(&text, width), style)
}

fn pin_role(pin: HeaderPin, app: &App) -> (String, bool, bool) {
	let Some(gpio) = pin.gpio else {
		return (String::new(), false, false);
	};
	let Some(map) = app.edited else {
		return ("GPIO".into(), false, false);
	};
	let signals = PinSignal::ALL
		.into_iter()
		.filter(|&signal| map.is_signal_enabled(signal) && map.pin(signal) == gpio)
		.collect::<Vec<_>>();
	let mut role = if signals.is_empty() && app.mode == UiMode::Operate {
		app.gpio
			.iter()
			.find(|state| state.pin == gpio)
			.map_or_else(|| "GPIO".into(), gpio_pin_role)
	} else if signals.is_empty() {
		"GPIO".into()
	} else {
		signals
			.iter()
			.map(|&signal| short_signal(signal))
			.collect::<Vec<_>>()
			.join("+")
	};
	if app.mode == UiMode::Operate
		&& app.current().is_some_and(|board| {
			board
				.gpio_startup
				.iter()
				.any(|startup| startup.pin() == gpio)
		}) {
		role.push_str(" •");
	}
	if app.mode == UiMode::Operate
		&& let Some(button) = app
			.current()
			.and_then(|board| board.buttons.iter().find(|button| button.pin == gpio))
	{
		role.push_str(if button.active_low { " B↓" } else { " B↑" });
	}
	if app.mode == UiMode::Operate
		&& app.current().is_some_and(|board| {
			board.pwm_startup.iter().any(|pwm| {
				map.pwm_pins
					.get(pwm.channel as usize)
					.is_some_and(|&pin| pin == gpio)
			})
		}) {
		role.push_str(" •");
	}
	let changed = app.active_map().is_some_and(|active| {
		PinSignal::ALL.into_iter().any(|signal| {
			let active_enabled = active.is_signal_enabled(signal);
			let draft_enabled = map.is_signal_enabled(signal);
			(active_enabled != draft_enabled
				|| (draft_enabled && active.pin(signal) != map.pin(signal)))
				&& ((active_enabled && active.pin(signal) == gpio)
					|| (draft_enabled && map.pin(signal) == gpio))
		})
	});
	(role, signals.len() > 1, changed)
}

fn pin_style(pin: HeaderPin, app: &App, conflict: bool) -> Style {
	let Some(gpio) = pin.gpio else {
		return Style::default().fg(Color::DarkGray);
	};
	if conflict {
		return Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
	}
	let signal = app.selected_signal();
	if app.edited.is_some_and(|map| map.pin(signal) == gpio) {
		return signal_style(signal).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
	}
	if signal.valid_pins().contains(&gpio)
		&& app
			.edited
			.is_some_and(|map| preferred_routed_map(map, signal, gpio, app.grouped).is_ok())
	{
		return Style::default().fg(Color::Green);
	}
	if let Some(map) = app.edited
		&& let Some(assigned) = PinSignal::ALL
			.into_iter()
			.find(|&candidate| map.is_signal_enabled(candidate) && map.pin(candidate) == gpio)
	{
		return signal_style(assigned);
	}
	Style::default().fg(Color::Gray)
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
	let validation = app.edited.and_then(|map| map.validate().err());
	let message = if !app.message.is_empty() {
		app.message.clone()
	} else if let Some(error) = &app.telemetry_error {
		error.clone()
	} else if let Some(error) = validation {
		rp_extiod::control::format_validation(error)
	} else if app.dirty() && app.mode == UiMode::Route {
		"draft is valid; press a to apply".into()
	} else if app.dirty() {
		"route draft pending; press v to return to route mode".into()
	} else if app.mode == UiMode::Operate {
		"live state refreshes while idle; mutations may request Polkit authorization".into()
	} else {
		"active map; move to a valid highlighted GPIO and press Enter to assign".into()
	};
	let detail = selected_runtime_detail(app);
	let style = if validation.is_some() || app.message_error || app.telemetry_error.is_some() {
		Style::default().fg(Color::Red)
	} else if app.dirty() {
		Style::default().fg(Color::Yellow)
	} else {
		Style::default().fg(Color::Green)
	};
	frame.render_widget(
		Paragraph::new(format!("{detail}\n{message}"))
			.style(style)
			.block(Block::default().title("Status").borders(Borders::ALL)),
		area,
	);
}

fn gpio_pin_role(gpio: &GpioInfo) -> String {
	let direction = if gpio.output { "O" } else { "I" };
	let bias = match gpio.bias {
		GpioBias::None => "",
		GpioBias::PullUp => "↑",
		GpioBias::PullDown => "↓",
	};
	format!(
		"GPIO {direction}{bias} {}",
		if gpio.level { "H" } else { "L" }
	)
}

fn selected_runtime_detail(app: &App) -> String {
	if app.mode == UiMode::Route {
		let signal = app.selected_signal();
		return app.edited.map_or_else(
			|| "No active route map".into(),
			|map| {
				format!(
					"Route: {} → GP{} · {} · {} assignment",
					display_signal(signal),
					map.pin(signal),
					if map.is_signal_enabled(signal) {
						"enabled"
					} else {
						"disabled"
					},
					if app.grouped { "grouped" } else { "individual" }
				)
			},
		);
	}
	if let Some(adc) = app.adc {
		let millivolts =
			u32::from(adc.raw) * u32::from(adc.reference_mv) / (1_u32 << adc.resolution_bits);
		return format!("ADC{}: {} raw · {} mV", adc.channel, adc.raw, millivolts);
	}
	if let Some(pwm) = app.pwm {
		let frequency = 1_000_000_000_u64.checked_div(pwm.period_ns).unwrap_or(0);
		let duty = if pwm.period_ns == 0 {
			0.0
		} else {
			pwm.duty_ns as f64 * 100.0 / pwm.period_ns as f64
		};
		let startup = app.selected_pwm_startup().map_or_else(
			|| "startup not saved".into(),
			|saved| {
				let saved_frequency = 1_000_000_000_u64.checked_div(saved.period_ns).unwrap_or(0);
				let saved_duty = if saved.period_ns == 0 {
					0.0
				} else {
					saved.duty_ns as f64 * 100.0 / saved.period_ns as f64
				};
				format!("startup {saved_frequency} Hz/{saved_duty:.1}%")
			},
		);
		return format!(
			"PWM{}: {} · {frequency} Hz · {duty:.1}% · {} · {startup}",
			pwm.channel,
			if pwm.enabled {
				"output enabled"
			} else {
				"output disabled"
			},
			if pwm.inverted { "inverted" } else { "normal" }
		);
	}
	if let Some(signal) = app.operation_signal()
		&& let Some(output) = signal.clock_output()
		&& let Some(map) = app.active_map()
	{
		return format!(
			"CLOCK{output}: {:?} · target {} Hz · {} · persisted in pin map",
			map.clock_sources[output],
			map.clock_hz[output],
			if map.is_signal_enabled(signal) {
				"enabled"
			} else {
				"disabled"
			}
		);
	}
	if let Some(gpio) = app.selected_gpio() {
		let button = app.selected_button().map_or_else(
			|| "button unconfigured".into(),
			|button| {
				format!(
					"button {} ({})",
					if gpio.level != button.active_low {
						"pressed"
					} else {
						"released"
					},
					if button.active_low {
						"active low"
					} else {
						"active high"
					}
				)
			},
		);
		let startup = app.selected_gpio_startup().map_or_else(
			|| "startup not saved".into(),
			|startup| match startup {
				GpioStartup::Output { value, .. } => {
					format!("startup output {}", if value { "HIGH" } else { "LOW" })
				}
				GpioStartup::Input { bias, .. } => {
					format!("startup input, bias {}", bias_name(bias))
				}
			},
		);
		return format!(
			"GP{}: {} · {} · bias {} · {startup} · {button}",
			gpio.pin,
			if gpio.level { "HIGH" } else { "LOW" },
			if gpio.output { "output" } else { "input" },
			bias_name(gpio.bias),
		);
	}
	"No live state for this selection".into()
}

fn bias_name(bias: GpioBias) -> &'static str {
	match bias {
		GpioBias::None => "none",
		GpioBias::PullUp => "pull-up",
		GpioBias::PullDown => "pull-down",
	}
}

fn draw_pwm_editor(frame: &mut Frame<'_>, app: &App) {
	let editor = app.pwm_editor.as_ref().unwrap();
	let area = centered_rect(54, 9, frame.area());
	frame.render_widget(Clear, area);
	let frequency_style = if editor.field == PwmField::Frequency {
		Style::default().fg(Color::Black).bg(Color::White)
	} else {
		Style::default()
	};
	let duty_style = if editor.field == PwmField::Duty {
		Style::default().fg(Color::Black).bg(Color::White)
	} else {
		Style::default()
	};
	let lines = vec![
		Line::from(vec![
			Span::raw("Frequency (Hz): "),
			Span::styled(editor.frequency_hz.clone(), frequency_style),
		]),
		Line::from(vec![
			Span::raw("Duty (%):      "),
			Span::styled(editor.duty_percent.clone(), duty_style),
		]),
		Line::raw(format!(
			"Output: {}    Polarity: {}",
			if editor.enabled {
				"enabled"
			} else {
				"disabled"
			},
			if editor.inverted {
				"inverted"
			} else {
				"normal"
			}
		)),
		Line::raw("Tab field · e enable · i invert · Enter apply · Esc cancel"),
	];
	frame.render_widget(
		Paragraph::new(lines).block(
			Block::default()
				.title(format!("PWM{} output", editor.channel))
				.borders(Borders::ALL),
		),
		area,
	);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
	Rect {
		x: area.x + area.width.saturating_sub(width) / 2,
		y: area.y + area.height.saturating_sub(height) / 2,
		width: width.min(area.width),
		height: height.min(area.height),
	}
}

fn centered_line(width: u16, text: String, style: Style) -> Line<'static> {
	let width = usize::from(width);
	let text = text.chars().take(width).collect::<String>();
	let text_width = text.chars().count();
	let left = width.saturating_sub(text_width) / 2;
	let right = width.saturating_sub(text_width + left);
	Line::from(vec![
		Span::raw(" ".repeat(left)),
		Span::styled(text, style),
		Span::raw(" ".repeat(right)),
	])
}

fn fit(text: &str, width: usize) -> String {
	let mut text = text.chars().take(width).collect::<String>();
	let length = text.chars().count();
	if length < width {
		text.push_str(&" ".repeat(width - length));
	}
	text
}

fn find_gpio(gpio: u8) -> Option<(usize, usize)> {
	LEFT_HEADER
		.iter()
		.position(|pin| pin.gpio == Some(gpio))
		.map(|row| (row, 0))
		.or_else(|| {
			RIGHT_HEADER
				.iter()
				.position(|pin| pin.gpio == Some(gpio))
				.map(|row| (row, 1))
		})
}

fn coupled_signal(signal: PinSignal) -> bool {
	matches!(
		signal,
		PinSignal::Uart0Tx
			| PinSignal::Uart0Rx
			| PinSignal::Uart1Tx
			| PinSignal::Uart1Rx
			| PinSignal::I2c0Sda
			| PinSignal::I2c0Scl
			| PinSignal::I2c1Sda
			| PinSignal::I2c1Scl
			| PinSignal::Spi0Miso
			| PinSignal::Spi0Sck
			| PinSignal::Spi0Mosi
			| PinSignal::Spi1Miso
			| PinSignal::Spi1Sck
			| PinSignal::Spi1Mosi
	)
}

fn enable_target(signal: PinSignal) -> &'static str {
	if matches!(signal, PinSignal::Spi0Cs1 | PinSignal::Spi1Cs1) {
		return display_signal(signal);
	}
	signal
		.serial_peripheral()
		.map_or_else(|| display_signal(signal), peripheral_name)
}

fn signal_toggle_enabled(map: PinMap, signal: PinSignal) -> bool {
	map.is_signal_enabled(signal)
}

fn routed_map(
	mut map: PinMap,
	signal: PinSignal,
	pin: u8,
	grouped: bool,
) -> Result<PinMap, &'static str> {
	if !grouped || !coupled_signal(signal) {
		map.set_pin(signal, pin);
		return Ok(map);
	}

	let routes: &[(PinSignal, u8)] = match signal {
		PinSignal::Uart0Tx => &[
			(PinSignal::Uart0Tx, pin),
			(PinSignal::Uart0Rx, pin.saturating_add(1)),
		],
		PinSignal::Uart0Rx => &[
			(PinSignal::Uart0Tx, pin.saturating_sub(1)),
			(PinSignal::Uart0Rx, pin),
		],
		PinSignal::Uart1Tx => &[
			(PinSignal::Uart1Tx, pin),
			(PinSignal::Uart1Rx, pin.saturating_add(1)),
		],
		PinSignal::Uart1Rx => &[
			(PinSignal::Uart1Tx, pin.saturating_sub(1)),
			(PinSignal::Uart1Rx, pin),
		],
		PinSignal::I2c0Sda => &[
			(PinSignal::I2c0Sda, pin),
			(PinSignal::I2c0Scl, pin.saturating_add(1)),
		],
		PinSignal::I2c0Scl => &[
			(PinSignal::I2c0Sda, pin.saturating_sub(1)),
			(PinSignal::I2c0Scl, pin),
		],
		PinSignal::I2c1Sda => &[
			(PinSignal::I2c1Sda, pin),
			(PinSignal::I2c1Scl, pin.saturating_add(1)),
		],
		PinSignal::I2c1Scl => &[
			(PinSignal::I2c1Sda, pin.saturating_sub(1)),
			(PinSignal::I2c1Scl, pin),
		],
		PinSignal::Spi0Miso => &[
			(PinSignal::Spi0Miso, pin),
			(PinSignal::Spi0Sck, pin.saturating_add(2)),
			(PinSignal::Spi0Mosi, pin.saturating_add(3)),
		],
		PinSignal::Spi0Sck => &[
			(PinSignal::Spi0Miso, pin.saturating_sub(2)),
			(PinSignal::Spi0Sck, pin),
			(PinSignal::Spi0Mosi, pin.saturating_add(1)),
		],
		PinSignal::Spi0Mosi => &[
			(PinSignal::Spi0Miso, pin.saturating_sub(3)),
			(PinSignal::Spi0Sck, pin.saturating_sub(1)),
			(PinSignal::Spi0Mosi, pin),
		],
		PinSignal::Spi1Miso => &[
			(PinSignal::Spi1Miso, pin),
			(PinSignal::Spi1Sck, pin.saturating_add(2)),
			(PinSignal::Spi1Mosi, pin.saturating_add(3)),
		],
		PinSignal::Spi1Sck => &[
			(PinSignal::Spi1Miso, pin.saturating_sub(2)),
			(PinSignal::Spi1Sck, pin),
			(PinSignal::Spi1Mosi, pin.saturating_add(1)),
		],
		PinSignal::Spi1Mosi => &[
			(PinSignal::Spi1Miso, pin.saturating_sub(3)),
			(PinSignal::Spi1Sck, pin.saturating_sub(1)),
			(PinSignal::Spi1Mosi, pin),
		],
		PinSignal::Spi0Cs0
		| PinSignal::Spi0Cs1
		| PinSignal::Spi1Cs0
		| PinSignal::Spi1Cs1
		| PinSignal::Adc0
		| PinSignal::Adc1
		| PinSignal::Adc2
		| PinSignal::Pwm0A
		| PinSignal::Pwm0B
		| PinSignal::Pwm1A
		| PinSignal::Pwm1B
		| PinSignal::Pwm2A
		| PinSignal::Pwm2B
		| PinSignal::Pwm3A
		| PinSignal::Pwm3B
		| PinSignal::Pwm4A
		| PinSignal::Pwm4B
		| PinSignal::Pwm5A
		| PinSignal::Pwm5B
		| PinSignal::Pwm6A
		| PinSignal::Pwm6B
		| PinSignal::Pwm7A
		| PinSignal::Pwm7B
		| PinSignal::Clock0
		| PinSignal::Clock1 => {
			unreachable!()
		}
	};
	if routes
		.iter()
		.any(|(signal, pin)| !signal.valid_pins().contains(pin))
	{
		return Err(match signal {
			PinSignal::Uart0Tx | PinSignal::Uart0Rx | PinSignal::Uart1Tx | PinSignal::Uart1Rx => {
				"selected pin has no complete UART pair on the Pico 2 header"
			}
			PinSignal::I2c0Sda | PinSignal::I2c0Scl | PinSignal::I2c1Sda | PinSignal::I2c1Scl => {
				"selected pin has no complete I²C pair on the Pico 2 header"
			}
			PinSignal::Spi0Miso
			| PinSignal::Spi0Sck
			| PinSignal::Spi0Mosi
			| PinSignal::Spi1Miso
			| PinSignal::Spi1Sck
			| PinSignal::Spi1Mosi => "selected pin has no complete SPI group on the Pico 2 header",
			PinSignal::Spi0Cs0 | PinSignal::Spi0Cs1 | PinSignal::Spi1Cs0 | PinSignal::Spi1Cs1 => {
				unreachable!()
			}
			_ => unreachable!(),
		});
	}
	for &(signal, pin) in routes {
		map.set_pin(signal, pin);
	}
	Ok(map)
}

enum RouteError {
	Incomplete(&'static str),
	Invalid(PinValidationError),
}

fn preferred_routed_map(
	map: PinMap,
	signal: PinSignal,
	pin: u8,
	grouped: bool,
) -> Result<(PinMap, bool), RouteError> {
	let grouped_candidate = routed_map(map, signal, pin, grouped);
	if !grouped {
		return grouped_candidate
			.map(|candidate| (candidate, false))
			.map_err(RouteError::Incomplete);
	}

	match grouped_candidate {
		Ok(candidate) if candidate.validate().is_ok() => {
			return Ok((candidate, coupled_signal(signal)));
		}
		_ => {}
	}

	let mut single = map;
	single.set_pin(signal, pin);
	if single.validate().is_ok() {
		return Ok((single, false));
	}

	match grouped_candidate {
		Ok(candidate) => Err(RouteError::Invalid(candidate.validate().unwrap_err())),
		Err(message) => Err(RouteError::Incomplete(message)),
	}
}

fn display_signal(signal: PinSignal) -> &'static str {
	match signal {
		PinSignal::Uart0Tx => "UART0 TX",
		PinSignal::Uart0Rx => "UART0 RX",
		PinSignal::Uart1Tx => "UART1 TX",
		PinSignal::Uart1Rx => "UART1 RX",
		PinSignal::I2c0Sda => "I²C0 SDA",
		PinSignal::I2c0Scl => "I²C0 SCL",
		PinSignal::I2c1Sda => "I²C1 SDA",
		PinSignal::I2c1Scl => "I²C1 SCL",
		PinSignal::Spi0Miso => "SPI0 MISO",
		PinSignal::Spi0Sck => "SPI0 SCK",
		PinSignal::Spi0Mosi => "SPI0 MOSI",
		PinSignal::Spi0Cs0 => "SPI0 CS0",
		PinSignal::Spi0Cs1 => "SPI0 CS1",
		PinSignal::Spi1Miso => "SPI1 MISO",
		PinSignal::Spi1Sck => "SPI1 SCK",
		PinSignal::Spi1Mosi => "SPI1 MOSI",
		PinSignal::Spi1Cs0 => "SPI1 CS0",
		PinSignal::Spi1Cs1 => "SPI1 CS1",
		PinSignal::Adc0 => "ADC0",
		PinSignal::Adc1 => "ADC1",
		PinSignal::Adc2 => "ADC2",
		PinSignal::Pwm0A => "PWM0 A",
		PinSignal::Pwm0B => "PWM0 B",
		PinSignal::Pwm1A => "PWM1 A",
		PinSignal::Pwm1B => "PWM1 B",
		PinSignal::Pwm2A => "PWM2 A",
		PinSignal::Pwm2B => "PWM2 B",
		PinSignal::Pwm3A => "PWM3 A",
		PinSignal::Pwm3B => "PWM3 B",
		PinSignal::Pwm4A => "PWM4 A",
		PinSignal::Pwm4B => "PWM4 B",
		PinSignal::Pwm5A => "PWM5 A",
		PinSignal::Pwm5B => "PWM5 B",
		PinSignal::Pwm6A => "PWM6 A",
		PinSignal::Pwm6B => "PWM6 B",
		PinSignal::Pwm7A => "PWM7 A",
		PinSignal::Pwm7B => "PWM7 B",
		PinSignal::Clock0 => "CLOCK0",
		PinSignal::Clock1 => "CLOCK1",
	}
}

fn short_signal(signal: PinSignal) -> &'static str {
	match signal {
		PinSignal::Uart0Tx => "U0-TX",
		PinSignal::Uart0Rx => "U0-RX",
		PinSignal::Uart1Tx => "U1-TX",
		PinSignal::Uart1Rx => "U1-RX",
		PinSignal::I2c0Sda => "I0-SDA",
		PinSignal::I2c0Scl => "I0-SCL",
		PinSignal::I2c1Sda => "I1-SDA",
		PinSignal::I2c1Scl => "I1-SCL",
		PinSignal::Spi0Miso => "S0-MI",
		PinSignal::Spi0Sck => "S0-CK",
		PinSignal::Spi0Mosi => "S0-MO",
		PinSignal::Spi0Cs0 => "S0-C0",
		PinSignal::Spi0Cs1 => "S0-C1",
		PinSignal::Spi1Miso => "S1-MI",
		PinSignal::Spi1Sck => "S1-CK",
		PinSignal::Spi1Mosi => "S1-MO",
		PinSignal::Spi1Cs0 => "S1-C0",
		PinSignal::Spi1Cs1 => "S1-C1",
		PinSignal::Adc0 => "ADC0",
		PinSignal::Adc1 => "ADC1",
		PinSignal::Adc2 => "ADC2",
		PinSignal::Pwm0A => "P0-A",
		PinSignal::Pwm0B => "P0-B",
		PinSignal::Pwm1A => "P1-A",
		PinSignal::Pwm1B => "P1-B",
		PinSignal::Pwm2A => "P2-A",
		PinSignal::Pwm2B => "P2-B",
		PinSignal::Pwm3A => "P3-A",
		PinSignal::Pwm3B => "P3-B",
		PinSignal::Pwm4A => "P4-A",
		PinSignal::Pwm4B => "P4-B",
		PinSignal::Pwm5A => "P5-A",
		PinSignal::Pwm5B => "P5-B",
		PinSignal::Pwm6A => "P6-A",
		PinSignal::Pwm6B => "P6-B",
		PinSignal::Pwm7A => "P7-A",
		PinSignal::Pwm7B => "P7-B",
		PinSignal::Clock0 => "CLK0",
		PinSignal::Clock1 => "CLK1",
	}
}

fn signal_style(signal: PinSignal) -> Style {
	Style::default().fg(match signal {
		PinSignal::Uart0Tx | PinSignal::Uart0Rx | PinSignal::Uart1Tx | PinSignal::Uart1Rx => {
			Color::Magenta
		}
		PinSignal::I2c0Sda | PinSignal::I2c0Scl | PinSignal::I2c1Sda | PinSignal::I2c1Scl => {
			Color::Cyan
		}
		PinSignal::Spi0Miso
		| PinSignal::Spi0Sck
		| PinSignal::Spi0Mosi
		| PinSignal::Spi0Cs0
		| PinSignal::Spi0Cs1
		| PinSignal::Spi1Miso
		| PinSignal::Spi1Sck
		| PinSignal::Spi1Mosi
		| PinSignal::Spi1Cs0
		| PinSignal::Spi1Cs1 => Color::Yellow,
		PinSignal::Adc0 | PinSignal::Adc1 | PinSignal::Adc2 => Color::Green,
		PinSignal::Pwm0A
		| PinSignal::Pwm0B
		| PinSignal::Pwm1A
		| PinSignal::Pwm1B
		| PinSignal::Pwm2A
		| PinSignal::Pwm2B
		| PinSignal::Pwm3A
		| PinSignal::Pwm3B
		| PinSignal::Pwm4A
		| PinSignal::Pwm4B
		| PinSignal::Pwm5A
		| PinSignal::Pwm5B
		| PinSignal::Pwm6A
		| PinSignal::Pwm6B
		| PinSignal::Pwm7A
		| PinSignal::Pwm7B => Color::Blue,
		PinSignal::Clock0 | PinSignal::Clock1 => Color::Red,
	})
}

fn peripheral_name(peripheral: Peripheral) -> &'static str {
	match peripheral {
		Peripheral::Uart0 => "UART0",
		Peripheral::Uart1 => "UART1",
		Peripheral::I2c0 => "I²C0",
		Peripheral::I2c1 => "I²C1",
		Peripheral::Spi0 => "SPI0",
		Peripheral::Spi1 => "SPI1",
	}
}

fn bus_numbers(buses: [Option<i32>; 2]) -> String {
	buses
		.into_iter()
		.enumerate()
		.map(|(instance, bus)| format!("{instance}:{}", option(bus)))
		.collect::<Vec<_>>()
		.join(",")
}

fn option(value: Option<impl ToString>) -> String {
	value.map_or_else(|| "-".into(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
	use super::*;
	use ratatui::{Terminal, backend::TestBackend};
	use rp_extio_wire::HEADER_PINS;

	fn app() -> App {
		App {
			boards: vec![BoardInfo {
				serial: "PICO2".into(),
				connected: true,
				active: Some(DEFAULT_PIN_MAP.into()),
				desired: Some(DEFAULT_PIN_MAP.into()),
				generation: Some(4),
				i2c_buses: [Some(27), None],
				spi_buses: [Some(0), None],
				gpio_instance: Some(3),
				iio_device: Some(0),
				pwm_chip: Some(0),
				onboard_led: None,
				gpio_startup: vec![],
				pwm_startup: vec![],
				buttons: vec![],
			}],
			board: 0,
			signal: 0,
			cursor_row: 0,
			cursor_side: 0,
			edited: Some(DEFAULT_PIN_MAP),
			grouped: true,
			flipped: false,
			mode: UiMode::Route,
			operate_selection: OperateSelection::Pin,
			gpio: Vec::new(),
			adc: None,
			pwm: None,
			pwm_editor: None,
			telemetry_error: None,
			last_refresh: Instant::now(),
			default_pending: false,
			message: String::new(),
			message_error: false,
		}
	}

	fn rendered(width: u16, height: u16, app: &App) -> String {
		let backend = TestBackend::new(width, height);
		let mut terminal = Terminal::new(backend).unwrap();
		terminal.draw(|frame| draw(frame, app)).unwrap();
		terminal.backend().to_string()
	}

	#[test]
	fn physical_headers_contain_every_exposed_gpio_once() {
		let mut actual = LEFT_HEADER
			.into_iter()
			.chain(RIGHT_HEADER)
			.filter_map(|pin| pin.gpio)
			.collect::<Vec<_>>();
		actual.sort_unstable();
		assert_eq!(actual, HEADER_PINS);
		assert_eq!(LEFT_HEADER[0].physical, 1);
		assert_eq!(RIGHT_HEADER[0].physical, 40);
		assert_eq!(LEFT_HEADER[19].physical, 20);
		assert_eq!(RIGHT_HEADER[19].physical, 21);
	}

	#[test]
	fn operate_mode_renders_gpio_adc_and_pwm_state() {
		let mut app = app();
		app.mode = UiMode::Operate;
		app.gpio = vec![GpioInfo {
			line: 0,
			pin: 2,
			level: true,
			output: false,
			bias: GpioBias::PullUp,
		}];
		app.boards[0].gpio_startup = vec![GpioStartup::Input {
			pin: 2,
			bias: GpioBias::PullUp,
		}];
		app.boards[0].buttons = vec![ButtonConfig {
			pin: 2,
			active_low: true,
		}];
		(app.cursor_row, app.cursor_side) = find_gpio(2).unwrap();
		let gpio = rendered(120, 40, &app);
		assert!(gpio.contains("OPERATE"));
		assert!(gpio.contains("GPIO I↑ H"));
		assert!(gpio.contains("GPIO I↑ H •"));
		assert!(gpio.contains("B↓"));
		assert!(gpio.contains("startup input, bias pull-up"));
		assert!(gpio.contains("button released (active low)"));
		assert!(gpio.contains("live state refreshes while idle"));

		app.adc = Some(AdcReading {
			channel: 0,
			raw: 2_048,
			reference_mv: 3_300,
			resolution_bits: 12,
		});
		let adc = rendered(120, 40, &app);
		assert!(adc.contains("ADC0: 2048 raw · 1650 mV"));

		app.adc = None;
		let mut map = DEFAULT_PIN_MAP;
		map.set_signal_enabled(PinSignal::Pwm1A, true);
		app.boards[0].active = Some(map.into());
		app.edited = Some(map);
		app.signal = PinSignal::Pwm1A as usize;
		app.operate_selection = OperateSelection::Signal;
		app.pwm = Some(PwmInfo {
			channel: 2,
			enabled: true,
			inverted: false,
			period_ns: 1_000_000,
			duty_ns: 250_000,
		});
		app.boards[0].pwm_startup = vec![app.pwm.unwrap()];
		(app.cursor_row, app.cursor_side) = find_gpio(map.pin(PinSignal::Pwm1A)).unwrap();
		let pwm = rendered(120, 40, &app);
		assert!(pwm.contains("PWM2: output enabled · 1000 Hz · 25.0% · normal"));
		assert!(pwm.contains("startup 1000 Hz/25.0%"));
		assert!(pwm.contains("P1-A •"));

		app.pwm = None;
		app.signal = PinSignal::Clock0 as usize;
		let clock = rendered(120, 40, &app);
		assert!(clock.contains("persisted in pin map"));
	}

	#[test]
	fn pwm_editor_starts_from_live_state() {
		let mut app = app();
		let mut map = DEFAULT_PIN_MAP;
		app.signal = PinSignal::Pwm1A as usize;
		app.operate_selection = OperateSelection::Signal;
		map.set_signal_enabled(PinSignal::Pwm1A, true);
		app.boards[0].active = Some(map.into());
		app.edited = Some(map);
		app.pwm = Some(PwmInfo {
			channel: 2,
			enabled: true,
			inverted: true,
			period_ns: 1_000_000,
			duty_ns: 250_000,
		});
		app.open_pwm_editor();
		let editor = app.pwm_editor.unwrap();
		assert_eq!(editor.channel, 2);
		assert_eq!(editor.frequency_hz, "1000");
		assert_eq!(editor.duty_percent, "25.0");
		assert!(editor.enabled);
		assert!(editor.inverted);
	}

	#[test]
	fn operate_mode_arrows_select_pin_roles_and_tab_selects_signals() {
		let mut app = app();
		let mut map = DEFAULT_PIN_MAP;
		map.set_signal_enabled(PinSignal::Pwm5A, true);
		app.boards[0].active = Some(map.into());
		app.edited = Some(map);
		app.mode = UiMode::Operate;
		(app.cursor_row, app.cursor_side) = find_gpio(map.pin(PinSignal::Pwm5A)).unwrap();
		app.move_cursor(0, 0);
		assert_eq!(app.operate_selection, OperateSelection::Pin);
		assert_eq!(app.operation_signal(), Some(PinSignal::Pwm5A));
		assert!(rendered(120, 40, &app).contains("GP10 → PWM5 A"));

		(app.cursor_row, app.cursor_side) = find_gpio(11).unwrap();
		app.move_cursor(0, 0);
		assert_eq!(app.operation_signal(), None);
		assert!(rendered(120, 40, &app).contains("GP11 → GPIO"));

		app.signal = PinSignal::Adc1 as usize;
		app.cycle_signal(1);
		assert_eq!(app.operate_selection, OperateSelection::Signal);
		assert_eq!(app.operation_signal(), Some(PinSignal::Adc2));
		assert!(rendered(120, 40, &app).contains("ADC2 → GP28"));
	}

	#[test]
	fn assignment_rejects_fixed_and_invalid_route_pins() {
		let mut app = app();
		app.cursor_row = 2;
		app.assign_selected();
		assert!(app.message.contains("cannot carry"));
		assert!(app.message_error);

		app.cursor_row = 5;
		app.assign_selected();
		assert!(app.message.contains("not a valid route"));
		assert!(app.message_error);
		assert_eq!(app.edited.unwrap(), DEFAULT_PIN_MAP);
	}

	#[test]
	fn defaults_remain_dirty_until_applied_or_undone() {
		let mut app = app();
		app.load_default();
		assert!(app.default_pending);
		assert!(app.dirty());

		app.undo();
		assert!(!app.default_pending);
		assert!(!app.dirty());
	}

	#[test]
	fn reloading_current_board_preserves_the_selected_signal() {
		let mut app = app();
		app.signal = PinSignal::Adc2 as usize;
		app.cursor_row = 0;
		app.cursor_side = 0;

		app.load_current();

		assert_eq!(app.selected_signal(), PinSignal::Adc2);
		assert_eq!(
			(app.cursor_row, app.cursor_side),
			find_gpio(DEFAULT_PIN_MAP.adc_pins[2]).unwrap()
		);
	}

	#[test]
	fn grouped_assignment_moves_i2c_uart_and_spi_routes_together() {
		let mut app = app();

		app.signal = PinSignal::I2c0Sda as usize;
		(app.cursor_row, app.cursor_side) = find_gpio(8).unwrap();
		app.assign_selected();
		let map = app.edited.unwrap();
		assert_eq!((map.i2c_sda[0], map.i2c_scl[0]), (8, 9));
		assert!(map.validate().is_ok());

		app.signal = PinSignal::Uart0Rx as usize;
		(app.cursor_row, app.cursor_side) = find_gpio(15).unwrap();
		app.assign_selected();
		let map = app.edited.unwrap();
		assert_eq!((map.uart_tx[0], map.uart_rx[0]), (14, 15));
		assert!(map.validate().is_ok());

		app.signal = PinSignal::Spi0Sck as usize;
		(app.cursor_row, app.cursor_side) = find_gpio(6).unwrap();
		app.assign_selected();
		let map = app.edited.unwrap();
		assert_eq!(
			(map.spi_miso[0], map.spi_sck[0], map.spi_mosi[0]),
			(4, 6, 7)
		);
		assert!(map.validate().is_ok());
	}

	#[test]
	fn grouped_assignment_rejects_conflicts_without_changing_the_draft() {
		let mut app = app();
		app.signal = PinSignal::Spi0Miso as usize;
		app.cursor_row = 0;
		app.assign_selected();
		assert_eq!(app.edited.unwrap(), DEFAULT_PIN_MAP);
		assert!(app.message.contains("grouped move rejected"));
		assert!(app.message.contains("assigned to both"));
		assert!(app.message_error);
		assert!(!app.dirty());
	}

	#[test]
	fn individual_assignment_retains_manual_invalid_drafts() {
		let mut app = app();
		app.toggle_grouped();
		app.signal = PinSignal::Spi0Miso as usize;
		app.cursor_row = 0;
		app.assign_selected();
		assert_eq!(app.edited.unwrap().spi_miso[0], 0);
		assert!(app.message.contains("assigned to both"));
		assert!(app.message_error);
		assert!(app.dirty());
	}

	#[test]
	fn grouped_assignment_falls_back_to_valid_single_route() {
		let mut app = app();
		app.signal = PinSignal::Spi0Sck as usize;
		(app.cursor_row, app.cursor_side) = find_gpio(2).unwrap();
		app.assign_selected();
		let map = app.edited.unwrap();
		assert_eq!(map.spi_sck[0], 2);
		assert_eq!(map.spi_miso[0], DEFAULT_PIN_MAP.spi_miso[0]);
		assert_eq!(map.spi_mosi[0], DEFAULT_PIN_MAP.spi_mosi[0]);
		assert!(map.validate().is_ok());
		assert!(app.message.contains("updated individually"));
		assert!(!app.message_error);
	}

	#[test]
	fn chip_selects_remain_individually_assignable_in_grouped_mode() {
		let mut app = app();
		app.signal = PinSignal::Spi0Cs0 as usize;
		(app.cursor_row, app.cursor_side) = find_gpio(10).unwrap();
		app.assign_selected();
		let map = app.edited.unwrap();
		assert_eq!(map.spi_chip_selects[0], [10, 21]);
		assert_eq!(
			(map.spi_miso[0], map.spi_sck[0], map.spi_mosi[0]),
			(16, 18, 19)
		);
		assert!(map.validate().is_ok());
	}

	#[test]
	fn assigning_disabled_peripheral_auto_enables_or_rejects_atomically() {
		let mut app = app();
		app.signal = PinSignal::Uart0Tx as usize;
		app.toggle_selected_enabled();
		let map = app.edited.unwrap();
		assert!(!map.is_enabled(Peripheral::Uart0));
		assert_eq!(pin_role(LEFT_HEADER[0], &app).0, "GPIO");

		app.signal = PinSignal::Uart1Tx as usize;
		(app.cursor_row, app.cursor_side) = find_gpio(4).unwrap();
		app.assign_selected();
		let map = app.edited.unwrap();
		assert_eq!((map.uart_tx[1], map.uart_rx[1]), (8, 9));
		assert!(!map.is_enabled(Peripheral::Uart1));
		assert!(app.message.contains("cannot assign and enable UART1"));
		assert!(app.message_error);

		(app.cursor_row, app.cursor_side) = find_gpio(6).unwrap();
		app.assign_selected();
		let map = app.edited.unwrap();
		assert_eq!((map.uart_tx[1], map.uart_rx[1]), (6, 7));
		assert!(map.is_enabled(Peripheral::Uart1));
		assert!(app.message.contains("UART1 routed and enabled"));
		assert!(!app.message_error);
	}

	#[test]
	fn second_chip_select_can_be_disabled_without_disabling_spi() {
		let mut app = app();
		app.signal = PinSignal::Spi0Cs1 as usize;
		let (row, side) = find_gpio(DEFAULT_PIN_MAP.spi_chip_selects[0][1]).unwrap();

		app.toggle_selected_enabled();
		let map = app.edited.unwrap();
		assert!(map.is_enabled(Peripheral::Spi0));
		assert!(!map.spi_cs1_enabled(0));
		assert_eq!(map.spi_chip_select_count(0), 1);
		assert_eq!(
			pin_role([LEFT_HEADER, RIGHT_HEADER][side][row], &app).0,
			"GPIO"
		);
		assert!(app.message.contains("SPI0 CS1 disabled"));

		(app.cursor_row, app.cursor_side) = (row, side);
		app.assign_selected();
		assert!(app.edited.unwrap().spi_cs1_enabled(0));
		assert!(app.message.contains("SPI0 CS1 routed and enabled"));
	}

	#[test]
	fn grouped_assignment_keeps_instance_one_signals_together() {
		let mut app = app();
		app.signal = PinSignal::Spi1Sck as usize;
		(app.cursor_row, app.cursor_side) = find_gpio(10).unwrap();
		app.assign_selected();
		let map = app.edited.unwrap();
		assert_eq!(
			(map.spi_miso[1], map.spi_sck[1], map.spi_mosi[1]),
			(8, 10, 11)
		);
		assert_eq!(
			(map.spi_miso[0], map.spi_sck[0], map.spi_mosi[0]),
			(16, 18, 19)
		);
	}

	#[test]
	fn flipped_view_mirrors_headers_and_horizontal_navigation() {
		let mut app = app();
		let front = rendered(120, 40, &app);
		let front_top_row = front
			.lines()
			.find(|line| line.contains("GP0") && line.contains("VBUS"))
			.unwrap();
		assert!(front_top_row.find("GP0").unwrap() < front_top_row.find("VBUS").unwrap());

		app.flip();
		let output = rendered(120, 40, &app);
		assert!(output.contains("back (belly up) · USB end"));
		assert!(output.find("USB CONNECTOR").unwrap() < output.find("SWD DEBUG").unwrap());
		let back_top_row = output
			.lines()
			.find(|line| line.contains("GP0") && line.contains("VBUS"))
			.unwrap();
		assert!(back_top_row.find("VBUS").unwrap() < back_top_row.find("GP0").unwrap());
		app.cursor_row = 19;
		app.cursor_side = 1;
		assert_eq!(app.selected_pin().physical, 21);
		assert_eq!(app.screen_cursor_row(), 19);
		app.cursor_row = 0;
		app.cursor_side = 0;
		assert_eq!(app.selected_pin().physical, 1);
		assert_eq!(app.screen_cursor_row(), 0);

		app.cursor_row = 10;
		app.cursor_side = 1;
		app.move_cursor(1, 0);
		assert_eq!(app.cursor_row, 11);
		app.move_cursor(0, 1);
		assert_eq!(app.cursor_side, 0);
	}

	#[test]
	fn wide_render_contains_physical_orientation_and_routes() {
		let output = rendered(120, 40, &app());
		assert!(output.contains("USB CONNECTOR"));
		assert!(output.contains(" 1 GP0"));
		assert!(output.contains("VBUS 40"));
		assert!(output.contains("UART0 TX"));
		assert!(output.contains("SWD DEBUG"));
	}

	#[test]
	fn compact_render_scrolls_around_selected_pin() {
		let mut app = app();
		app.cursor_row = 19;
		let output = rendered(80, 20, &app);
		assert!(output.contains("▲ more · header rows"));
		assert!(output.contains("GP15"));
		assert!(output.contains("GP16"));
		assert!(output.contains("SWD DEBUG"));
	}

	#[test]
	fn compact_render_marks_omitted_lower_rows() {
		let output = rendered(80, 20, &app());
		assert!(output.contains("USB CONNECTOR · rows 1–"));
		assert!(output.contains("▼ more header rows"));
		assert!(!output.contains("SWD DEBUG"));
	}

	#[test]
	fn short_wide_legend_scrolls_to_selected_signal() {
		let mut app = app();
		app.signal = PinSignal::Spi1Cs1 as usize;
		let output = rendered(120, 20, &app);
		assert!(output.contains("SPI1 CS1"));
		assert!(!output.contains("UART0 TX"));
	}

	#[test]
	fn auxiliary_signals_move_individually_and_clocks_are_adjustable() {
		let mut app = app();
		app.edited
			.as_mut()
			.unwrap()
			.set_enabled(Peripheral::Spi0, false);
		app.signal = PinSignal::Pwm1A as usize;
		(app.cursor_row, app.cursor_side) = find_gpio(18).unwrap();
		app.assign_selected();
		let map = app.edited.unwrap();
		assert_eq!(map.pwm_pins[2], 18);
		assert!(map.is_signal_enabled(PinSignal::Pwm1A));

		app.signal = PinSignal::Clock0 as usize;
		app.cycle_clock_source();
		assert_eq!(app.edited.unwrap().clock_sources[0], ClockSource::Reference);
		app.adjust_clock_frequency(true);
		assert_eq!(app.edited.unwrap().clock_hz[0], 2_000_000);
	}

	#[test]
	fn tiny_render_reports_minimum_size() {
		let output = rendered(40, 8, &app());
		assert!(output.contains("Terminal too small"));
	}
}
