use core::sync::atomic::{AtomicU8, Ordering};
use embassy_rp::adc::{Adc, Blocking};
use embassy_rp::gpio::{Flex, Level, Pull};
use embassy_rp::peripherals::{ADC, SPI0, SPI1, UART0, UART1};
use embassy_rp::{Peri, pac};
use rp_extio_firmware::{
	Board, BusFailure, SPI_CPHA, SPI_CPOL, SPI_CS_HIGH, SPI_LSB_FIRST, SPI_MAX_SPEED_HZ,
	SPI_MIN_SPEED_HZ, SPI_NO_CS, SpiOutcome, UART_BAUD, cumulative_spi_delay_ns,
};
use rp_extio_wire::{
	BoardKind, ClockSource, DeviceIdentity, GPIO_BIAS_NONE, GPIO_BIAS_PULL_DOWN, GPIO_BIAS_PULL_UP,
	GPIO_DIRECTION_INPUT, GPIO_DIRECTION_OUTPUT, GPIO_GET, GPIO_GET_DIRECTION, GPIO_SET,
	GPIO_SET_BIAS, HEADER_PINS, I2C_READ, I2C_TEN_BIT, I2cRequest, LedState, Peripheral, PinMap,
	PinSignal, PinValidationError, PinValidationKind, PwmState, SpiRequest, Status, clock_divider,
};

const TRANSACTION_DEADLINE_US: u64 = 5_000_000;
const I2C_HALF_PERIOD_US: u64 = 5;
const CLOCK_STRETCH_US: u64 = 5_000_000;
static UART_ENABLED: AtomicU8 = AtomicU8::new(0);

pub struct Pico2Board<'d> {
	pins: [Flex<'d>; HEADER_PINS.len()],
	led: Flex<'d>,
	_spi0: Peri<'d, SPI0>,
	_spi1: Peri<'d, SPI1>,
	_uart0: Peri<'d, UART0>,
	_uart1: Peri<'d, UART1>,
	_adc: Adc<'d, Blocking>,
	cs_active_high: [[bool; 2]; 2],
	gpio_outputs: u32,
	gpio_pull_ups: u32,
	gpio_pull_downs: u32,
	pwm_states: [PwmState; rp_extio_wire::PWM_CHANNEL_COUNT],
	led_state: LedState,
	pin_map: PinMap,
	generation: u64,
	identity: DeviceIdentity,
}

impl<'d> Pico2Board<'d> {
	pub fn new(
		mut pins: [Flex<'d>; HEADER_PINS.len()],
		mut led: Flex<'d>,
		spi0: Peri<'d, SPI0>,
		spi1: Peri<'d, SPI1>,
		uart0: Peri<'d, UART0>,
		uart1: Peri<'d, UART1>,
		adc: Peri<'d, ADC>,
		pin_map: PinMap,
		identity: DeviceIdentity,
	) -> Self {
		for pin in &mut pins {
			pin.set_as_input();
			pin.set_pull(Pull::None);
		}
		led.set_low();
		led.set_as_output();
		let mut board = Self {
			pins,
			led,
			_spi0: spi0,
			_spi1: spi1,
			_uart0: uart0,
			_uart1: uart1,
			_adc: Adc::new_blocking(adc, Default::default()),
			cs_active_high: [[false; 2]; 2],
			gpio_outputs: 0,
			gpio_pull_ups: 0,
			gpio_pull_downs: 0,
			pwm_states: core::array::from_fn(|channel| PwmState {
				channel: channel as u8,
				enabled: false,
				inverted: false,
				period_ns: 1_000_000,
				duty_ns: 0,
			}),
			led_state: LedState::OFF,
			pin_map,
			generation: 0,
			identity,
		};
		board.apply_pin_map(pin_map);
		board
	}

	fn pin(&self, number: u8) -> &Flex<'d> {
		&self.pins[HEADER_PINS.iter().position(|&pin| pin == number).unwrap()]
	}

	fn pin_mut(&mut self, number: u8) -> &mut Flex<'d> {
		&mut self.pins[HEADER_PINS.iter().position(|&pin| pin == number).unwrap()]
	}

	fn release(&mut self, number: u8) {
		let pin = self.pin_mut(number);
		pin.set_low();
		pin.set_as_input();
	}

	fn drive_low(&mut self, number: u8) {
		let pin = self.pin_mut(number);
		pin.set_low();
		pin.set_as_output();
	}

	fn set_output(&mut self, number: u8, level: Level) {
		let pin = self.pin_mut(number);
		pin.set_level(level);
		pin.set_as_output();
	}

	fn connected() -> bool {
		let status = pac::USB.sie_status().read();
		status.connected() && !status.suspended()
	}

	fn now_us() -> u64 {
		loop {
			let high = pac::TIMER0.timerawh().read();
			let low = pac::TIMER0.timerawl().read();
			if high == pac::TIMER0.timerawh().read() {
				return (u64::from(high) << 32) | u64::from(low);
			}
		}
	}

	fn wait_until(deadline: u64) -> Result<(), BusFailure> {
		while Self::now_us() < deadline {
			if !Self::connected() {
				return Err(BusFailure::new(Status::Disconnected, 0));
			}
			core::hint::spin_loop();
		}
		Ok(())
	}

	fn delay_us(duration: u64) -> Result<(), BusFailure> {
		Self::wait_until(Self::now_us().saturating_add(duration))
	}

	fn delay_ns(duration: u32) -> Result<(), BusFailure> {
		Self::delay_us(u64::from(duration).div_ceil(1_000))
	}

	fn release_buses(&mut self) {
		for bus in 0..2 {
			if self
				.pin_map
				.is_enabled([Peripheral::I2c0, Peripheral::I2c1][bus])
			{
				self.release(self.pin_map.i2c_sda[bus]);
				self.release(self.pin_map.i2c_scl[bus]);
			}
			if self
				.pin_map
				.is_enabled([Peripheral::Spi0, Peripheral::Spi1][bus])
			{
				for index in 0..self.pin_map.spi_chip_select_count(bus) as usize {
					let level = if self.cs_active_high[bus][index] {
						Level::Low
					} else {
						Level::High
					};
					self.pin_mut(self.pin_map.spi_chip_selects[bus][index])
						.set_level(level);
				}
			}
		}
	}

	fn wait_scl_high(&self, bus: usize, transaction_deadline: u64) -> Result<(), BusFailure> {
		let stretch_deadline = Self::now_us().saturating_add(CLOCK_STRETCH_US);
		while self.pin(self.pin_map.i2c_scl[bus]).is_low() {
			if !Self::connected() {
				return Err(BusFailure::new(Status::Disconnected, 0));
			}
			if Self::now_us() >= stretch_deadline || Self::now_us() >= transaction_deadline {
				return Err(BusFailure::new(Status::Timeout, 0));
			}
			core::hint::spin_loop();
		}
		Ok(())
	}

	fn clock_high(&mut self, bus: usize, deadline: u64) -> Result<(), BusFailure> {
		self.release(self.pin_map.i2c_scl[bus]);
		self.wait_scl_high(bus, deadline)?;
		Self::delay_us(I2C_HALF_PERIOD_US)
	}

	fn clock_low(&mut self, bus: usize) -> Result<(), BusFailure> {
		self.drive_low(self.pin_map.i2c_scl[bus]);
		Self::delay_us(I2C_HALF_PERIOD_US)
	}

	fn recover_i2c(&mut self, bus: usize, deadline: u64) -> Result<(), BusFailure> {
		self.release(self.pin_map.i2c_sda[bus]);
		self.release(self.pin_map.i2c_scl[bus]);
		self.wait_scl_high(bus, deadline)?;
		if self.pin(self.pin_map.i2c_sda[bus]).is_high() {
			return Ok(());
		}
		for _ in 0..9 {
			self.clock_low(bus)?;
			self.clock_high(bus, deadline)?;
			if self.pin(self.pin_map.i2c_sda[bus]).is_high() {
				break;
			}
		}
		self.drive_low(self.pin_map.i2c_sda[bus]);
		Self::delay_us(I2C_HALF_PERIOD_US)?;
		self.clock_high(bus, deadline)?;
		self.release(self.pin_map.i2c_sda[bus]);
		Self::delay_us(I2C_HALF_PERIOD_US)?;
		if self.pin(self.pin_map.i2c_sda[bus]).is_low() {
			Err(BusFailure::new(Status::Timeout, 0))
		} else {
			Ok(())
		}
	}

	fn i2c_start(&mut self, bus: usize, deadline: u64) -> Result<(), BusFailure> {
		self.release(self.pin_map.i2c_sda[bus]);
		self.clock_high(bus, deadline)?;
		if self.pin(self.pin_map.i2c_sda[bus]).is_low() {
			return Err(BusFailure::new(Status::ArbitrationLost, 0));
		}
		self.drive_low(self.pin_map.i2c_sda[bus]);
		Self::delay_us(I2C_HALF_PERIOD_US)?;
		self.clock_low(bus)
	}

	fn i2c_stop(&mut self, bus: usize, deadline: u64) -> Result<(), BusFailure> {
		self.drive_low(self.pin_map.i2c_sda[bus]);
		Self::delay_us(I2C_HALF_PERIOD_US)?;
		self.clock_high(bus, deadline)?;
		self.release(self.pin_map.i2c_sda[bus]);
		Self::delay_us(I2C_HALF_PERIOD_US)
	}

	fn i2c_write_byte(&mut self, bus: usize, byte: u8, deadline: u64) -> Result<bool, BusFailure> {
		for bit in (0..8).rev() {
			let released = byte & (1 << bit) != 0;
			if released {
				self.release(self.pin_map.i2c_sda[bus]);
			} else {
				self.drive_low(self.pin_map.i2c_sda[bus]);
			}
			self.clock_high(bus, deadline)?;
			if released && self.pin(self.pin_map.i2c_sda[bus]).is_low() {
				self.drive_low(self.pin_map.i2c_scl[bus]);
				self.release(self.pin_map.i2c_sda[bus]);
				return Err(BusFailure::new(Status::ArbitrationLost, 0));
			}
			self.clock_low(bus)?;
		}
		self.release(self.pin_map.i2c_sda[bus]);
		self.clock_high(bus, deadline)?;
		let acknowledged = self.pin(self.pin_map.i2c_sda[bus]).is_low();
		self.clock_low(bus)?;
		Ok(acknowledged)
	}

	fn i2c_read_byte(
		&mut self,
		bus: usize,
		acknowledge: bool,
		deadline: u64,
	) -> Result<u8, BusFailure> {
		self.release(self.pin_map.i2c_sda[bus]);
		let mut byte = 0;
		for _ in 0..8 {
			self.clock_high(bus, deadline)?;
			byte = (byte << 1) | u8::from(self.pin(self.pin_map.i2c_sda[bus]).is_high());
			self.clock_low(bus)?;
		}
		if acknowledge {
			self.drive_low(self.pin_map.i2c_sda[bus]);
		} else {
			self.release(self.pin_map.i2c_sda[bus]);
		}
		self.clock_high(bus, deadline)?;
		self.clock_low(bus)?;
		self.release(self.pin_map.i2c_sda[bus]);
		Ok(byte)
	}

	fn address(
		&mut self,
		bus: usize,
		address: u16,
		ten_bit: bool,
		read: bool,
		deadline: u64,
	) -> Result<bool, BusFailure> {
		if !ten_bit {
			return self.i2c_write_byte(bus, ((address as u8) << 1) | u8::from(read), deadline);
		}

		let prefix = 0b1111_0000 | (((address >> 7) as u8) & 0b110);
		if !self.i2c_write_byte(bus, prefix, deadline)?
			|| !self.i2c_write_byte(bus, address as u8, deadline)?
		{
			return Ok(false);
		}
		if read {
			self.i2c_start(bus, deadline)?;
			self.i2c_write_byte(bus, prefix | 1, deadline)
		} else {
			Ok(true)
		}
	}

	fn set_cs(&mut self, bus: usize, index: u8, active_high: bool, asserted: bool) {
		let high = asserted == active_high;
		let pin = self.pin_map.spi_chip_selects[bus][index as usize];
		self.pin_mut(pin)
			.set_level(if high { Level::High } else { Level::Low });
	}

	fn set_function(pin: u8, function: u8) {
		pac::IO_BANK0
			.gpio(pin as usize)
			.ctrl()
			.write(|register| register.set_funcsel(function));
	}

	fn neutralize(&mut self) {
		self.release_buses();
		pac::SPI0.cr1().write(|register| register.set_sse(false));
		pac::SPI1.cr1().write(|register| register.set_sse(false));
		pac::UART0.uartcr().write(|_| {});
		pac::UART1.uartcr().write(|_| {});
		UART_ENABLED.store(0, Ordering::Release);
		for slice in 0..8 {
			pac::PWM
				.ch(slice)
				.csr()
				.modify(|register| register.set_en(false));
		}
		for output in 0..rp_extio_wire::CLOCK_OUTPUT_COUNT {
			pac::CLOCKS
				.clk_gpout_ctrl(output)
				.modify(|register| register.set_enable(false));
		}
		for (number, pin) in HEADER_PINS.into_iter().zip(&mut self.pins) {
			pin.set_as_input();
			pin.set_pull(Pull::None);
			pac::PADS_BANK0.gpio(number as usize).modify(|register| {
				register.set_iso(false);
				register.set_ie(true);
				register.set_od(false);
			});
			Self::set_function(number, 5);
		}
	}

	fn uart(bus: usize) -> pac::uart::Uart {
		if bus == 0 { pac::UART0 } else { pac::UART1 }
	}

	fn spi(bus: usize) -> pac::spi::Spi {
		if bus == 0 { pac::SPI0 } else { pac::SPI1 }
	}

	fn configure_uart(bus: usize) {
		let clock = embassy_rp::clocks::clk_peri_freq();
		let divisor = (8 * clock) / UART_BAUD;
		let integer = (divisor >> 7).clamp(1, 65_535);
		let fraction = if integer == 65_535 {
			0
		} else {
			((divisor & 0x7f) + 1) / 2
		};
		let uart = Self::uart(bus);
		uart.uartibrd()
			.write(|register| register.set_baud_divint(integer as u16));
		uart.uartfbrd()
			.write(|register| register.set_baud_divfrac(fraction as u8));
		uart.uartlcr_h().write(|register| {
			register.set_wlen(3);
			register.set_fen(true);
		});
		uart.uartifls().write(|register| {
			register.set_rxiflsel(0b100);
			register.set_txiflsel(0);
		});
		uart.uartcr().write(|register| {
			register.set_uarten(true);
			register.set_rxe(true);
			register.set_txe(true);
		});
	}

	fn configure_spi(bus: usize, speed_hz: u32, phase: bool, polarity: bool) -> u32 {
		let peripheral = embassy_rp::clocks::clk_peri_freq();
		let ratio = peripheral.div_ceil(speed_hz.saturating_mul(2));
		let prescaler = ratio.div_ceil(256).max(1);
		let postdivider = if prescaler == 1 {
			ratio
		} else {
			ratio.div_ceil(prescaler)
		}
		.clamp(1, 256);
		let spi = Self::spi(bus);
		spi.cr1().write(|register| register.set_sse(false));
		spi.cpsr()
			.write(|register| register.set_cpsdvsr((prescaler * 2) as u8));
		spi.cr0().write(|register| {
			register.set_dss(0b0111);
			register.set_spo(polarity);
			register.set_sph(phase);
			register.set_scr((postdivider - 1) as u8);
		});
		spi.dmacr().write(|_| {});
		spi.cr1().write(|register| register.set_sse(true));
		peripheral / (prescaler * 2) / postdivider
	}

	fn apply_pin_map(&mut self, map: PinMap) {
		self.neutralize();
		self.pin_map = map;
		self.gpio_outputs = 0;
		self.gpio_pull_ups = 0;
		self.gpio_pull_downs = 0;
		for state in &mut self.pwm_states {
			state.enabled = false;
			state.inverted = false;
			state.duty_ns = 0;
		}
		self.cs_active_high = [[false; 2]; 2];

		let mut uart_enabled = 0;
		for bus in 0..2 {
			if map.is_enabled([Peripheral::I2c0, Peripheral::I2c1][bus]) {
				self.release(map.i2c_sda[bus]);
				self.release(map.i2c_scl[bus]);
			}
			if map.is_enabled([Peripheral::Spi0, Peripheral::Spi1][bus]) {
				for &chip_select in
					&map.spi_chip_selects[bus][..map.spi_chip_select_count(bus) as usize]
				{
					self.set_output(chip_select, Level::High);
				}
				for pin in [map.spi_miso[bus], map.spi_sck[bus], map.spi_mosi[bus]] {
					Self::set_function(pin, 1);
				}
				Self::configure_spi(bus, SPI_MIN_SPEED_HZ, false, false);
			}
			if map.is_enabled([Peripheral::Uart0, Peripheral::Uart1][bus]) {
				let tx_function = if map.uart_tx[bus] % 4 == 0 { 2 } else { 11 };
				let rx_function = if (map.uart_rx[bus] - 1) % 4 == 0 {
					2
				} else {
					11
				};
				Self::set_function(map.uart_tx[bus], tx_function);
				Self::set_function(map.uart_rx[bus], rx_function);
				Self::configure_uart(bus);
				uart_enabled |= 1 << bus;
			}
		}
		for channel in 0..rp_extio_wire::ADC_CHANNEL_COUNT {
			if map.adc_enabled & (1 << channel) != 0 {
				let pin = map.adc_pins[channel];
				Self::set_function(pin, 31);
				pac::PADS_BANK0.gpio(pin as usize).modify(|register| {
					register.set_iso(false);
					register.set_ie(false);
					register.set_od(true);
					register.set_pue(false);
					register.set_pde(false);
				});
			}
		}
		for channel in 0..rp_extio_wire::PWM_CHANNEL_COUNT {
			if map.pwm_enabled & (1 << channel) != 0 {
				Self::set_function(map.pwm_pins[channel], 4);
			}
		}
		for output in 0..rp_extio_wire::CLOCK_OUTPUT_COUNT {
			if map.clock_enabled & (1 << output) != 0 {
				self.pin_map.clock_hz[output] =
					Self::configure_clock(output, map.clock_sources[output], map.clock_hz[output])
						.expect("clock configuration was validated");
				Self::set_function(map.clock_pins[output], 9);
				pac::CLOCKS
					.clk_gpout_ctrl(output)
					.modify(|register| register.set_enable(true));
			}
		}
		UART_ENABLED.store(uart_enabled, Ordering::Release);
	}

	fn clock_source(source: ClockSource) -> (u8, u32) {
		match source {
			ClockSource::System => (8, embassy_rp::clocks::clk_sys_freq()),
			ClockSource::Reference => (11, embassy_rp::clocks::clk_ref_freq()),
			ClockSource::Usb => (9, embassy_rp::clocks::clk_usb_freq()),
			ClockSource::Adc => (10, embassy_rp::clocks::clk_adc_freq()),
			ClockSource::Crystal => (6, embassy_rp::clocks::xosc_freq()),
			ClockSource::RingOscillator => (5, embassy_rp::clocks::rosc_freq()),
			ClockSource::LowPowerOscillator => (7, 32_768),
		}
	}

	fn clock_divider(source: ClockSource, target_hz: u32) -> Option<rp_extio_wire::ClockDivider> {
		let (_, source_hz) = Self::clock_source(source);
		clock_divider(source_hz, target_hz)
	}

	fn configure_clock(output: usize, source: ClockSource, target_hz: u32) -> Option<u32> {
		let (source_select, _) = Self::clock_source(source);
		let divider = Self::clock_divider(source, target_hz)?;
		pac::CLOCKS
			.clk_gpout_ctrl(output)
			.modify(|register| register.set_enable(false));
		pac::CLOCKS.clk_gpout_div(output).write(|register| {
			register.set_int(divider.integer);
			register.set_frac(divider.fraction);
		});
		pac::CLOCKS
			.clk_gpout_ctrl(output)
			.modify(|register| register.set_auxsrc(source_select.into()));
		Some(divider.effective_hz)
	}

	fn spi_transfer(bus: usize, byte: u8) -> u8 {
		let spi = Self::spi(bus);
		while !spi.sr().read().tnf() {}
		spi.dr().write(|register| register.set_data(byte as u16));
		while !spi.sr().read().rne() {}
		spi.dr().read().data() as u8
	}

	pub fn uart_enabled(bus: usize) -> bool {
		UART_ENABLED.load(Ordering::Acquire) & (1 << bus) != 0
	}

	pub fn uart_tx_full(bus: usize) -> bool {
		Self::uart(bus).uartfr().read().txff()
	}

	pub fn uart_write(bus: usize, byte: u8) {
		Self::uart(bus)
			.uartdr()
			.write(|register| register.set_data(byte));
	}

	pub fn uart_read(bus: usize, buffer: &mut [u8]) -> usize {
		if !Self::uart_enabled(bus) {
			return 0;
		}
		let uart = Self::uart(bus);
		let mut count = 0;
		while count < buffer.len() && !uart.uartfr().read().rxfe() {
			let value = uart.uartdr().read();
			if !value.fe() && !value.pe() && !value.be() && !value.oe() {
				buffer[count] = value.data();
				count += 1;
			}
		}
		count
	}
}

impl Board for Pico2Board<'_> {
	fn board_kind(&self) -> BoardKind {
		BoardKind::Pico2
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
		map.validate()?;
		for output in 0..rp_extio_wire::CLOCK_OUTPUT_COUNT {
			if map.clock_enabled & (1 << output) == 0 {
				continue;
			}
			let Some(_) = Self::clock_divider(map.clock_sources[output], map.clock_hz[output])
			else {
				return Err(PinValidationError {
					kind: PinValidationKind::InvalidRoute,
					signal: [PinSignal::Clock0, PinSignal::Clock1][output],
					other: None,
					pin: map.clock_pins[output],
				});
			};
		}
		self.apply_pin_map(map);
		self.generation = generation;
		Ok(())
	}

	fn i2c_transfer(
		&mut self,
		request: &I2cRequest<'_>,
		reads: &mut [u8],
	) -> Result<usize, BusFailure> {
		let bus = request.bus_id as usize;
		let result = (|| {
			let deadline = Self::now_us().saturating_add(TRANSACTION_DEADLINE_US);
			self.recover_i2c(bus, deadline)?;
			let mut read_offset = 0;

			for (completed, message) in request.messages().enumerate() {
				let completed = completed as u32;
				let message = message.map_err(|_| BusFailure::new(Status::BadFrame, completed))?;
				let result = (|| {
					self.i2c_start(bus, deadline)?;
					let read = message.flags & I2C_READ != 0;
					if !self.address(
						bus,
						message.address,
						message.flags & I2C_TEN_BIT != 0,
						read,
						deadline,
					)? {
						return Err(BusFailure::new(Status::Nack, completed));
					}
					if read {
						for index in 0..message.len as usize {
							reads[read_offset] = self.i2c_read_byte(
								bus,
								index + 1 < message.len as usize,
								deadline,
							)?;
							read_offset += 1;
						}
					} else {
						for &byte in message.write_data {
							if !self.i2c_write_byte(bus, byte, deadline)? {
								return Err(BusFailure::new(Status::Nack, completed));
							}
						}
					}
					Ok(())
				})();
				if let Err(mut failure) = result {
					failure.completed = completed;
					if failure.status != Status::ArbitrationLost {
						let _ = self.i2c_stop(bus, deadline);
					}
					return Err(failure);
				}
			}
			self.i2c_stop(bus, deadline)?;
			Ok(read_offset)
		})();

		// A failed STOP can return while SDA is still asserted, particularly when SCL is held
		// low. Never allow one failed transaction to leave either open-drain output enabled.
		self.release(self.pin_map.i2c_sda[bus]);
		self.release(self.pin_map.i2c_scl[bus]);
		result
	}

	fn spi_message(
		&mut self,
		request: &SpiRequest<'_>,
		receive: &mut [u8],
	) -> Result<SpiOutcome, BusFailure> {
		let bus = request.bus_id as usize;
		cumulative_spi_delay_ns(request)?;
		for segment in request.segments() {
			let segment = segment.map_err(|_| BusFailure::new(Status::BadFrame, 0))?;
			if !(SPI_MIN_SPEED_HZ..=SPI_MAX_SPEED_HZ).contains(&segment.speed_hz) {
				return Err(BusFailure::new(Status::Unsupported, 0));
			}
		}

		let deadline = Self::now_us().saturating_add(TRANSACTION_DEADLINE_US);
		let no_cs = request.mode & SPI_NO_CS != 0;
		let active_high = request.mode & SPI_CS_HIGH != 0;
		let lsb_first = request.mode & SPI_LSB_FIRST != 0;
		if !no_cs {
			self.cs_active_high[bus][request.chip_select as usize] = active_high;
			self.set_cs(bus, request.chip_select, active_high, false);
			self.set_cs(bus, request.chip_select, active_high, true);
		}

		let result = (|| {
			let mut outcome = SpiOutcome::new();
			let mut offset = 0;
			for (index, segment) in request.segments().enumerate() {
				let segment = segment
					.map_err(|_| BusFailure::new(Status::BadFrame, outcome.completed_bytes))?;
				if Self::now_us() >= deadline || !Self::connected() {
					return Err(BusFailure::new(
						if Self::connected() {
							Status::Timeout
						} else {
							Status::Disconnected
						},
						outcome.completed_bytes,
					));
				}
				outcome.effective_speeds[index] = Self::configure_spi(
					bus,
					segment.speed_hz,
					request.mode & SPI_CPHA != 0,
					request.mode & SPI_CPOL != 0,
				);
				let target = &mut receive[offset..offset + segment.len as usize];
				for (byte_index, (target, &source)) in target.iter_mut().zip(segment.tx).enumerate()
				{
					*target = if lsb_first {
						source.reverse_bits()
					} else {
						source
					};
					*target = Self::spi_transfer(bus, *target);
					if lsb_first {
						*target = target.reverse_bits();
					}
					outcome.completed_bytes += 1;
					if byte_index + 1 < segment.len as usize {
						Self::delay_ns(segment.word_delay_ns)?;
					}
					if Self::now_us() >= deadline || !Self::connected() {
						return Err(BusFailure::new(
							if Self::connected() {
								Status::Timeout
							} else {
								Status::Disconnected
							},
							outcome.completed_bytes,
						));
					}
				}
				offset += segment.len as usize;
				Self::delay_ns(segment.delay_ns)?;

				if segment.flags & rp_extio_wire::SPI_CS_CHANGE != 0
					&& index + 1 < request.count as usize
					&& !no_cs
				{
					self.set_cs(bus, request.chip_select, active_high, false);
					Self::delay_ns(segment.cs_change_delay_ns)?;
					self.set_cs(bus, request.chip_select, active_high, true);
				}
			}
			Ok(outcome)
		})();
		if !no_cs {
			self.set_cs(bus, request.chip_select, active_high, false);
		}
		result
	}

	fn disconnect(&mut self) {
		self.neutralize();
		self.led.set_low();
		self.led_state = LedState::OFF;
	}

	fn gpio_command(&mut self, command: u8, line: u8, value: u8) -> Result<u8, BusFailure> {
		let index = line as usize;
		let Ok(gpio_pins) = self.pin_map.gpio_pins() else {
			return Err(BusFailure::new(Status::IoError, 0));
		};
		let Some(&number) = gpio_pins.as_slice().get(index) else {
			return Err(BusFailure::new(Status::Unsupported, 0));
		};
		let mask = 1_u32 << index;
		match command {
			GPIO_GET => Ok(u8::from(self.pin(number).is_high())),
			GPIO_SET => {
				if self.gpio_outputs & mask == 0 {
					return Err(BusFailure::new(Status::Unsupported, 0));
				}
				self.pin_mut(number)
					.set_level(if value == 0 { Level::Low } else { Level::High });
				Ok(value)
			}
			GPIO_DIRECTION_INPUT => {
				self.pin_mut(number).set_as_input();
				self.gpio_outputs &= !mask;
				Ok(1)
			}
			GPIO_DIRECTION_OUTPUT => {
				let pin = self.pin_mut(number);
				pin.set_level(if value == 0 { Level::Low } else { Level::High });
				pin.set_as_output();
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
				self.pin_mut(number).set_pull(pull);
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
		let gpio_pins = self
			.pin_map
			.gpio_pins()
			.map_err(|_| BusFailure::new(Status::IoError, 0))?;
		let mut values = 0;
		for (index, &number) in gpio_pins.as_slice().iter().enumerate() {
			if self.pin(number).is_high() {
				values |= 1 << index;
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
		if channel as usize >= rp_extio_wire::ADC_CHANNEL_COUNT
			|| self.pin_map.adc_enabled & (1 << channel) == 0
		{
			return Err(BusFailure::new(Status::Unsupported, 0));
		}
		pac::ADC.cs().modify(|register| {
			register.set_ainsel(channel);
			register.set_start_once(true);
			register.set_err(true);
		});
		while !pac::ADC.cs().read().ready() {
			if !Self::connected() {
				return Err(BusFailure::new(Status::Disconnected, 0));
			}
		}
		if pac::ADC.cs().read().err() {
			Err(BusFailure::new(Status::IoError, 0))
		} else {
			Ok(pac::ADC.result().read().result())
		}
	}

	fn pwm_state(&self, channel: u8) -> Result<PwmState, BusFailure> {
		if channel as usize >= rp_extio_wire::PWM_CHANNEL_COUNT
			|| self.pin_map.pwm_enabled & (1 << channel) == 0
		{
			return Err(BusFailure::new(Status::Unsupported, 0));
		}
		Ok(self.pwm_states[channel as usize])
	}

	fn pwm_apply(&mut self, requested: PwmState) -> Result<PwmState, BusFailure> {
		let channel = requested.channel as usize;
		if channel >= rp_extio_wire::PWM_CHANNEL_COUNT
			|| self.pin_map.pwm_enabled & (1 << channel) == 0
		{
			return Err(BusFailure::new(Status::Unsupported, 0));
		}
		let slice = channel / 2;
		let sibling = channel ^ 1;
		let peripheral_clock = embassy_rp::clocks::clk_sys_freq();
		let registers = pac::PWM.ch(slice);

		let (divider, count, effective_period) = if requested.enabled {
			let scaled_cycles = u64::from(peripheral_clock)
				.saturating_mul(requested.period_ns)
				.saturating_mul(16)
				.saturating_add(500_000_000)
				/ 1_000_000_000;
			let divider = scaled_cycles.div_ceil(65_535).clamp(16, 4_095);
			let count = scaled_cycles.div_ceil(divider).clamp(1, 65_535);
			if scaled_cycles > 4_095 * 65_535 {
				return Err(BusFailure::new(Status::Unsupported, 0));
			}
			let effective_period = count.saturating_mul(divider).saturating_mul(1_000_000_000)
				/ (u64::from(peripheral_clock) * 16);
			if self.pwm_states[sibling].enabled
				&& self.pwm_states[sibling].period_ns != effective_period
			{
				return Err(BusFailure::new(Status::Busy, 0));
			}
			(divider as u16, count as u16, effective_period)
		} else {
			let divider = registers.div().read();
			let divider = u16::from(divider.int()) * 16 + u16::from(divider.frac());
			let count = registers.top().read().top().saturating_add(1);
			(divider, count, self.pwm_states[channel].period_ns)
		};

		let compare = if requested.enabled {
			((requested.duty_ns.saturating_mul(u64::from(count)) + requested.period_ns / 2)
				/ requested.period_ns)
				.min(u64::from(count)) as u16
		} else {
			0
		};
		let effective_duty = if requested.enabled {
			effective_period.saturating_mul(u64::from(compare)) / u64::from(count)
		} else {
			0
		};

		if requested.enabled && !self.pwm_states[sibling].enabled {
			registers.div().write(|register| {
				register.set_int((divider >> 4) as u8);
				register.set_frac((divider & 0x0f) as u8);
			});
			registers
				.top()
				.write(|register| register.set_top(count - 1));
		}
		registers.cc().modify(|register| {
			if channel % 2 == 0 {
				register.set_a(compare);
			} else {
				register.set_b(compare);
			}
		});
		registers.csr().modify(|register| {
			if channel % 2 == 0 {
				register.set_a_inv(requested.inverted);
			} else {
				register.set_b_inv(requested.inverted);
			}
			register.set_ph_correct(false);
			register.set_en(requested.enabled || self.pwm_states[sibling].enabled);
		});

		let applied = PwmState {
			channel: requested.channel,
			enabled: requested.enabled,
			inverted: requested.inverted,
			period_ns: effective_period,
			duty_ns: effective_duty,
		};
		self.pwm_states[channel] = applied;
		Ok(applied)
	}

	fn onboard_led_state(&self) -> Result<LedState, BusFailure> {
		Ok(self.led_state)
	}

	fn onboard_led_apply(&mut self, requested: LedState) -> Result<LedState, BusFailure> {
		let on = requested.red == 255 && requested.green == 255 && requested.blue == 255;
		if requested != LedState::OFF && !on {
			return Err(BusFailure::new(Status::Unsupported, 0));
		}
		self.led
			.set_level(if on { Level::High } else { Level::Low });
		self.led_state = requested;
		Ok(requested)
	}
}
