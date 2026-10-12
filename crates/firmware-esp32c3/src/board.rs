use esp_hal::{
	delay::Delay,
	gpio::{AnyPin, DriveMode, Flex, InputConfig, Level, OutputConfig, Pull},
	i2c::master::{Config as I2cConfig, Error as I2cError, I2c},
	peripherals::{I2C0, SPI2},
	spi::{
		BitOrder, Mode,
		master::{Config as SpiConfig, Spi},
	},
	time::Rate,
};
use rp_extio_firmware::{
	Board, BusFailure, SPI_CPHA, SPI_CPOL, SPI_CS_HIGH, SPI_LSB_FIRST, SPI_MAX_SPEED_HZ,
	SPI_MIN_SPEED_HZ, SPI_NO_CS, SpiOutcome, cumulative_spi_delay_ns,
};
use rp_extio_wire::{
	BoardKind, DeviceIdentity, ESP32_C3_GPIO_PINS, GPIO_BIAS_NONE, GPIO_BIAS_PULL_DOWN,
	GPIO_BIAS_PULL_UP, GPIO_DIRECTION_INPUT, GPIO_DIRECTION_OUTPUT, GPIO_GET, GPIO_GET_DIRECTION,
	GPIO_SET, GPIO_SET_BIAS, I2C_READ, I2C_TEN_BIT, I2cRequest, LedState, PinMap,
	PinValidationError, PwmState, SPI_CS_CHANGE, SpiRequest, Status,
};

pub struct Esp32C3Board<'d> {
	pins: [Flex<'d>; ESP32_C3_GPIO_PINS.len()],
	route_pins: [AnyPin<'d>; ESP32_C3_GPIO_PINS.len()],
	i2c: Option<I2c<'d, esp_hal::Blocking>>,
	spi: Option<Spi<'d, esp_hal::Blocking>>,
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
				Spi::new(spi, SpiConfig::default().with_frequency(Rate::from_khz(50))).unwrap(),
			),
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

	fn configure_pins(&mut self, map: PinMap, generation: u64) -> Result<(), PinValidationError> {
		map.validate_for(BoardKind::Esp32C3DevKitM1.profile())?;
		for pin in &mut self.pins {
			pin.set_low();
			pin.set_output_enable(false);
			pin.set_input_enable(true);
			pin.apply_input_config(&InputConfig::default());
		}
		self.pin_map = map;
		self.generation = generation;
		self.gpio_outputs = 0;
		self.gpio_pull_ups = 0;
		self.gpio_pull_downs = 0;
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
		let mut outcome = SpiOutcome::new();
		let mut offset = 0;
		let delay = Delay::new();
		for (index, segment) in request.segments().enumerate() {
			let segment = segment.map_err(|_| BusFailure::new(Status::BadFrame, offset as u32))?;
			if !(SPI_MIN_SPEED_HZ..=SPI_MAX_SPEED_HZ).contains(&segment.speed_hz) {
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
			outcome.effective_speeds[index] = segment.speed_hz;
			delay.delay_nanos(segment.delay_ns);
			if let Some(cs_pin) = cs_pin.filter(|_| segment.flags & SPI_CS_CHANGE != 0) {
				self.pin_mut(cs_pin)
					.set_level(if cs_high { Level::Low } else { Level::High });
				delay.delay_nanos(segment.cs_change_delay_ns);
				self.pin_mut(cs_pin)
					.set_level(if cs_high { Level::High } else { Level::Low });
			}
		}
		if let Some(cs_pin) = cs_pin {
			self.pin_mut(cs_pin)
				.set_level(if cs_high { Level::Low } else { Level::High });
		}
		Ok(outcome)
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

	fn adc_read(&mut self, _channel: u8) -> Result<u16, BusFailure> {
		Err(BusFailure::new(Status::Unsupported, 0))
	}

	fn pwm_state(&self, _channel: u8) -> Result<PwmState, BusFailure> {
		Err(BusFailure::new(Status::Unsupported, 0))
	}

	fn pwm_apply(&mut self, _state: PwmState) -> Result<PwmState, BusFailure> {
		Err(BusFailure::new(Status::Unsupported, 0))
	}

	fn onboard_led_state(&self) -> Result<LedState, BusFailure> {
		Err(BusFailure::new(Status::Unsupported, 0))
	}
}
