use esp_hal::gpio::{Flex, InputConfig, Level, Pull};
use rp_extio_firmware::{Board, BusFailure, SpiOutcome};
use rp_extio_wire::{
	BoardKind, DeviceIdentity, ESP32_C3_GPIO_PINS, GPIO_BIAS_NONE, GPIO_BIAS_PULL_DOWN,
	GPIO_BIAS_PULL_UP, GPIO_DIRECTION_INPUT, GPIO_DIRECTION_OUTPUT, GPIO_GET, GPIO_GET_DIRECTION,
	GPIO_SET, GPIO_SET_BIAS, I2cRequest, LedState, PinMap, PinValidationError, PwmState,
	SpiRequest, Status,
};

pub struct Esp32C3Board<'d> {
	pins: [Flex<'d>; ESP32_C3_GPIO_PINS.len()],
	pin_map: PinMap,
	generation: u64,
	identity: DeviceIdentity,
	gpio_outputs: u32,
	gpio_pull_ups: u32,
	gpio_pull_downs: u32,
}

impl<'d> Esp32C3Board<'d> {
	pub fn new(mut pins: [Flex<'d>; ESP32_C3_GPIO_PINS.len()], identity: DeviceIdentity) -> Self {
		for pin in &mut pins {
			pin.set_output_enable(false);
			pin.set_input_enable(true);
			pin.apply_input_config(&InputConfig::default());
		}
		Self {
			pins,
			pin_map: rp_extio_wire::ESP32_C3_DEFAULT_PIN_MAP,
			generation: 0,
			identity,
			gpio_outputs: 0,
			gpio_pull_ups: 0,
			gpio_pull_downs: 0,
		}
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
		Ok(())
	}

	fn i2c_transfer(
		&mut self,
		_request: &I2cRequest<'_>,
		_reads: &mut [u8],
	) -> Result<usize, BusFailure> {
		Err(BusFailure::new(Status::Unsupported, 0))
	}

	fn spi_message(
		&mut self,
		_request: &SpiRequest<'_>,
		_receive: &mut [u8],
	) -> Result<SpiOutcome, BusFailure> {
		Err(BusFailure::new(Status::Unsupported, 0))
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
