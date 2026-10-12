#![no_std]

pub const USB_MAGIC: [u8; 4] = *b"RPXB";
pub const KERNEL_MAGIC: [u8; 4] = *b"RPKD";
pub const VERSION_MAJOR: u8 = 4;
pub const VERSION_MINOR: u8 = 0;
pub const USB_HEADER_LEN: usize = 24;
pub const KERNEL_HEADER_LEN: usize = 32;
pub const MAX_PAYLOAD: usize = 4096;
pub const MAX_DATA: usize = 3584;
pub const MAX_I2C_MESSAGES: usize = 16;
pub const MAX_SPI_SEGMENTS: usize = 16;
pub const USB_REPLY: u8 = 1;
pub const SERIAL_FRAME_VERSION: u8 = 1;
pub const CONTROL_SERIAL_BAUD: u32 = 921_600;
pub const MAX_SERIAL_BODY: usize = USB_HEADER_LEN + MAX_PAYLOAD;
pub const MAX_SERIAL_RAW_FRAME: usize = 8 + MAX_SERIAL_BODY + 4;
pub const MAX_SERIAL_FRAME: usize = MAX_SERIAL_RAW_FRAME + MAX_SERIAL_RAW_FRAME / 254 + 2;
pub const CAP_GENERAL: u16 = 1;
pub const CAP_I2C: u16 = 2;
pub const CAP_SPI: u16 = 3;
pub const CAP_UART: u16 = 4;
pub const UART_CAP_DAEMON_BACKED: u8 = 1 << 0;
pub const UART_REQUEST_HEADER_LEN: usize = 12;
pub const UART_REPLY_HEADER_LEN: usize = 4;
pub const UART_PARITY_NONE: u8 = 0;
pub const UART_PARITY_EVEN: u8 = 1;
pub const UART_PARITY_ODD: u8 = 2;
pub const CAP_GPIO: u16 = 5;
pub const CAP_PIN_CONFIG: u16 = 6;
pub const CAP_ADC: u16 = 7;
pub const CAP_PWM: u16 = 8;
pub const CAP_CLOCK: u16 = 9;
pub const CAP_BOARD: u16 = 10;
pub const CAP_ONBOARD_LED: u16 = 11;
pub const BOARD_CAPABILITY_VERSION: u8 = 1;
pub const MAX_DEVICE_ID_LEN: usize = 16;
pub const GPIO_GET: u8 = 1;
pub const GPIO_SET: u8 = 2;
pub const GPIO_DIRECTION_INPUT: u8 = 3;
pub const GPIO_DIRECTION_OUTPUT: u8 = 4;
pub const GPIO_GET_DIRECTION: u8 = 5;
pub const GPIO_SNAPSHOT: u8 = 6;
pub const GPIO_SET_BIAS: u8 = 7;
pub const GPIO_BIAS_NONE: u8 = 0;
pub const GPIO_BIAS_PULL_UP: u8 = 1;
pub const GPIO_BIAS_PULL_DOWN: u8 = 2;
pub const PWM_GET: u8 = 1;
pub const PWM_APPLY: u8 = 2;
pub const PWM_ENABLED: u16 = 1 << 0;
pub const PWM_INVERTED: u16 = 1 << 1;
pub const PWM_STATE_LEN: usize = 20;
pub const LED_GET: u8 = 1;
pub const LED_SET: u8 = 2;
pub const LED_STATE_LEN: usize = 8;
pub const HEADER_PINS: [u8; 26] = [
	0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 26, 27, 28,
];
pub const ESP32_C3_GPIO_PINS: [u8; 13] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 18, 19];
pub const ESP32_C3_TRANSPORT_PINS: [u8; 2] = [20, 21];
pub const MAX_GPIO_LINES: usize = HEADER_PINS.len();
pub const ADC_CHANNEL_COUNT: usize = 5;
pub const PICO2_ADC_CHANNEL_COUNT: usize = 3;
pub const ESP32_C3_ADC_CHANNEL_COUNT: usize = 5;
pub const PWM_CHANNEL_COUNT: usize = 16;
pub const ESP32_C3_PWM_CHANNEL_COUNT: usize = 6;
pub const CLOCK_OUTPUT_COUNT: usize = 2;
pub const PIN_SIGNAL_COUNT: usize = 41;
pub const PIN_MAP_LEN: usize = PIN_SIGNAL_COUNT + 15;
const SPI0_CS1_DISABLE_MASK: u8 = 1 << 6;
const SPI1_CS1_DISABLE_MASK: u8 = 1 << 7;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BoardKind {
	Pico2 = 1,
	Esp32C3DevKitM1 = 2,
}

impl TryFrom<u8> for BoardKind {
	type Error = WireError;

	fn try_from(value: u8) -> Result<Self, Self::Error> {
		match value {
			1 => Ok(Self::Pico2),
			2 => Ok(Self::Esp32C3DevKitM1),
			_ => Err(WireError::Unsupported),
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TransportKind {
	UsbBulk = 1,
	Serial = 2,
}

impl TryFrom<u8> for TransportKind {
	type Error = WireError;

	fn try_from(value: u8) -> Result<Self, Self::Error> {
		match value {
			1 => Ok(Self::UsbBulk),
			2 => Ok(Self::Serial),
			_ => Err(WireError::Unsupported),
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PinWarning {
	BootStrap,
	OnboardRgbLed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum OnboardLedKind {
	Monochrome = 1,
	Rgb = 2,
}

impl TryFrom<u8> for OnboardLedKind {
	type Error = WireError;

	fn try_from(value: u8) -> Result<Self, Self::Error> {
		match value {
			1 => Ok(Self::Monochrome),
			2 => Ok(Self::Rgb),
			_ => Err(WireError::Unsupported),
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OnboardLed {
	pub kind: OnboardLedKind,
	pub pin: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoardProfile {
	kind: BoardKind,
}

impl BoardKind {
	pub const fn profile(self) -> BoardProfile {
		BoardProfile { kind: self }
	}
}

impl BoardProfile {
	pub const fn kind(self) -> BoardKind {
		self.kind
	}

	pub const fn name(self) -> &'static str {
		match self.kind {
			BoardKind::Pico2 => "Raspberry Pi Pico 2",
			BoardKind::Esp32C3DevKitM1 => "ESP32-C3-DevKitM-1 v1.6",
		}
	}

	pub const fn transport(self) -> TransportKind {
		match self.kind {
			BoardKind::Pico2 => TransportKind::UsbBulk,
			BoardKind::Esp32C3DevKitM1 => TransportKind::Serial,
		}
	}

	pub const fn gpio_pins(self) -> &'static [u8] {
		match self.kind {
			BoardKind::Pico2 => &HEADER_PINS,
			BoardKind::Esp32C3DevKitM1 => &ESP32_C3_GPIO_PINS,
		}
	}

	pub const fn reserved_pins(self) -> &'static [u8] {
		match self.kind {
			BoardKind::Pico2 => &[],
			BoardKind::Esp32C3DevKitM1 => &ESP32_C3_TRANSPORT_PINS,
		}
	}

	pub const fn default_pin_map(self) -> PinMap {
		match self.kind {
			BoardKind::Pico2 => DEFAULT_PIN_MAP,
			BoardKind::Esp32C3DevKitM1 => ESP32_C3_DEFAULT_PIN_MAP,
		}
	}

	pub const fn supports(self, signal: PinSignal) -> bool {
		match self.kind {
			BoardKind::Pico2 => !matches!(signal, PinSignal::Adc3 | PinSignal::Adc4),
			BoardKind::Esp32C3DevKitM1 => {
				matches!(
					signal,
					PinSignal::Uart1Tx
						| PinSignal::Uart1Rx
						| PinSignal::I2c0Sda
						| PinSignal::I2c0Scl
						| PinSignal::Spi0Miso
						| PinSignal::Spi0Sck
						| PinSignal::Spi0Mosi
						| PinSignal::Spi0Cs0
						| PinSignal::Spi0Cs1
						| PinSignal::Adc0 | PinSignal::Adc1
						| PinSignal::Adc2 | PinSignal::Adc3
						| PinSignal::Adc4 | PinSignal::Pwm0A
						| PinSignal::Pwm0B | PinSignal::Pwm1A
						| PinSignal::Pwm1B | PinSignal::Pwm2A
						| PinSignal::Pwm2B
				)
			}
		}
	}

	pub const fn supported_signals(self) -> u64 {
		let mut mask = 0;
		let mut index = 0;
		while index < PinSignal::ALL.len() {
			let signal = PinSignal::ALL[index];
			if self.supports(signal) {
				mask |= 1 << signal as u8;
			}
			index += 1;
		}
		mask
	}

	pub const fn valid_pins(self, signal: PinSignal) -> &'static [u8] {
		match self.kind {
			BoardKind::Pico2 => signal.valid_pins(),
			BoardKind::Esp32C3DevKitM1 => match signal {
				PinSignal::Adc0 => &[0],
				PinSignal::Adc1 => &[1],
				PinSignal::Adc2 => &[2],
				PinSignal::Adc3 => &[3],
				PinSignal::Adc4 => &[4],
				_ if self.supports(signal) => &ESP32_C3_GPIO_PINS,
				_ => &[],
			},
		}
	}

	pub const fn adc_channel_count(self) -> usize {
		match self.kind {
			BoardKind::Pico2 => PICO2_ADC_CHANNEL_COUNT,
			BoardKind::Esp32C3DevKitM1 => ESP32_C3_ADC_CHANNEL_COUNT,
		}
	}

	pub const fn pwm_channel_count(self) -> usize {
		match self.kind {
			BoardKind::Pico2 => PWM_CHANNEL_COUNT,
			BoardKind::Esp32C3DevKitM1 => ESP32_C3_PWM_CHANNEL_COUNT,
		}
	}

	/// Hardware timer shared by a PWM channel. Channels on the same timer must
	/// use the same period while both are enabled.
	pub const fn pwm_timer(self, channel: usize) -> Option<usize> {
		if channel >= self.pwm_channel_count() {
			return None;
		}
		Some(match self.kind {
			BoardKind::Pico2 => channel / 2,
			BoardKind::Esp32C3DevKitM1 => channel % 4,
		})
	}

	pub const fn pin_warning(self, pin: u8) -> Option<PinWarning> {
		match (self.kind, pin) {
			(BoardKind::Esp32C3DevKitM1, 2 | 9) => Some(PinWarning::BootStrap),
			(BoardKind::Esp32C3DevKitM1, 8) => Some(PinWarning::OnboardRgbLed),
			_ => None,
		}
	}

	pub const fn onboard_led(self) -> OnboardLed {
		match self.kind {
			BoardKind::Pico2 => OnboardLed {
				kind: OnboardLedKind::Monochrome,
				pin: 25,
			},
			BoardKind::Esp32C3DevKitM1 => OnboardLed {
				kind: OnboardLedKind::Rgb,
				pin: 8,
			},
		}
	}

	const fn requires_adjacent_i2c_pins(self) -> bool {
		matches!(self.kind, BoardKind::Pico2)
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceIdentity {
	bytes: [u8; MAX_DEVICE_ID_LEN],
	len: u8,
}

impl DeviceIdentity {
	pub const EMPTY: Self = Self {
		bytes: [0; MAX_DEVICE_ID_LEN],
		len: 0,
	};

	pub fn new(value: &[u8]) -> Result<Self, WireError> {
		if value.is_empty() || value.len() > MAX_DEVICE_ID_LEN {
			return Err(WireError::Invalid);
		}
		let mut identity = Self::EMPTY;
		identity.bytes[..value.len()].copy_from_slice(value);
		identity.len = value.len() as u8;
		Ok(identity)
	}

	pub const fn len(self) -> usize {
		self.len as usize
	}

	pub const fn is_empty(self) -> bool {
		self.len == 0
	}

	pub fn as_bytes(&self) -> &[u8] {
		&self.bytes[..self.len()]
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockDivider {
	pub integer: u16,
	pub fraction: u16,
	pub effective_hz: u32,
}

pub const fn clock_divider(source_hz: u32, target_hz: u32) -> Option<ClockDivider> {
	if source_hz == 0 || target_hz == 0 || target_hz > source_hz {
		return None;
	}
	let divisor = (source_hz as u64 * 65_536 + target_hz as u64 / 2) / target_hz as u64;
	if divisor < 65_536 || divisor > u32::MAX as u64 {
		return None;
	}
	Some(ClockDivider {
		integer: (divisor >> 16) as u16,
		fraction: divisor as u16,
		effective_hz: ((source_hz as u64 * 65_536) / divisor) as u32,
	})
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Operation {
	Hello = 1,
	I2cTransfer = 2,
	SpiMessage = 3,
	GpioCommand = 4,
	ConfigurePins = 5,
	AdcRead = 6,
	Pwm = 7,
	OnboardLed = 8,
	UartTransfer = 9,
}

impl TryFrom<u8> for Operation {
	type Error = WireError;
	fn try_from(value: u8) -> Result<Self, Self::Error> {
		match value {
			1 => Ok(Self::Hello),
			2 => Ok(Self::I2cTransfer),
			3 => Ok(Self::SpiMessage),
			4 => Ok(Self::GpioCommand),
			5 => Ok(Self::ConfigurePins),
			6 => Ok(Self::AdcRead),
			7 => Ok(Self::Pwm),
			8 => Ok(Self::OnboardLed),
			9 => Ok(Self::UartTransfer),
			_ => Err(WireError::Unsupported),
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Status {
	Ok = 0,
	Nack = 1,
	ArbitrationLost = 2,
	Timeout = 3,
	Disconnected = 4,
	Unsupported = 5,
	BadFrame = 6,
	IoError = 7,
	InvalidRoute = 8,
	PinConflict = 9,
	Busy = 10,
}

impl TryFrom<i32> for Status {
	type Error = WireError;
	fn try_from(value: i32) -> Result<Self, Self::Error> {
		match value {
			0 => Ok(Self::Ok),
			1 => Ok(Self::Nack),
			2 => Ok(Self::ArbitrationLost),
			3 => Ok(Self::Timeout),
			4 => Ok(Self::Disconnected),
			5 => Ok(Self::Unsupported),
			6 => Ok(Self::BadFrame),
			7 => Ok(Self::IoError),
			8 => Ok(Self::InvalidRoute),
			9 => Ok(Self::PinConflict),
			10 => Ok(Self::Busy),
			_ => Err(WireError::Invalid),
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
	Truncated,
	Invalid,
	Unsupported,
	TooLarge,
}

pub fn encode_serial_frame(body: &[u8], output: &mut [u8]) -> Result<usize, WireError> {
	if body.len() > MAX_SERIAL_BODY {
		return Err(WireError::TooLarge);
	}
	let raw_len = 8 + body.len() + 4;
	let mut raw = [0; MAX_SERIAL_RAW_FRAME];
	raw[0] = SERIAL_FRAME_VERSION;
	raw[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
	raw[8..8 + body.len()].copy_from_slice(body);
	let checksum = crc32(&raw[..raw_len - 4]);
	raw[raw_len - 4..raw_len].copy_from_slice(&checksum.to_le_bytes());
	let encoded = cobs_encode(&raw[..raw_len], output)?;
	let Some(delimiter) = output.get_mut(encoded) else {
		return Err(WireError::TooLarge);
	};
	*delimiter = 0;
	Ok(encoded + 1)
}

pub fn decode_serial_frame(encoded: &[u8], body: &mut [u8]) -> Result<usize, WireError> {
	let encoded = encoded.strip_suffix(&[0]).unwrap_or(encoded);
	let mut raw = [0; MAX_SERIAL_RAW_FRAME];
	let raw_len = cobs_decode(encoded, &mut raw)?;
	if raw_len < 12
		|| raw[0] != SERIAL_FRAME_VERSION
		|| raw[1..4] != [0; 3]
		|| raw_len != 12 + read_u32(&raw[4..8]) as usize
	{
		return Err(WireError::Invalid);
	}
	let body_len = raw_len - 12;
	if body_len > body.len() {
		return Err(WireError::TooLarge);
	}
	let expected = read_u32(&raw[raw_len - 4..]);
	if crc32(&raw[..raw_len - 4]) != expected {
		return Err(WireError::Invalid);
	}
	body[..body_len].copy_from_slice(&raw[8..8 + body_len]);
	Ok(body_len)
}

fn cobs_encode(input: &[u8], output: &mut [u8]) -> Result<usize, WireError> {
	let mut read: usize = 0;
	let mut write: usize = 1;
	let mut code_index: usize = 0;
	let mut code = 1_u8;
	while read < input.len() {
		if input[read] == 0 {
			*output.get_mut(code_index).ok_or(WireError::TooLarge)? = code;
			code_index = write;
			write += 1;
			code = 1;
		} else {
			*output.get_mut(write).ok_or(WireError::TooLarge)? = input[read];
			write += 1;
			code += 1;
			if code == 0xff {
				*output.get_mut(code_index).ok_or(WireError::TooLarge)? = code;
				code_index = write;
				write += 1;
				code = 1;
			}
		}
		read += 1;
	}
	*output.get_mut(code_index).ok_or(WireError::TooLarge)? = code;
	Ok(write)
}

fn cobs_decode(input: &[u8], output: &mut [u8]) -> Result<usize, WireError> {
	if input.is_empty() {
		return Err(WireError::Truncated);
	}
	let mut read: usize = 0;
	let mut write: usize = 0;
	while read < input.len() {
		let code = input[read] as usize;
		if code == 0 || read + code > input.len() + 1 {
			return Err(WireError::Invalid);
		}
		read += 1;
		let count = code - 1;
		let end = write.checked_add(count).ok_or(WireError::TooLarge)?;
		if end > output.len() || read + count > input.len() {
			return Err(WireError::TooLarge);
		}
		output[write..end].copy_from_slice(&input[read..read + count]);
		read += count;
		write = end;
		if code != 0xff && read < input.len() {
			*output.get_mut(write).ok_or(WireError::TooLarge)? = 0;
			write += 1;
		}
	}
	Ok(write)
}

fn crc32(bytes: &[u8]) -> u32 {
	let mut crc = u32::MAX;
	for &byte in bytes {
		crc ^= u32::from(byte);
		for _ in 0..8 {
			crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
		}
	}
	!crc
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PinSignal {
	Uart0Tx = 0,
	Uart0Rx = 1,
	Uart1Tx = 2,
	Uart1Rx = 3,
	I2c0Sda = 4,
	I2c0Scl = 5,
	I2c1Sda = 6,
	I2c1Scl = 7,
	Spi0Miso = 8,
	Spi0Sck = 9,
	Spi0Mosi = 10,
	Spi0Cs0 = 11,
	Spi0Cs1 = 12,
	Spi1Miso = 13,
	Spi1Sck = 14,
	Spi1Mosi = 15,
	Spi1Cs0 = 16,
	Spi1Cs1 = 17,
	Adc0 = 18,
	Adc1 = 19,
	Adc2 = 20,
	Adc3 = 21,
	Adc4 = 22,
	Pwm0A = 23,
	Pwm0B = 24,
	Pwm1A = 25,
	Pwm1B = 26,
	Pwm2A = 27,
	Pwm2B = 28,
	Pwm3A = 29,
	Pwm3B = 30,
	Pwm4A = 31,
	Pwm4B = 32,
	Pwm5A = 33,
	Pwm5B = 34,
	Pwm6A = 35,
	Pwm6B = 36,
	Pwm7A = 37,
	Pwm7B = 38,
	Clock0 = 39,
	Clock1 = 40,
}

impl PinSignal {
	pub const ALL: [Self; PIN_SIGNAL_COUNT] = [
		Self::Uart0Tx,
		Self::Uart0Rx,
		Self::Uart1Tx,
		Self::Uart1Rx,
		Self::I2c0Sda,
		Self::I2c0Scl,
		Self::I2c1Sda,
		Self::I2c1Scl,
		Self::Spi0Miso,
		Self::Spi0Sck,
		Self::Spi0Mosi,
		Self::Spi0Cs0,
		Self::Spi0Cs1,
		Self::Spi1Miso,
		Self::Spi1Sck,
		Self::Spi1Mosi,
		Self::Spi1Cs0,
		Self::Spi1Cs1,
		Self::Adc0,
		Self::Adc1,
		Self::Adc2,
		Self::Adc3,
		Self::Adc4,
		Self::Pwm0A,
		Self::Pwm0B,
		Self::Pwm1A,
		Self::Pwm1B,
		Self::Pwm2A,
		Self::Pwm2B,
		Self::Pwm3A,
		Self::Pwm3B,
		Self::Pwm4A,
		Self::Pwm4B,
		Self::Pwm5A,
		Self::Pwm5B,
		Self::Pwm6A,
		Self::Pwm6B,
		Self::Pwm7A,
		Self::Pwm7B,
		Self::Clock0,
		Self::Clock1,
	];

	pub const fn valid_pins(self) -> &'static [u8] {
		match self {
			Self::Uart0Tx => &[0, 2, 12, 14, 16, 18, 28],
			Self::Uart0Rx => &[1, 3, 13, 15, 17, 19],
			Self::Uart1Tx => &[4, 6, 8, 10, 20, 22, 26],
			Self::Uart1Rx => &[5, 7, 9, 11, 21, 27],
			Self::I2c0Sda => &[0, 4, 8, 12, 16, 20],
			Self::I2c0Scl => &[1, 5, 9, 13, 17, 21],
			Self::I2c1Sda => &[2, 6, 10, 14, 18, 22, 26],
			Self::I2c1Scl => &[3, 7, 11, 15, 19, 27],
			Self::Spi0Miso => &[0, 4, 16, 20],
			Self::Spi0Sck => &[2, 6, 18, 22],
			Self::Spi0Mosi => &[3, 7, 19],
			Self::Spi1Miso => &[8, 12, 28],
			Self::Spi1Sck => &[10, 14, 26],
			Self::Spi1Mosi => &[11, 15, 27],
			Self::Spi0Cs0 | Self::Spi0Cs1 | Self::Spi1Cs0 | Self::Spi1Cs1 => &HEADER_PINS,
			Self::Adc0 => &[26],
			Self::Adc1 => &[27],
			Self::Adc2 => &[28],
			Self::Adc3 | Self::Adc4 => &[],
			Self::Pwm0A => &[0, 16],
			Self::Pwm0B => &[1, 17],
			Self::Pwm1A => &[2, 18],
			Self::Pwm1B => &[3, 19],
			Self::Pwm2A => &[4, 20],
			Self::Pwm2B => &[5, 21],
			Self::Pwm3A => &[6, 22],
			Self::Pwm3B => &[7],
			Self::Pwm4A => &[8],
			Self::Pwm4B => &[9],
			Self::Pwm5A => &[10, 26],
			Self::Pwm5B => &[11, 27],
			Self::Pwm6A => &[12, 28],
			Self::Pwm6B => &[13],
			Self::Pwm7A => &[14],
			Self::Pwm7B => &[15],
			Self::Clock0 => &[13, 21],
			Self::Clock1 => &[15],
		}
	}

	pub const fn serial_peripheral(self) -> Option<Peripheral> {
		Some(match self {
			Self::Uart0Tx | Self::Uart0Rx => Peripheral::Uart0,
			Self::Uart1Tx | Self::Uart1Rx => Peripheral::Uart1,
			Self::I2c0Sda | Self::I2c0Scl => Peripheral::I2c0,
			Self::I2c1Sda | Self::I2c1Scl => Peripheral::I2c1,
			Self::Spi0Miso | Self::Spi0Sck | Self::Spi0Mosi | Self::Spi0Cs0 | Self::Spi0Cs1 => {
				Peripheral::Spi0
			}
			Self::Spi1Miso | Self::Spi1Sck | Self::Spi1Mosi | Self::Spi1Cs0 | Self::Spi1Cs1 => {
				Peripheral::Spi1
			}
			_ => return None,
		})
	}

	pub const fn adc_channel(self) -> Option<usize> {
		let value = self as usize;
		if value >= Self::Adc0 as usize && value <= Self::Adc4 as usize {
			Some(value - Self::Adc0 as usize)
		} else {
			None
		}
	}

	pub const fn pwm_channel(self) -> Option<usize> {
		let value = self as usize;
		if value >= Self::Pwm0A as usize && value <= Self::Pwm7B as usize {
			Some(value - Self::Pwm0A as usize)
		} else {
			None
		}
	}

	pub const fn clock_output(self) -> Option<usize> {
		let value = self as usize;
		if value >= Self::Clock0 as usize && value <= Self::Clock1 as usize {
			Some(value - Self::Clock0 as usize)
		} else {
			None
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ClockSource {
	System = 0,
	Reference = 1,
	Usb = 2,
	Adc = 3,
	Crystal = 4,
	RingOscillator = 5,
	LowPowerOscillator = 6,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PwmState {
	pub channel: u8,
	pub enabled: bool,
	pub inverted: bool,
	pub period_ns: u64,
	pub duty_ns: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LedState {
	pub index: u8,
	pub red: u8,
	pub green: u8,
	pub blue: u8,
}

impl LedState {
	pub const OFF: Self = Self {
		index: 0,
		red: 0,
		green: 0,
		blue: 0,
	};

	pub fn encode(self, command: u8) -> [u8; LED_STATE_LEN] {
		[
			command, self.index, self.red, self.green, self.blue, 0, 0, 0,
		]
	}

	pub fn decode(bytes: &[u8]) -> Result<(u8, Self), WireError> {
		if bytes.len() != LED_STATE_LEN {
			return Err(WireError::Truncated);
		}
		if !matches!(bytes[0], LED_GET | LED_SET) || bytes[1] != 0 || bytes[5..] != [0; 3] {
			return Err(WireError::Invalid);
		}
		Ok((
			bytes[0],
			Self {
				index: bytes[1],
				red: bytes[2],
				green: bytes[3],
				blue: bytes[4],
			},
		))
	}
}

impl Default for LedState {
	fn default() -> Self {
		Self::OFF
	}
}

impl PwmState {
	pub fn encode(self, command: u8) -> [u8; PWM_STATE_LEN] {
		let mut bytes = [0; PWM_STATE_LEN];
		bytes[0] = command;
		bytes[1] = self.channel;
		let flags =
			(u16::from(self.enabled) * PWM_ENABLED) | (u16::from(self.inverted) * PWM_INVERTED);
		bytes[2..4].copy_from_slice(&flags.to_le_bytes());
		bytes[4..12].copy_from_slice(&self.period_ns.to_le_bytes());
		bytes[12..20].copy_from_slice(&self.duty_ns.to_le_bytes());
		bytes
	}

	pub fn decode(bytes: &[u8]) -> Result<(u8, Self), WireError> {
		if bytes.len() != PWM_STATE_LEN {
			return Err(WireError::Truncated);
		}
		let command = bytes[0];
		let channel = bytes[1];
		let flags = read_u16(&bytes[2..4]);
		let period_ns = read_u64(&bytes[4..12]);
		let duty_ns = read_u64(&bytes[12..20]);
		if !matches!(command, PWM_GET | PWM_APPLY)
			|| channel as usize >= PWM_CHANNEL_COUNT
			|| flags & !(PWM_ENABLED | PWM_INVERTED) != 0
			|| duty_ns > period_ns
			|| (flags & PWM_ENABLED != 0 && period_ns == 0)
		{
			return Err(WireError::Invalid);
		}
		Ok((
			command,
			Self {
				channel,
				enabled: flags & PWM_ENABLED != 0,
				inverted: flags & PWM_INVERTED != 0,
				period_ns,
				duty_ns,
			},
		))
	}
}

impl TryFrom<u8> for ClockSource {
	type Error = WireError;

	fn try_from(value: u8) -> Result<Self, Self::Error> {
		match value {
			0 => Ok(Self::System),
			1 => Ok(Self::Reference),
			2 => Ok(Self::Usb),
			3 => Ok(Self::Adc),
			4 => Ok(Self::Crystal),
			5 => Ok(Self::RingOscillator),
			6 => Ok(Self::LowPowerOscillator),
			_ => Err(WireError::Invalid),
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Peripheral {
	Uart0 = 0,
	Uart1 = 1,
	I2c0 = 2,
	I2c1 = 3,
	Spi0 = 4,
	Spi1 = 5,
}

impl Peripheral {
	pub const ALL: [Self; 6] = [
		Self::Uart0,
		Self::Uart1,
		Self::I2c0,
		Self::I2c1,
		Self::Spi0,
		Self::Spi1,
	];

	pub const fn mask(self) -> u8 {
		1 << self as u8
	}

	pub const fn signals(self) -> &'static [PinSignal] {
		match self {
			Self::Uart0 => &[PinSignal::Uart0Tx, PinSignal::Uart0Rx],
			Self::Uart1 => &[PinSignal::Uart1Tx, PinSignal::Uart1Rx],
			Self::I2c0 => &[PinSignal::I2c0Sda, PinSignal::I2c0Scl],
			Self::I2c1 => &[PinSignal::I2c1Sda, PinSignal::I2c1Scl],
			Self::Spi0 => &[
				PinSignal::Spi0Miso,
				PinSignal::Spi0Sck,
				PinSignal::Spi0Mosi,
				PinSignal::Spi0Cs0,
				PinSignal::Spi0Cs1,
			],
			Self::Spi1 => &[
				PinSignal::Spi1Miso,
				PinSignal::Spi1Sck,
				PinSignal::Spi1Mosi,
				PinSignal::Spi1Cs0,
				PinSignal::Spi1Cs1,
			],
		}
	}
}

impl TryFrom<u8> for PinSignal {
	type Error = WireError;

	fn try_from(value: u8) -> Result<Self, Self::Error> {
		Self::ALL
			.get(value as usize)
			.copied()
			.ok_or(WireError::Invalid)
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PinValidationKind {
	UnavailablePin = 1,
	InvalidRoute = 2,
	MismatchedPair = 3,
	DuplicatePin = 4,
	InvalidEnableMask = 5,
	UnsupportedSignal = 6,
	ReservedPin = 7,
}

impl TryFrom<u8> for PinValidationKind {
	type Error = WireError;

	fn try_from(value: u8) -> Result<Self, Self::Error> {
		match value {
			1 => Ok(Self::UnavailablePin),
			2 => Ok(Self::InvalidRoute),
			3 => Ok(Self::MismatchedPair),
			4 => Ok(Self::DuplicatePin),
			5 => Ok(Self::InvalidEnableMask),
			6 => Ok(Self::UnsupportedSignal),
			7 => Ok(Self::ReservedPin),
			_ => Err(WireError::Invalid),
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PinValidationError {
	pub kind: PinValidationKind,
	pub signal: PinSignal,
	pub other: Option<PinSignal>,
	pub pin: u8,
}

impl PinValidationError {
	pub const ENCODED_LEN: usize = 4;

	pub const fn encode(self) -> [u8; Self::ENCODED_LEN] {
		[
			self.kind as u8,
			self.signal as u8,
			match self.other {
				Some(signal) => signal as u8,
				None => u8::MAX,
			},
			self.pin,
		]
	}

	pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
		if bytes.len() != Self::ENCODED_LEN {
			return Err(WireError::Truncated);
		}
		Ok(Self {
			kind: PinValidationKind::try_from(bytes[0])?,
			signal: PinSignal::try_from(bytes[1])?,
			other: if bytes[2] == u8::MAX {
				None
			} else {
				Some(PinSignal::try_from(bytes[2])?)
			},
			pin: bytes[3],
		})
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PinMap {
	pub uart_tx: [u8; 2],
	pub uart_rx: [u8; 2],
	pub i2c_sda: [u8; 2],
	pub i2c_scl: [u8; 2],
	pub spi_miso: [u8; 2],
	pub spi_sck: [u8; 2],
	pub spi_mosi: [u8; 2],
	pub spi_chip_selects: [[u8; 2]; 2],
	pub enabled: u8,
	pub adc_pins: [u8; ADC_CHANNEL_COUNT],
	pub adc_enabled: u8,
	pub pwm_pins: [u8; PWM_CHANNEL_COUNT],
	pub pwm_enabled: u16,
	pub clock_pins: [u8; CLOCK_OUTPUT_COUNT],
	pub clock_enabled: u8,
	pub clock_sources: [ClockSource; CLOCK_OUTPUT_COUNT],
	pub clock_hz: [u32; CLOCK_OUTPUT_COUNT],
}

pub const DEFAULT_PIN_MAP: PinMap = PinMap {
	uart_tx: [0, 8],
	uart_rx: [1, 9],
	i2c_sda: [4, 10],
	i2c_scl: [5, 11],
	spi_miso: [16, 12],
	spi_sck: [18, 14],
	spi_mosi: [19, 15],
	spi_chip_selects: [[17, 21], [13, 20]],
	enabled: Peripheral::Uart0.mask() | Peripheral::I2c0.mask() | Peripheral::Spi0.mask(),
	adc_pins: [26, 27, 28, 0, 0],
	adc_enabled: 0,
	pwm_pins: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
	pwm_enabled: 0,
	clock_pins: [13, 15],
	clock_enabled: 0,
	clock_sources: [ClockSource::System, ClockSource::System],
	clock_hz: [1_000_000, 1_000_000],
};

pub const ESP32_C3_DEFAULT_PIN_MAP: PinMap = PinMap {
	uart_tx: [20, 6],
	uart_rx: [21, 7],
	i2c_sda: [4, 0],
	i2c_scl: [5, 1],
	spi_miso: [3, 0],
	spi_sck: [6, 1],
	spi_mosi: [7, 2],
	spi_chip_selects: [[10, 1], [3, 4]],
	enabled: Peripheral::I2c0.mask() | Peripheral::Spi0.mask(),
	adc_pins: [0, 1, 2, 3, 4],
	adc_enabled: 0,
	pwm_pins: [0, 1, 2, 3, 4, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
	pwm_enabled: 0,
	clock_pins: [0, 0],
	clock_enabled: 0,
	clock_sources: [ClockSource::System, ClockSource::System],
	clock_hz: [1_000_000, 1_000_000],
};

impl Default for PinMap {
	fn default() -> Self {
		DEFAULT_PIN_MAP
	}
}

impl PinMap {
	pub const fn encode(self) -> [u8; PIN_MAP_LEN] {
		let mut bytes = [0; PIN_MAP_LEN];
		bytes[0] = self.uart_tx[0];
		bytes[1] = self.uart_rx[0];
		bytes[2] = self.uart_tx[1];
		bytes[3] = self.uart_rx[1];
		bytes[4] = self.i2c_sda[0];
		bytes[5] = self.i2c_scl[0];
		bytes[6] = self.i2c_sda[1];
		bytes[7] = self.i2c_scl[1];
		bytes[8] = self.spi_miso[0];
		bytes[9] = self.spi_sck[0];
		bytes[10] = self.spi_mosi[0];
		bytes[11] = self.spi_chip_selects[0][0];
		bytes[12] = self.spi_chip_selects[0][1];
		bytes[13] = self.spi_miso[1];
		bytes[14] = self.spi_sck[1];
		bytes[15] = self.spi_mosi[1];
		bytes[16] = self.spi_chip_selects[1][0];
		bytes[17] = self.spi_chip_selects[1][1];
		bytes[18] = self.adc_pins[0];
		bytes[19] = self.adc_pins[1];
		bytes[20] = self.adc_pins[2];
		bytes[21] = self.adc_pins[3];
		bytes[22] = self.adc_pins[4];
		let mut index = 0;
		while index < PWM_CHANNEL_COUNT {
			bytes[23 + index] = self.pwm_pins[index];
			index += 1;
		}
		bytes[39] = self.clock_pins[0];
		bytes[40] = self.clock_pins[1];
		bytes[41] = self.enabled;
		bytes[42] = self.adc_enabled;
		let pwm_enabled = self.pwm_enabled.to_le_bytes();
		bytes[43] = pwm_enabled[0];
		bytes[44] = pwm_enabled[1];
		bytes[45] = self.clock_enabled;
		bytes[46] = self.clock_sources[0] as u8;
		bytes[47] = self.clock_sources[1] as u8;
		let clock0 = self.clock_hz[0].to_le_bytes();
		bytes[48] = clock0[0];
		bytes[49] = clock0[1];
		bytes[50] = clock0[2];
		bytes[51] = clock0[3];
		let clock1 = self.clock_hz[1].to_le_bytes();
		bytes[52] = clock1[0];
		bytes[53] = clock1[1];
		bytes[54] = clock1[2];
		bytes[55] = clock1[3];
		bytes
	}

	pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
		if bytes.len() != PIN_MAP_LEN {
			return Err(WireError::Truncated);
		}
		Ok(Self {
			uart_tx: [bytes[0], bytes[2]],
			uart_rx: [bytes[1], bytes[3]],
			i2c_sda: [bytes[4], bytes[6]],
			i2c_scl: [bytes[5], bytes[7]],
			spi_miso: [bytes[8], bytes[13]],
			spi_sck: [bytes[9], bytes[14]],
			spi_mosi: [bytes[10], bytes[15]],
			spi_chip_selects: [[bytes[11], bytes[12]], [bytes[16], bytes[17]]],
			adc_pins: [bytes[18], bytes[19], bytes[20], bytes[21], bytes[22]],
			pwm_pins: bytes[23..39].try_into().unwrap(),
			clock_pins: [bytes[39], bytes[40]],
			enabled: bytes[41],
			adc_enabled: bytes[42],
			pwm_enabled: u16::from_le_bytes([bytes[43], bytes[44]]),
			clock_enabled: bytes[45],
			clock_sources: [
				ClockSource::try_from(bytes[46])?,
				ClockSource::try_from(bytes[47])?,
			],
			clock_hz: [read_u32(&bytes[48..52]), read_u32(&bytes[52..56])],
		})
	}

	pub const fn pin(self, signal: PinSignal) -> u8 {
		self.encode()[signal as usize]
	}

	pub fn set_pin(&mut self, signal: PinSignal, pin: u8) {
		match signal {
			PinSignal::Uart0Tx => self.uart_tx[0] = pin,
			PinSignal::Uart0Rx => self.uart_rx[0] = pin,
			PinSignal::Uart1Tx => self.uart_tx[1] = pin,
			PinSignal::Uart1Rx => self.uart_rx[1] = pin,
			PinSignal::I2c0Sda => self.i2c_sda[0] = pin,
			PinSignal::I2c0Scl => self.i2c_scl[0] = pin,
			PinSignal::I2c1Sda => self.i2c_sda[1] = pin,
			PinSignal::I2c1Scl => self.i2c_scl[1] = pin,
			PinSignal::Spi0Miso => self.spi_miso[0] = pin,
			PinSignal::Spi0Sck => self.spi_sck[0] = pin,
			PinSignal::Spi0Mosi => self.spi_mosi[0] = pin,
			PinSignal::Spi0Cs0 => self.spi_chip_selects[0][0] = pin,
			PinSignal::Spi0Cs1 => self.spi_chip_selects[0][1] = pin,
			PinSignal::Spi1Miso => self.spi_miso[1] = pin,
			PinSignal::Spi1Sck => self.spi_sck[1] = pin,
			PinSignal::Spi1Mosi => self.spi_mosi[1] = pin,
			PinSignal::Spi1Cs0 => self.spi_chip_selects[1][0] = pin,
			PinSignal::Spi1Cs1 => self.spi_chip_selects[1][1] = pin,
			PinSignal::Adc0 => self.adc_pins[0] = pin,
			PinSignal::Adc1 => self.adc_pins[1] = pin,
			PinSignal::Adc2 => self.adc_pins[2] = pin,
			PinSignal::Adc3 => self.adc_pins[3] = pin,
			PinSignal::Adc4 => self.adc_pins[4] = pin,
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
			| PinSignal::Pwm7B => self.pwm_pins[signal.pwm_channel().unwrap()] = pin,
			PinSignal::Clock0 | PinSignal::Clock1 => {
				self.clock_pins[signal.clock_output().unwrap()] = pin
			}
		}
	}

	pub const fn is_enabled(self, peripheral: Peripheral) -> bool {
		self.enabled & peripheral.mask() != 0
	}

	pub fn set_enabled(&mut self, peripheral: Peripheral, enabled: bool) {
		if enabled {
			self.enabled |= peripheral.mask();
		} else {
			self.enabled &= !peripheral.mask();
		}
	}

	pub const fn spi_cs1_enabled(self, bus: usize) -> bool {
		self.enabled & [SPI0_CS1_DISABLE_MASK, SPI1_CS1_DISABLE_MASK][bus] == 0
	}

	pub fn set_spi_cs1_enabled(&mut self, bus: usize, enabled: bool) {
		let mask = [SPI0_CS1_DISABLE_MASK, SPI1_CS1_DISABLE_MASK][bus];
		if enabled {
			self.enabled &= !mask;
		} else {
			self.enabled |= mask;
		}
	}

	pub const fn spi_chip_select_count(self, bus: usize) -> u8 {
		1 + self.spi_cs1_enabled(bus) as u8
	}

	pub const fn is_signal_enabled(self, signal: PinSignal) -> bool {
		if let Some(peripheral) = signal.serial_peripheral() {
			return self.is_enabled(peripheral)
				&& match signal {
					PinSignal::Spi0Cs1 => self.spi_cs1_enabled(0),
					PinSignal::Spi1Cs1 => self.spi_cs1_enabled(1),
					_ => true,
				};
		}
		if let Some(channel) = signal.adc_channel() {
			return self.adc_enabled & (1 << channel) != 0;
		}
		if let Some(channel) = signal.pwm_channel() {
			return self.pwm_enabled & (1 << channel) != 0;
		}
		if let Some(output) = signal.clock_output() {
			return self.clock_enabled & (1 << output) != 0;
		}
		false
	}

	pub fn set_signal_enabled(&mut self, signal: PinSignal, enabled: bool) {
		if let Some(peripheral) = signal.serial_peripheral() {
			match signal {
				PinSignal::Spi0Cs1 => self.set_spi_cs1_enabled(0, enabled),
				PinSignal::Spi1Cs1 => self.set_spi_cs1_enabled(1, enabled),
				_ => self.set_enabled(peripheral, enabled),
			}
		} else if let Some(channel) = signal.adc_channel() {
			set_mask_bit_u8(&mut self.adc_enabled, channel, enabled);
		} else if let Some(channel) = signal.pwm_channel() {
			set_mask_bit_u16(&mut self.pwm_enabled, channel, enabled);
		} else if let Some(output) = signal.clock_output() {
			set_mask_bit_u8(&mut self.clock_enabled, output, enabled);
		}
	}

	pub fn validate(self) -> Result<(), PinValidationError> {
		self.validate_for(BoardKind::Pico2.profile())
	}

	pub fn validate_for(self, profile: BoardProfile) -> Result<(), PinValidationError> {
		let adc_mask = (1_u8 << profile.adc_channel_count()) - 1;
		let pwm_mask = if profile.pwm_channel_count() == u16::BITS as usize {
			u16::MAX
		} else {
			(1_u16 << profile.pwm_channel_count()) - 1
		};
		if self.adc_enabled & !adc_mask != 0
			|| self.pwm_enabled & !pwm_mask != 0
			|| self.clock_enabled & !0x03 != 0
		{
			return Err(PinValidationError {
				kind: PinValidationKind::InvalidEnableMask,
				signal: PinSignal::Adc0,
				other: None,
				pin: 0,
			});
		}
		for output in 0..CLOCK_OUTPUT_COUNT {
			if self.clock_hz[output] == 0 {
				return Err(PinValidationError {
					kind: PinValidationKind::InvalidRoute,
					signal: [PinSignal::Clock0, PinSignal::Clock1][output],
					other: None,
					pin: self.clock_pins[output],
				});
			}
		}
		for signal in PinSignal::ALL {
			let pin = self.pin(signal);
			if !profile.supports(signal) {
				if self.is_signal_enabled(signal) {
					return Err(PinValidationError {
						kind: PinValidationKind::UnsupportedSignal,
						signal,
						other: None,
						pin,
					});
				}
				continue;
			}
			if profile.reserved_pins().contains(&pin) {
				return Err(PinValidationError {
					kind: PinValidationKind::ReservedPin,
					signal,
					other: None,
					pin,
				});
			}
			if !profile.gpio_pins().contains(&pin) {
				return Err(PinValidationError {
					kind: PinValidationKind::UnavailablePin,
					signal,
					other: None,
					pin,
				});
			}
			if !profile.valid_pins(signal).contains(&pin) {
				return Err(PinValidationError {
					kind: PinValidationKind::InvalidRoute,
					signal,
					other: None,
					pin,
				});
			}
		}
		for (sda, scl) in [
			(PinSignal::I2c0Sda, PinSignal::I2c0Scl),
			(PinSignal::I2c1Sda, PinSignal::I2c1Scl),
		] {
			if !profile.requires_adjacent_i2c_pins() || !profile.supports(sda) {
				continue;
			}
			if self.pin(scl) != self.pin(sda) + 1 {
				return Err(PinValidationError {
					kind: PinValidationKind::MismatchedPair,
					signal: scl,
					other: Some(sda),
					pin: self.pin(scl),
				});
			}
		}
		for (index, signal) in PinSignal::ALL.iter().copied().enumerate() {
			if !self.is_signal_enabled(signal) {
				continue;
			}
			let pin = self.pin(signal);
			if let Some(other) = PinSignal::ALL[..index]
				.iter()
				.copied()
				.find(|&other| self.is_signal_enabled(other) && self.pin(other) == pin)
			{
				return Err(PinValidationError {
					kind: PinValidationKind::DuplicatePin,
					signal,
					other: Some(other),
					pin,
				});
			}
		}
		Ok(())
	}

	pub fn gpio_pins(self) -> Result<GpioPins, PinValidationError> {
		self.gpio_pins_for(BoardKind::Pico2.profile())
	}

	pub fn gpio_pins_for(self, profile: BoardProfile) -> Result<GpioPins, PinValidationError> {
		self.validate_for(profile)?;
		let mut assigned = [u8::MAX; PIN_SIGNAL_COUNT];
		let mut assigned_len = 0;
		for signal in PinSignal::ALL {
			if self.is_signal_enabled(signal) {
				assigned[assigned_len] = self.pin(signal);
				assigned_len += 1;
			}
		}
		let mut gpio = [0; MAX_GPIO_LINES];
		let mut output = 0;
		for &pin in profile.gpio_pins() {
			if !assigned[..assigned_len].contains(&pin) {
				gpio[output] = pin;
				output += 1;
			}
		}
		Ok(GpioPins {
			pins: gpio,
			len: output as u8,
		})
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpioPins {
	pins: [u8; MAX_GPIO_LINES],
	len: u8,
}

impl GpioPins {
	pub const fn len(self) -> usize {
		self.len as usize
	}

	pub const fn is_empty(self) -> bool {
		self.len == 0
	}

	pub fn as_slice(&self) -> &[u8] {
		&self.pins[..self.len()]
	}

	pub const fn into_array(self) -> [u8; MAX_GPIO_LINES] {
		self.pins
	}
}

fn set_mask_bit_u8(mask: &mut u8, bit: usize, enabled: bool) {
	if enabled {
		*mask |= 1 << bit;
	} else {
		*mask &= !(1 << bit);
	}
}

fn set_mask_bit_u16(mask: &mut u16, bit: usize, enabled: bool) {
	if enabled {
		*mask |= 1 << bit;
	} else {
		*mask &= !(1 << bit);
	}
}

fn read_u16(bytes: &[u8]) -> u16 {
	u16::from_le_bytes([bytes[0], bytes[1]])
}
fn read_u32(bytes: &[u8]) -> u32 {
	u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}
fn read_u64(bytes: &[u8]) -> u64 {
	u64::from_le_bytes(bytes[..8].try_into().unwrap())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsbHeader {
	pub operation: Operation,
	pub reply: bool,
	pub request_id: u64,
	pub payload_len: u32,
	pub status: Status,
}

impl UsbHeader {
	pub fn encode(self) -> [u8; USB_HEADER_LEN] {
		let mut out = [0; USB_HEADER_LEN];
		out[..4].copy_from_slice(&USB_MAGIC);
		out[4] = VERSION_MAJOR;
		out[5] = VERSION_MINOR;
		out[6] = self.operation as u8;
		out[7] = u8::from(self.reply);
		out[8..16].copy_from_slice(&self.request_id.to_le_bytes());
		out[16..20].copy_from_slice(&self.payload_len.to_le_bytes());
		out[20..24].copy_from_slice(&(self.status as i32).to_le_bytes());
		out
	}

	pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
		if bytes.len() != USB_HEADER_LEN {
			return Err(WireError::Truncated);
		}
		if bytes[..4] != USB_MAGIC || bytes[4] != VERSION_MAJOR || bytes[7] & !USB_REPLY != 0 {
			return Err(WireError::Invalid);
		}
		let payload_len = read_u32(&bytes[16..20]);
		if payload_len as usize > MAX_PAYLOAD {
			return Err(WireError::TooLarge);
		}
		let status = Status::try_from(i32::from_le_bytes(bytes[20..24].try_into().unwrap()))?;
		let reply = bytes[7] & USB_REPLY != 0;
		if !reply && status != Status::Ok {
			return Err(WireError::Invalid);
		}
		Ok(Self {
			operation: Operation::try_from(bytes[6])?,
			reply,
			request_id: read_u64(&bytes[8..16]),
			payload_len,
			status,
		})
	}
}

/// Writes a complete payload into caller-owned storage without allocation.
pub struct Writer<'a> {
	bytes: &'a mut [u8],
	len: usize,
}

impl<'a> Writer<'a> {
	pub fn new(bytes: &'a mut [u8]) -> Self {
		Self { bytes, len: 0 }
	}
	pub fn len(&self) -> usize {
		self.len
	}
	pub fn is_empty(&self) -> bool {
		self.len == 0
	}
	pub fn written(&self) -> &[u8] {
		&self.bytes[..self.len]
	}
	pub fn push(&mut self, value: u8) -> Result<(), WireError> {
		self.extend(&[value])
	}
	pub fn u16(&mut self, value: u16) -> Result<(), WireError> {
		self.extend(&value.to_le_bytes())
	}
	pub fn u32(&mut self, value: u32) -> Result<(), WireError> {
		self.extend(&value.to_le_bytes())
	}
	pub fn i32(&mut self, value: i32) -> Result<(), WireError> {
		self.extend(&value.to_le_bytes())
	}
	pub fn extend(&mut self, value: &[u8]) -> Result<(), WireError> {
		if value.len() > self.bytes.len().saturating_sub(self.len) {
			return Err(WireError::TooLarge);
		}
		self.bytes[self.len..self.len + value.len()].copy_from_slice(value);
		self.len += value.len();
		Ok(())
	}
}

pub const I2C_READ: u16 = 1;
pub const I2C_TEN_BIT: u16 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct I2cMessage<'a> {
	pub address: u16,
	pub flags: u16,
	pub len: u16,
	pub write_data: &'a [u8],
}

pub struct I2cRequest<'a> {
	pub bus_id: u8,
	pub count: u8,
	payload: &'a [u8],
}

impl<'a> I2cRequest<'a> {
	pub fn parse(payload: &'a [u8]) -> Result<Self, WireError> {
		if payload.len() < 4 {
			return Err(WireError::Truncated);
		}
		let count = payload[1] as usize;
		if count == 0 || count > MAX_I2C_MESSAGES || read_u16(&payload[2..4]) != 0 {
			return Err(WireError::Invalid);
		}
		let request = Self {
			bus_id: payload[0],
			count: count as u8,
			payload,
		};
		let mut iter = request.messages();
		while iter.next().transpose()?.is_some() {}
		if iter.remaining != 0
			|| iter.offset != payload.len()
			|| iter.read_len > MAX_DATA
			|| iter.write_len > MAX_DATA
		{
			return Err(WireError::Invalid);
		}
		Ok(request)
	}

	pub fn messages(&self) -> I2cMessages<'a> {
		I2cMessages {
			payload: self.payload,
			remaining: self.count,
			offset: 4,
			read_len: 0,
			write_len: 0,
		}
	}
}

pub struct I2cMessages<'a> {
	payload: &'a [u8],
	remaining: u8,
	offset: usize,
	read_len: usize,
	write_len: usize,
}

impl<'a> Iterator for I2cMessages<'a> {
	type Item = Result<I2cMessage<'a>, WireError>;
	fn next(&mut self) -> Option<Self::Item> {
		if self.remaining == 0 {
			return None;
		}
		self.remaining -= 1;
		let tail = self.payload.get(self.offset..)?;
		if tail.len() < 8 {
			return Some(Err(WireError::Truncated));
		}
		let address = read_u16(tail);
		let flags = read_u16(&tail[2..4]);
		let len = read_u16(&tail[4..6]);
		if read_u16(&tail[6..8]) != 0
			|| flags & !(I2C_READ | I2C_TEN_BIT) != 0
			|| address
				> if flags & I2C_TEN_BIT != 0 {
					0x3ff
				} else {
					0x7f
				} {
			return Some(Err(WireError::Invalid));
		}
		self.offset += 8;
		let data = if flags & I2C_READ != 0 {
			&[][..]
		} else {
			let Some(data) = self.payload.get(self.offset..self.offset + len as usize) else {
				return Some(Err(WireError::Truncated));
			};
			self.offset += len as usize;
			data
		};
		if flags & I2C_READ != 0 {
			self.read_len += len as usize;
		} else {
			self.write_len += len as usize;
		}
		Some(Ok(I2cMessage {
			address,
			flags,
			len,
			write_data: data,
		}))
	}
}

pub const SPI_CS_CHANGE: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpiSegment<'a> {
	pub speed_hz: u32,
	pub len: u16,
	pub flags: u16,
	pub word_delay_ns: u32,
	pub delay_ns: u32,
	pub cs_change_delay_ns: u32,
	pub tx: &'a [u8],
}

pub struct SpiRequest<'a> {
	pub bus_id: u8,
	pub chip_select: u8,
	pub mode: u16,
	pub count: u8,
	payload: &'a [u8],
}

impl<'a> SpiRequest<'a> {
	pub fn parse(payload: &'a [u8]) -> Result<Self, WireError> {
		if payload.len() < 8 {
			return Err(WireError::Truncated);
		}
		let count = payload[4] as usize;
		if count == 0 || count > MAX_SPI_SEGMENTS || payload[5..8] != [0; 3] {
			return Err(WireError::Invalid);
		}
		let request = Self {
			bus_id: payload[0],
			chip_select: payload[1],
			mode: read_u16(&payload[2..4]),
			count: count as u8,
			payload,
		};
		let mut iter = request.segments();
		while iter.next().transpose()?.is_some() {}
		if iter.remaining != 0 || iter.offset != payload.len() || iter.data_len > MAX_DATA {
			return Err(WireError::Invalid);
		}
		Ok(request)
	}

	pub fn segments(&self) -> SpiSegments<'a> {
		SpiSegments {
			payload: self.payload,
			remaining: self.count,
			offset: 8,
			data_len: 0,
		}
	}
}

pub struct SpiSegments<'a> {
	payload: &'a [u8],
	remaining: u8,
	offset: usize,
	data_len: usize,
}

impl<'a> Iterator for SpiSegments<'a> {
	type Item = Result<SpiSegment<'a>, WireError>;
	fn next(&mut self) -> Option<Self::Item> {
		if self.remaining == 0 {
			return None;
		}
		self.remaining -= 1;
		let tail = self.payload.get(self.offset..)?;
		if tail.len() < 20 {
			return Some(Err(WireError::Truncated));
		}
		let speed_hz = read_u32(tail);
		let len = read_u16(&tail[4..6]);
		let flags = read_u16(&tail[6..8]);
		if flags & !SPI_CS_CHANGE != 0 || speed_hz == 0 {
			return Some(Err(WireError::Invalid));
		}
		let word_delay_ns = read_u32(&tail[8..12]);
		let delay_ns = read_u32(&tail[12..16]);
		let cs_change_delay_ns = read_u32(&tail[16..20]);
		self.offset += 20;
		let Some(tx) = self.payload.get(self.offset..self.offset + len as usize) else {
			return Some(Err(WireError::Truncated));
		};
		self.offset += len as usize;
		self.data_len += len as usize;
		Some(Ok(SpiSegment {
			speed_hz,
			len,
			flags,
			word_delay_ns,
			delay_ns,
			cs_change_delay_ns,
			tx,
		}))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn header_round_trip_and_rejects_invalid_status() {
		let header = UsbHeader {
			operation: Operation::I2cTransfer,
			reply: false,
			request_id: 72,
			payload_len: 12,
			status: Status::Ok,
		};
		assert_eq!(UsbHeader::decode(&header.encode()), Ok(header));
		let mut malformed = header.encode();
		malformed[20] = Status::Nack as u8;
		assert_eq!(UsbHeader::decode(&malformed), Err(WireError::Invalid));
	}

	#[test]
	fn serial_frames_round_trip_binary_bodies_and_detect_corruption() {
		let mut body = [0; MAX_SERIAL_BODY];
		for (index, byte) in body.iter_mut().enumerate() {
			*byte = (index % 251) as u8;
		}
		let mut encoded = [0; MAX_SERIAL_FRAME];
		let encoded_len = encode_serial_frame(&body, &mut encoded).unwrap();
		assert_eq!(encoded[encoded_len - 1], 0);
		assert!(!encoded[..encoded_len - 1].contains(&0));

		let mut decoded = [0; MAX_SERIAL_BODY];
		let decoded_len = decode_serial_frame(&encoded[..encoded_len], &mut decoded).unwrap();
		assert_eq!(decoded_len, body.len());
		assert_eq!(decoded, body);

		encoded[encoded_len / 2] ^= 0x40;
		assert_eq!(
			decode_serial_frame(&encoded[..encoded_len], &mut decoded),
			Err(WireError::Invalid)
		);
	}

	#[test]
	fn serial_frames_reject_noise_and_small_buffers() {
		let mut output = [0; MAX_SERIAL_FRAME];
		let mut body = [0; 4];
		assert!(decode_serial_frame(b"ESP-ROM:esp32c3-api1\0", &mut body).is_err());
		let encoded_len = encode_serial_frame(b"hello", &mut output).unwrap();
		assert_eq!(
			decode_serial_frame(&output[..encoded_len], &mut body),
			Err(WireError::TooLarge)
		);
		assert_eq!(
			encode_serial_frame(b"hello", &mut [0; 4]),
			Err(WireError::TooLarge)
		);
	}

	#[test]
	fn pin_maps_validate_routes_conflicts_and_gpio_derivation() {
		assert_eq!(DEFAULT_PIN_MAP.validate(), Ok(()));
		assert_eq!(
			DEFAULT_PIN_MAP.gpio_pins().unwrap().as_slice(),
			&[2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 20, 22, 26, 27, 28]
		);

		let mut alternate = DEFAULT_PIN_MAP;
		alternate.i2c_sda[0] = 12;
		alternate.i2c_scl[0] = 13;
		alternate.spi_chip_selects[0] = [4, 5];
		assert_eq!(PinMap::decode(&alternate.encode()), Ok(alternate));
		assert_eq!(alternate.validate(), Ok(()));

		let mut invalid = alternate;
		invalid.spi_sck[0] = 3;
		assert_eq!(
			invalid.validate().unwrap_err(),
			PinValidationError {
				kind: PinValidationKind::InvalidRoute,
				signal: PinSignal::Spi0Sck,
				other: None,
				pin: 3,
			}
		);

		let mut duplicate = alternate;
		duplicate.spi_chip_selects[0][0] = duplicate.uart_tx[0];
		assert_eq!(
			duplicate.validate().unwrap_err(),
			PinValidationError {
				kind: PinValidationKind::DuplicatePin,
				signal: PinSignal::Spi0Cs0,
				other: Some(PinSignal::Uart0Tx),
				pin: duplicate.uart_tx[0],
			}
		);

		let mut all_enabled = DEFAULT_PIN_MAP;
		all_enabled.enabled = 0x3f;
		assert_eq!(all_enabled.validate(), Ok(()));
		assert_eq!(all_enabled.gpio_pins().unwrap().len(), 8);

		let mut one_chip_select = DEFAULT_PIN_MAP;
		one_chip_select.set_spi_cs1_enabled(0, false);
		assert_eq!(
			PinMap::decode(&one_chip_select.encode()),
			Ok(one_chip_select)
		);
		assert_eq!(one_chip_select.spi_chip_select_count(0), 1);
		assert!(!one_chip_select.is_signal_enabled(PinSignal::Spi0Cs1));
		assert!(
			one_chip_select
				.gpio_pins()
				.unwrap()
				.as_slice()
				.contains(&one_chip_select.spi_chip_selects[0][1])
		);

		let mut none_enabled = DEFAULT_PIN_MAP;
		none_enabled.enabled = 0;
		assert_eq!(none_enabled.validate(), Ok(()));
		assert_eq!(none_enabled.gpio_pins().unwrap().as_slice(), &HEADER_PINS);

		let mut auxiliary = none_enabled;
		auxiliary.adc_enabled = 1;
		auxiliary.pwm_enabled = 1 << 2;
		auxiliary.clock_enabled = 1;
		assert_eq!(PinMap::decode(&auxiliary.encode()), Ok(auxiliary));
		assert_eq!(auxiliary.validate(), Ok(()));
		assert!(!auxiliary.gpio_pins().unwrap().as_slice().contains(&26));
		assert!(!auxiliary.gpio_pins().unwrap().as_slice().contains(&2));
		assert!(!auxiliary.gpio_pins().unwrap().as_slice().contains(&13));

		auxiliary.pwm_pins[2] = 26;
		assert_eq!(
			auxiliary.validate().unwrap_err().kind,
			PinValidationKind::InvalidRoute
		);
	}

	#[test]
	fn esp32_c3_profile_accepts_matrix_routes_and_reserves_transport() {
		let profile = BoardKind::Esp32C3DevKitM1.profile();
		let map = profile.default_pin_map();
		assert_eq!(profile.transport(), TransportKind::Serial);
		assert_eq!(map.validate_for(profile), Ok(()));
		assert_eq!(
			map.gpio_pins_for(profile).unwrap().as_slice(),
			&[0, 2, 8, 9, 18, 19]
		);

		let mut matrix_route = map;
		matrix_route.i2c_sda[0] = 18;
		matrix_route.i2c_scl[0] = 0;
		assert_eq!(matrix_route.validate_for(profile), Ok(()));

		let mut transport_conflict = map;
		transport_conflict.i2c_sda[0] = 20;
		assert_eq!(
			transport_conflict.validate_for(profile).unwrap_err().kind,
			PinValidationKind::ReservedPin
		);

		let mut unsupported = map;
		unsupported.set_enabled(Peripheral::I2c1, true);
		assert_eq!(
			unsupported.validate_for(profile).unwrap_err().kind,
			PinValidationKind::UnsupportedSignal
		);
		assert_eq!(profile.pin_warning(2), Some(PinWarning::BootStrap));
		assert_eq!(profile.pin_warning(8), Some(PinWarning::OnboardRgbLed));
	}

	#[test]
	fn pwm_states_round_trip_and_reject_invalid_duty() {
		let state = PwmState {
			channel: 3,
			enabled: true,
			inverted: false,
			period_ns: 20_000_000,
			duty_ns: 1_500_000,
		};
		assert_eq!(
			PwmState::decode(&state.encode(PWM_APPLY)),
			Ok((PWM_APPLY, state))
		);

		let mut invalid = state.encode(PWM_APPLY);
		invalid[12..20].copy_from_slice(&21_000_000_u64.to_le_bytes());
		assert_eq!(PwmState::decode(&invalid), Err(WireError::Invalid));
	}

	#[test]
	fn onboard_led_profiles_and_states_are_explicit() {
		assert_eq!(
			BoardKind::Pico2.profile().onboard_led(),
			OnboardLed {
				kind: OnboardLedKind::Monochrome,
				pin: 25,
			}
		);
		assert_eq!(
			BoardKind::Esp32C3DevKitM1.profile().onboard_led(),
			OnboardLed {
				kind: OnboardLedKind::Rgb,
				pin: 8,
			}
		);
		let state = LedState {
			index: 0,
			red: 12,
			green: 34,
			blue: 56,
		};
		assert_eq!(
			LedState::decode(&state.encode(LED_SET)),
			Ok((LED_SET, state))
		);
		let mut invalid = state.encode(LED_SET);
		invalid[7] = 1;
		assert_eq!(LedState::decode(&invalid), Err(WireError::Invalid));
	}

	#[test]
	fn clock_dividers_validate_and_report_quantized_frequency() {
		assert_eq!(clock_divider(150_000_000, 0), None);
		assert_eq!(clock_divider(48_000_000, 50_000_000), None);
		assert_eq!(
			clock_divider(150_000_000, 1_000_000),
			Some(ClockDivider {
				integer: 150,
				fraction: 0,
				effective_hz: 1_000_000,
			})
		);
		let divided = clock_divider(12_000_000, 32_768).unwrap();
		assert!(divided.effective_hz.abs_diff(32_768) <= 1);
		assert_eq!(clock_divider(150_000_000, 1), None);
	}

	#[test]
	fn pin_validation_errors_round_trip() {
		let error = PinValidationError {
			kind: PinValidationKind::MismatchedPair,
			signal: PinSignal::I2c0Scl,
			other: Some(PinSignal::I2c0Sda),
			pin: 9,
		};
		assert_eq!(PinValidationError::decode(&error.encode()), Ok(error));
	}

	#[test]
	fn i2c_rejects_short_write_and_trailing_data() {
		let mut bytes = [0; 13];
		bytes[1] = 1;
		bytes[4] = 0x50;
		bytes[8] = 2;
		bytes[12] = 0xaa;
		assert!(matches!(
			I2cRequest::parse(&bytes),
			Err(WireError::Truncated)
		));
		bytes[8] = 1;
		assert_eq!(
			I2cRequest::parse(&bytes)
				.unwrap()
				.messages()
				.next()
				.unwrap()
				.unwrap()
				.write_data,
			&[0xaa]
		);
	}

	#[test]
	fn spi_segment_bounds() {
		let mut bytes = [0; 29];
		bytes[4] = 1;
		bytes[8..12].copy_from_slice(&1_000_000u32.to_le_bytes());
		bytes[12] = 1;
		bytes[28] = 0x5a;
		assert_eq!(
			SpiRequest::parse(&bytes)
				.unwrap()
				.segments()
				.next()
				.unwrap()
				.unwrap()
				.tx,
			&[0x5a]
		);
		bytes[12] = 2;
		assert!(SpiRequest::parse(&bytes).is_err());
	}
}
