use embedded_hal::pwm::SetDutyCycle;
use esp_hal::{
	analog::adc::{Adc, AdcConfig, AdcPin, Attenuation},
	delay::Delay,
	gpio::{AnyPin, DriveMode, Flex, InputConfig, Level, NoPin, OutputConfig, Pull},
	i2c::master::{Config as I2cConfig, Error as I2cError, I2c},
	ledc::{
		Ledc, LowSpeed,
		channel::{self, ChannelIFace},
		timer::{self, TimerIFace},
	},
	peripherals::{ADC1, GPIO0, GPIO1, GPIO2, GPIO3, GPIO4, I2C0, LEDC, RMT, SPI2, UART1},
	rmt::{Channel, PulseCode, Rmt, Tx, TxChannelConfig, TxChannelCreator},
	spi::{
		BitOrder, Mode,
		master::{Config as SpiConfig, Spi},
	},
	time::Rate,
	uart::{Config as UartConfig, DataBits, Parity, StopBits, Uart},
};
use rp_extio_firmware::{
	Board, BusFailure, SPI_CPHA, SPI_CPOL, SPI_CS_HIGH, SPI_LSB_FIRST, SPI_MAX_SPEED_HZ, SPI_NO_CS,
	SpiOutcome, cumulative_spi_delay_ns,
};
use rp_extio_wire::{
	BoardKind, DeviceIdentity, ESP32_C3_GPIO_PINS, GPIO_BIAS_NONE, GPIO_BIAS_PULL_DOWN,
	GPIO_BIAS_PULL_UP, GPIO_DIRECTION_INPUT, GPIO_DIRECTION_OUTPUT, GPIO_GET, GPIO_GET_DIRECTION,
	GPIO_SET, GPIO_SET_BIAS, I2C_READ, I2C_TEN_BIT, I2cRequest, LedState, PinMap,
	PinValidationError, PwmState, SPI_CS_CHANGE, SpiRequest, Status, UART_PARITY_EVEN,
	UART_PARITY_NONE, UART_PARITY_ODD,
};

const SPI_CLOCK_HZ: u32 = 80_000_000;
const ESP32_C3_SPI_MIN_SPEED_HZ: u32 = SPI_CLOCK_HZ / (16 * 64);

pub struct Esp32C3Board<'d> {
	pins: [Flex<'d>; ESP32_C3_GPIO_PINS.len()],
	route_pins: [AnyPin<'d>; ESP32_C3_GPIO_PINS.len()],
	i2c: Option<I2c<'d, esp_hal::Blocking>>,
	spi: Option<Spi<'d, esp_hal::Blocking>>,
	uart: Option<Uart<'d, esp_hal::Blocking>>,
	adc: Adc<'d, ADC1<'d>, esp_hal::Blocking>,
	adc0: AdcPin<GPIO0<'d>, ADC1<'d>>,
	adc1: AdcPin<GPIO1<'d>, ADC1<'d>>,
	adc2: AdcPin<GPIO2<'d>, ADC1<'d>>,
	adc3: AdcPin<GPIO3<'d>, ADC1<'d>>,
	adc4: AdcPin<GPIO4<'d>, ADC1<'d>>,
	ledc: Ledc<'d>,
	pwm_states: [PwmState; rp_extio_wire::ESP32_C3_PWM_CHANNEL_COUNT],
	led_channel: Option<Channel<'d, esp_hal::Blocking, Tx>>,
	led_state: LedState,
	pin_map: PinMap,
	generation: u64,
	identity: DeviceIdentity,
	gpio_outputs: u32,
	gpio_pull_ups: u32,
	gpio_pull_downs: u32,
}

impl<'d> Esp32C3Board<'d> {
	pub fn new(
		route_pins: [AnyPin<'d>; ESP32_C3_GPIO_PINS.len()],
		i2c: I2C0<'d>,
		spi: SPI2<'d>,
		uart: UART1<'d>,
		adc: ADC1<'d>,
		adc_pins: (GPIO0<'d>, GPIO1<'d>, GPIO2<'d>, GPIO3<'d>, GPIO4<'d>),
		ledc: LEDC<'d>,
		rmt: RMT<'d>,
		identity: DeviceIdentity,
	) -> Self {
		let mut pins = core::array::from_fn(|index| {
			// The route clone is only used by peripheral drivers. Access is serialized
			// with the GPIO-side Flex driver by the firmware request loop.
			Flex::new(unsafe { route_pins[index].clone_unchecked() })
		});
		for pin in &mut pins {
			pin.set_output_enable(false);
			pin.set_input_enable(true);
			pin.apply_input_config(&InputConfig::default());
		}
		let mut adc_config = AdcConfig::new();
		let adc0 = adc_config.enable_pin(adc_pins.0, Attenuation::_11dB);
		let adc1 = adc_config.enable_pin(adc_pins.1, Attenuation::_11dB);
		let adc2 = adc_config.enable_pin(adc_pins.2, Attenuation::_11dB);
		let adc3 = adc_config.enable_pin(adc_pins.3, Attenuation::_11dB);
		let adc4 = adc_config.enable_pin(adc_pins.4, Attenuation::_11dB);
		let rmt = Rmt::new(rmt, Rate::from_mhz(80)).unwrap();
		let led_channel = rmt
			.channel0
			.configure_tx(
				&TxChannelConfig::default()
					.with_clk_divider(1)
					.with_idle_output(true)
					.with_idle_output_level(Level::Low),
			)
			.unwrap();
		let mut board = Self {
			pins,
			route_pins,
			i2c: Some(
				I2c::new(
					i2c,
					I2cConfig::default().with_frequency(Rate::from_khz(100)),
				)
				.unwrap(),
			),
			spi: Some(
				Spi::new(
					spi,
					SpiConfig::default().with_frequency(Rate::from_hz(ESP32_C3_SPI_MIN_SPEED_HZ)),
				)
				.unwrap(),
			),
			uart: Some(Uart::new(uart, UartConfig::default()).unwrap()),
			adc: Adc::new(adc, adc_config),
			adc0,
			adc1,
			adc2,
			adc3,
			adc4,
			ledc: Ledc::new(ledc),
			pwm_states: core::array::from_fn(|channel| PwmState {
				channel: channel as u8,
				enabled: false,
				inverted: false,
				period_ns: 0,
				duty_ns: 0,
			}),
			led_channel: Some(led_channel),
			led_state: LedState::OFF,
			pin_map: rp_extio_wire::ESP32_C3_DEFAULT_PIN_MAP,
			generation: 0,
			identity,
			gpio_outputs: 0,
			gpio_pull_ups: 0,
			gpio_pull_downs: 0,
		};
		board.apply_peripheral_routes();
		board
	}

	fn route_flex(&self, number: u8) -> Flex<'d> {
		let index = ESP32_C3_GPIO_PINS
			.iter()
			.position(|&pin| pin == number)
			.unwrap();
		Flex::new(unsafe { self.route_pins[index].clone_unchecked() })
	}

	fn apply_peripheral_routes(&mut self) {
		self.i2c = Some(self.i2c.take().unwrap().with_sda(NoPin).with_scl(NoPin));
		self.spi = Some(
			self.spi
				.take()
				.unwrap()
				.with_sck(NoPin)
				.with_mosi(NoPin)
				.with_miso(NoPin),
		);
		self.uart = Some(self.uart.take().unwrap().with_tx(NoPin).with_rx(NoPin));
		if self.pin_map.is_enabled(rp_extio_wire::Peripheral::I2c0) {
			let mut sda = self.route_flex(self.pin_map.i2c_sda[0]);
			let mut scl = self.route_flex(self.pin_map.i2c_scl[0]);
			for pin in [&mut sda, &mut scl] {
				pin.apply_output_config(
					&OutputConfig::default().with_drive_mode(DriveMode::OpenDrain),
				);
				pin.set_high();
				pin.set_input_enable(true);
				pin.set_output_enable(true);
			}
			let i2c = self.i2c.take().unwrap().with_sda(sda).with_scl(scl);
			self.i2c = Some(i2c);
		}
		if self.pin_map.is_enabled(rp_extio_wire::Peripheral::Spi0) {
			let spi = self
				.spi
				.take()
				.unwrap()
				.with_sck(self.route_flex(self.pin_map.spi_sck[0]))
				.with_mosi(self.route_flex(self.pin_map.spi_mosi[0]))
				.with_miso(self.route_flex(self.pin_map.spi_miso[0]));
			self.spi = Some(spi);
			for index in 0..self.pin_map.spi_chip_select_count(0) as usize {
				let pin = self.pin_mut(self.pin_map.spi_chip_selects[0][index]);
				pin.set_high();
				pin.set_output_enable(true);
			}
		}
		if self.pin_map.is_enabled(rp_extio_wire::Peripheral::Uart1) {
			let uart = self
				.uart
				.take()
				.unwrap()
				.with_tx(self.route_flex(self.pin_map.uart_tx[1]))
				.with_rx(self.route_flex(self.pin_map.uart_rx[1]));
			self.uart = Some(uart);
		}
	}

	fn i2c_failure(error: I2cError, completed: u32) -> BusFailure {
		let status = match error {
			I2cError::AcknowledgeCheckFailed(_) => Status::Nack,
			I2cError::ArbitrationLost => Status::ArbitrationLost,
			I2cError::Timeout => Status::Timeout,
			_ => Status::IoError,
		};
		BusFailure::new(status, completed)
	}

	fn onboard_led_busy(&self) -> bool {
		rp_extio_wire::PinSignal::ALL
			.iter()
			.copied()
			.any(|signal| self.pin_map.is_signal_enabled(signal) && self.pin_map.pin(signal) == 8)
	}

	fn transmit_led(&mut self, state: LedState) -> Result<(), BusFailure> {
		let mut pulses = [PulseCode::end_marker(); 25];
		let bytes = [state.green, state.red, state.blue];
		for (index, bit) in bytes
			.iter()
			.flat_map(|byte| (0..8).rev().map(move |shift| byte & (1 << shift) != 0))
			.enumerate()
		{
			pulses[index] = if bit {
				PulseCode::new(Level::High, 64, Level::Low, 36)
			} else {
				PulseCode::new(Level::High, 32, Level::Low, 68)
			};
		}
		pulses[24] = PulseCode::new(Level::Low, 5_000, Level::Low, 0);
		let channel = self
			.led_channel
			.take()
			.unwrap()
			.with_pin(self.route_flex(8));
		let transaction = channel.transmit(&pulses).map_err(|(_, channel)| {
			self.led_channel = Some(channel.with_pin(NoPin));
			BusFailure::new(Status::IoError, 0)
		})?;
		self.led_channel = Some(
			transaction
				.wait()
				.map_err(|(_, channel)| {
					self.led_channel = Some(channel.with_pin(NoPin));
					BusFailure::new(Status::IoError, 0)
				})?
				.with_pin(NoPin),
		);
		Ok(())
	}

	fn pin(&self, number: u8) -> &Flex<'d> {
		&self.pins[ESP32_C3_GPIO_PINS
			.iter()
			.position(|&pin| pin == number)
			.unwrap()]
	}

	fn pin_mut(&mut self, number: u8) -> &mut Flex<'d> {
		&mut self.pins[ESP32_C3_GPIO_PINS
			.iter()
			.position(|&pin| pin == number)
			.unwrap()]
	}

	fn gpio_pin(&self, line: u8) -> Result<u8, BusFailure> {
		self.pin_map
			.gpio_pins_for(BoardKind::Esp32C3DevKitM1.profile())
			.map_err(|_| BusFailure::new(Status::IoError, 0))?
			.as_slice()
			.get(line as usize)
			.copied()
			.ok_or_else(|| BusFailure::new(Status::Unsupported, 0))
	}

	fn spi_effective_speed_hz(requested: u32) -> u32 {
		let minimum_divider = SPI_CLOCK_HZ.div_ceil(requested).max(2);
		if minimum_divider <= 64 {
			return SPI_CLOCK_HZ / minimum_divider;
		}
		let mut best_divider = 16 * 64;
		let mut pre = minimum_divider.div_ceil(64);
		while pre <= 16 {
			let divider = pre * minimum_divider.div_ceil(pre);
			if divider < best_divider {
				best_divider = divider;
				if divider == minimum_divider {
					break;
				}
			}
			pre += 1;
		}
		SPI_CLOCK_HZ / best_divider
	}
}

impl Board for Esp32C3Board<'_> {
	fn board_kind(&self) -> BoardKind {
		BoardKind::Esp32C3DevKitM1
	}

	fn identity(&self) -> DeviceIdentity {
		self.identity
	}

	fn pin_map(&self) -> &PinMap {
		&self.pin_map
	}

	fn generation(&self) -> u64 {
		self.generation
	}

	fn i2c_flags(&self) -> u16 {
		I2C_READ
	}

	fn spi_min_speed_hz(&self) -> u32 {
		ESP32_C3_SPI_MIN_SPEED_HZ
	}

	fn spi_max_speed_hz(&self) -> u32 {
		SPI_MAX_SPEED_HZ
	}

	fn adc_reference_mv(&self) -> u16 {
		2_500
	}

	fn pwm_clock_hz(&self) -> u32 {
		80_000_000
	}

	fn uart_daemon_backed(&self, bus: u8) -> bool {
		bus == 1
	}

	fn uart_max_baud(&self, bus: u8) -> u32 {
		if bus == 1 { 5_000_000 } else { 0 }
	}

	fn uart_transfer(
		&mut self,
		bus: u8,
		baud: u32,
		data_bits: u8,
		stop_bits: u8,
		parity: u8,
		tx: &[u8],
		rx: &mut [u8],
	) -> Result<(usize, usize), BusFailure> {
		if bus != 1 {
			return Err(BusFailure::new(Status::Unsupported, 0));
		}
		let data_bits = match data_bits {
			5 => DataBits::_5,
			6 => DataBits::_6,
			7 => DataBits::_7,
			8 => DataBits::_8,
			_ => return Err(BusFailure::new(Status::BadFrame, 0)),
		};
		let stop_bits = match stop_bits {
			1 => StopBits::_1,
			2 => StopBits::_2,
			_ => return Err(BusFailure::new(Status::BadFrame, 0)),
		};
		let parity = match parity {
			UART_PARITY_NONE => Parity::None,
			UART_PARITY_EVEN => Parity::Even,
			UART_PARITY_ODD => Parity::Odd,
			_ => return Err(BusFailure::new(Status::BadFrame, 0)),
		};
		let uart = self.uart.as_mut().unwrap();
		uart.apply_config(
			&UartConfig::default()
				.with_baudrate(baud)
				.with_data_bits(data_bits)
				.with_stop_bits(stop_bits)
				.with_parity(parity),
		)
		.map_err(|_| BusFailure::new(Status::Unsupported, 0))?;
		let mut written = 0;
		while written < tx.len() {
			let count = uart
				.write(&tx[written..])
				.map_err(|_| BusFailure::new(Status::IoError, written as u32))?;
			if count == 0 {
				break;
			}
			written += count;
		}
		let read = uart
			.read_buffered(rx)
			.map_err(|_| BusFailure::new(Status::IoError, written as u32))?;
		Ok((written, read))
	}

	fn configure_pins(&mut self, map: PinMap, generation: u64) -> Result<(), PinValidationError> {
		map.validate_for(BoardKind::Esp32C3DevKitM1.profile())?;
		for &number in &ESP32_C3_GPIO_PINS {
			let mut pin = self.route_flex(number);
			pin.set_low();
			pin.set_output_enable(false);
			pin.set_input_enable(true);
			pin.apply_input_config(&InputConfig::default());
		}
		for channel in 0..rp_extio_wire::ESP32_C3_ADC_CHANNEL_COUNT {
			if map.adc_enabled & (1 << channel) != 0 {
				self.pin_mut(map.adc_pins[channel]).set_input_enable(false);
			}
		}
		self.pin_map = map;
		self.generation = generation;
		self.gpio_outputs = 0;
		self.gpio_pull_ups = 0;
		self.gpio_pull_downs = 0;
		self.pwm_states = core::array::from_fn(|channel| PwmState {
			channel: channel as u8,
			enabled: false,
			inverted: false,
			period_ns: 0,
			duty_ns: 0,
		});
		self.apply_peripheral_routes();
		Ok(())
	}

	fn i2c_transfer(
		&mut self,
		request: &I2cRequest<'_>,
		reads: &mut [u8],
	) -> Result<usize, BusFailure> {
		if request.bus_id != 0 {
			return Err(BusFailure::new(Status::Unsupported, 0));
		}
		let mut messages = [None; rp_extio_wire::MAX_I2C_MESSAGES];
		for (index, message) in request.messages().enumerate() {
			messages[index] =
				Some(message.map_err(|_| BusFailure::new(Status::BadFrame, index as u32))?);
		}
		let i2c = self.i2c.as_mut().unwrap();
		let mut index = 0;
		let mut read_offset = 0;
		while index < request.count as usize {
			let message = messages[index].unwrap();
			if message.flags & I2C_TEN_BIT != 0 {
				return Err(BusFailure::new(Status::Unsupported, index as u32));
			}
			let address = message.address as u8;
			if message.flags & I2C_READ == 0
				&& index + 1 < request.count as usize
				&& messages[index + 1]
					.is_some_and(|next| next.flags == I2C_READ && next.address == message.address)
			{
				let next = messages[index + 1].unwrap();
				let end = read_offset + next.len as usize;
				i2c.write_read(address, message.write_data, &mut reads[read_offset..end])
					.map_err(|error| Self::i2c_failure(error, index as u32))?;
				read_offset = end;
				index += 2;
				continue;
			}
			if message.flags & I2C_READ != 0 {
				let end = read_offset + message.len as usize;
				i2c.read(address, &mut reads[read_offset..end])
					.map_err(|error| Self::i2c_failure(error, index as u32))?;
				read_offset = end;
			} else {
				i2c.write(address, message.write_data)
					.map_err(|error| Self::i2c_failure(error, index as u32))?;
			}
			index += 1;
		}
		Ok(read_offset)
	}

	fn spi_message(
		&mut self,
		request: &SpiRequest<'_>,
		receive: &mut [u8],
	) -> Result<SpiOutcome, BusFailure> {
		if request.bus_id != 0 {
			return Err(BusFailure::new(Status::Unsupported, 0));
		}
		cumulative_spi_delay_ns(request)?;
		let mode = match request.mode & (SPI_CPOL | SPI_CPHA) {
			0 => Mode::_0,
			SPI_CPHA => Mode::_1,
			SPI_CPOL => Mode::_2,
			_ => Mode::_3,
		};
		let order = if request.mode & SPI_LSB_FIRST != 0 {
			BitOrder::LsbFirst
		} else {
			BitOrder::MsbFirst
		};
		let no_cs = request.mode & SPI_NO_CS != 0;
		let cs_high = request.mode & SPI_CS_HIGH != 0;
		let cs_pin =
			(!no_cs).then(|| self.pin_map.spi_chip_selects[0][request.chip_select as usize]);
		if let Some(cs_pin) = cs_pin {
			self.pin_mut(cs_pin)
				.set_level(if cs_high { Level::High } else { Level::Low });
		}
		let result = (|| {
			let mut outcome = SpiOutcome::new();
			let mut offset = 0;
			let delay = Delay::new();
			for (index, segment) in request.segments().enumerate() {
				let segment =
					segment.map_err(|_| BusFailure::new(Status::BadFrame, offset as u32))?;
				if !(ESP32_C3_SPI_MIN_SPEED_HZ..=SPI_MAX_SPEED_HZ).contains(&segment.speed_hz) {
					return Err(BusFailure::new(Status::Unsupported, offset as u32));
				}
				let config = SpiConfig::default()
					.with_frequency(Rate::from_hz(segment.speed_hz))
					.with_mode(mode)
					.with_read_bit_order(order)
					.with_write_bit_order(order);
				let spi = self.spi.as_mut().unwrap();
				spi.apply_config(&config)
					.map_err(|_| BusFailure::new(Status::Unsupported, offset as u32))?;
				let end = offset + segment.len as usize;
				receive[offset..end].copy_from_slice(segment.tx);
				if segment.word_delay_ns == 0 {
					spi.transfer(&mut receive[offset..end])
						.map_err(|_| BusFailure::new(Status::IoError, offset as u32))?;
				} else {
					for byte in &mut receive[offset..end] {
						spi.transfer(core::slice::from_mut(byte))
							.map_err(|_| BusFailure::new(Status::IoError, offset as u32))?;
						delay.delay_nanos(segment.word_delay_ns);
					}
				}
				offset = end;
				outcome.completed_bytes = offset as u32;
				outcome.effective_speeds[index] = Self::spi_effective_speed_hz(segment.speed_hz);
				delay.delay_nanos(segment.delay_ns);
				if let Some(cs_pin) = cs_pin.filter(|_| segment.flags & SPI_CS_CHANGE != 0) {
					self.pin_mut(cs_pin)
						.set_level(if cs_high { Level::Low } else { Level::High });
					delay.delay_nanos(segment.cs_change_delay_ns);
					self.pin_mut(cs_pin)
						.set_level(if cs_high { Level::High } else { Level::Low });
				}
			}
			Ok(outcome)
		})();
		if let Some(cs_pin) = cs_pin {
			self.pin_mut(cs_pin)
				.set_level(if cs_high { Level::Low } else { Level::High });
		}
		result
	}

	fn gpio_command(&mut self, command: u8, line: u8, value: u8) -> Result<u8, BusFailure> {
		let number = self.gpio_pin(line)?;
		let mask = 1_u32 << line;
		match command {
			GPIO_GET => Ok(u8::from(self.pin(number).is_high())),
			GPIO_SET if self.gpio_outputs & mask != 0 => {
				self.pin_mut(number)
					.set_level(if value == 0 { Level::Low } else { Level::High });
				Ok(value)
			}
			GPIO_DIRECTION_INPUT => {
				self.pin_mut(number).set_output_enable(false);
				self.gpio_outputs &= !mask;
				Ok(1)
			}
			GPIO_DIRECTION_OUTPUT => {
				let pin = self.pin_mut(number);
				pin.set_level(if value == 0 { Level::Low } else { Level::High });
				pin.set_output_enable(true);
				self.gpio_outputs |= mask;
				Ok(0)
			}
			GPIO_GET_DIRECTION => Ok(u8::from(self.gpio_outputs & mask == 0)),
			GPIO_SET_BIAS => {
				let pull = match value {
					GPIO_BIAS_NONE => Pull::None,
					GPIO_BIAS_PULL_UP => Pull::Up,
					GPIO_BIAS_PULL_DOWN => Pull::Down,
					_ => return Err(BusFailure::new(Status::BadFrame, 0)),
				};
				self.pin_mut(number)
					.apply_input_config(&InputConfig::default().with_pull(pull));
				self.gpio_pull_ups &= !mask;
				self.gpio_pull_downs &= !mask;
				if value == GPIO_BIAS_PULL_UP {
					self.gpio_pull_ups |= mask;
				} else if value == GPIO_BIAS_PULL_DOWN {
					self.gpio_pull_downs |= mask;
				}
				Ok(value)
			}
			_ => Err(BusFailure::new(Status::Unsupported, 0)),
		}
	}

	fn gpio_snapshot(&mut self) -> Result<u32, BusFailure> {
		let pins = self
			.pin_map
			.gpio_pins_for(BoardKind::Esp32C3DevKitM1.profile())
			.map_err(|_| BusFailure::new(Status::IoError, 0))?;
		let mut values = 0;
		for (line, &number) in pins.as_slice().iter().enumerate() {
			if self.pin(number).is_high() {
				values |= 1 << line;
			}
		}
		Ok(values)
	}

	fn gpio_directions(&self) -> u32 {
		self.gpio_outputs
	}

	fn gpio_pull_ups(&self) -> u32 {
		self.gpio_pull_ups
	}

	fn gpio_pull_downs(&self) -> u32 {
		self.gpio_pull_downs
	}

	fn adc_read(&mut self, channel: u8) -> Result<u16, BusFailure> {
		if channel as usize >= rp_extio_wire::ESP32_C3_ADC_CHANNEL_COUNT
			|| self.pin_map.adc_enabled & (1 << channel) == 0
		{
			return Err(BusFailure::new(Status::Unsupported, 0));
		}
		let value = match channel {
			0 => nb::block!(self.adc.read_oneshot(&mut self.adc0)),
			1 => nb::block!(self.adc.read_oneshot(&mut self.adc1)),
			2 => nb::block!(self.adc.read_oneshot(&mut self.adc2)),
			3 => nb::block!(self.adc.read_oneshot(&mut self.adc3)),
			4 => nb::block!(self.adc.read_oneshot(&mut self.adc4)),
			_ => unreachable!(),
		};
		value.map_err(|_| BusFailure::new(Status::IoError, 0))
	}

	fn pwm_state(&self, channel: u8) -> Result<PwmState, BusFailure> {
		if channel as usize >= rp_extio_wire::ESP32_C3_PWM_CHANNEL_COUNT
			|| self.pin_map.pwm_enabled & (1 << channel) == 0
		{
			return Err(BusFailure::new(Status::Unsupported, 0));
		}
		Ok(self.pwm_states[channel as usize])
	}

	fn pwm_apply(&mut self, requested: PwmState) -> Result<PwmState, BusFailure> {
		const CHANNELS: [channel::Number; 6] = [
			channel::Number::Channel0,
			channel::Number::Channel1,
			channel::Number::Channel2,
			channel::Number::Channel3,
			channel::Number::Channel4,
			channel::Number::Channel5,
		];
		const TIMERS: [timer::Number; 4] = [
			timer::Number::Timer0,
			timer::Number::Timer1,
			timer::Number::Timer2,
			timer::Number::Timer3,
		];
		let channel = requested.channel as usize;
		if channel >= rp_extio_wire::ESP32_C3_PWM_CHANNEL_COUNT
			|| self.pin_map.pwm_enabled & (1 << channel) == 0
		{
			return Err(BusFailure::new(Status::Unsupported, 0));
		}
		let timer_index = channel % 4;
		if let Some(sibling) = (0..rp_extio_wire::ESP32_C3_PWM_CHANNEL_COUNT).find(|&other| {
			other != channel
				&& other % 4 == timer_index
				&& self.pwm_states[other].enabled
				&& self.pwm_states[other].period_ns != requested.period_ns
		}) {
			return Err(BusFailure::new(Status::Busy, sibling as u32));
		}
		let pin_number = self.pin_map.pwm_pins[channel];
		if !requested.enabled {
			let mut pin = self.route_flex(pin_number);
			pin.set_level(if requested.inverted {
				Level::High
			} else {
				Level::Low
			});
			pin.set_output_enable(true);
			self.pwm_states[channel] = requested;
			return Ok(requested);
		}
		let frequency = (1_000_000_000_u64 / requested.period_ns) as u32;
		if frequency == 0 {
			return Err(BusFailure::new(Status::Unsupported, 0));
		}
		let mut selected = None;
		for bits in (1..=14).rev() {
			let divisor = ((80_000_000_u64 << 8) / u64::from(frequency)) >> bits;
			if (256..=0x3ffff).contains(&divisor) {
				selected = Some((bits, timer::config::Duty::try_from(bits).unwrap()));
				break;
			}
		}
		let Some((bits, duty_resolution)) = selected else {
			return Err(BusFailure::new(Status::Unsupported, 0));
		};
		let mut timer = self.ledc.timer::<LowSpeed>(TIMERS[timer_index]);
		timer
			.configure(timer::config::Config {
				duty: duty_resolution,
				clock_source: timer::LSClockSource::APBClk,
				frequency: Rate::from_hz(frequency),
			})
			.map_err(|_| BusFailure::new(Status::Unsupported, 0))?;
		let output = self
			.route_flex(pin_number)
			.into_peripheral_output()
			.with_output_inverter(requested.inverted);
		let mut output = self.ledc.channel::<LowSpeed>(CHANNELS[channel], output);
		output
			.configure(channel::config::Config {
				timer: &timer,
				duty_pct: 0,
				drive_mode: DriveMode::PushPull,
			})
			.map_err(|_| BusFailure::new(Status::IoError, 0))?;
		let maximum = (1_u64 << bits) - 1;
		let duty = requested.duty_ns.saturating_mul(maximum) / requested.period_ns;
		output
			.set_duty_cycle(duty as u16)
			.map_err(|_| BusFailure::new(Status::IoError, 0))?;
		let effective_period = 1_000_000_000_u64 / u64::from(frequency);
		let state = PwmState {
			period_ns: effective_period,
			duty_ns: duty * effective_period / maximum,
			..requested
		};
		self.pwm_states[channel] = state;
		Ok(state)
	}

	fn onboard_led_state(&self) -> Result<LedState, BusFailure> {
		if self.onboard_led_busy() {
			return Err(BusFailure::new(Status::Busy, 0));
		}
		Ok(self.led_state)
	}

	fn onboard_led_apply(&mut self, state: LedState) -> Result<LedState, BusFailure> {
		if self.onboard_led_busy() {
			return Err(BusFailure::new(Status::Busy, 0));
		}
		self.transmit_led(state)?;
		self.led_state = state;
		Ok(state)
	}
}
