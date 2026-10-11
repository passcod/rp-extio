use embassy_rp::gpio::{Flex, Level, Pull};
use embassy_rp::peripherals::{SPI0, UART0};
use embassy_rp::{Peri, pac};
use rp_extio_firmware::{
    Board, BusFailure, SPI_CPHA, SPI_CPOL, SPI_CS_HIGH, SPI_LSB_FIRST, SPI_MAX_SPEED_HZ,
    SPI_MIN_SPEED_HZ, SPI_NO_CS, SpiOutcome, UART_BAUD, cumulative_spi_delay_ns,
};
use rp_extio_wire::{
    GPIO_DIRECTION_INPUT, GPIO_DIRECTION_OUTPUT, GPIO_GET, GPIO_GET_DIRECTION, GPIO_SET,
    HEADER_PINS, I2C_READ, I2C_TEN_BIT, I2cRequest, PinMap, PinValidationError, SpiRequest, Status,
};

const TRANSACTION_DEADLINE_US: u64 = 5_000_000;
const I2C_HALF_PERIOD_US: u64 = 5;
const CLOCK_STRETCH_US: u64 = 5_000_000;

pub struct Pico2Board<'d> {
    pins: [Flex<'d>; HEADER_PINS.len()],
    _spi0: Peri<'d, SPI0>,
    _uart0: Peri<'d, UART0>,
    cs_active_high: [bool; 2],
    gpio_outputs: u32,
    pin_map: PinMap,
    generation: u64,
}

impl<'d> Pico2Board<'d> {
    pub fn new(
        mut pins: [Flex<'d>; HEADER_PINS.len()],
        spi0: Peri<'d, SPI0>,
        uart0: Peri<'d, UART0>,
        pin_map: PinMap,
    ) -> Self {
        for pin in &mut pins {
            pin.set_as_input();
            pin.set_pull(Pull::None);
        }
        let mut board = Self {
            pins,
            _spi0: spi0,
            _uart0: uart0,
            cs_active_high: [false; 2],
            gpio_outputs: 0,
            pin_map,
            generation: 0,
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
        self.release(self.pin_map.i2c_sda);
        self.release(self.pin_map.i2c_scl);
        for index in 0..self.pin_map.spi_chip_selects.len() {
            let level = if self.cs_active_high[index] {
                Level::Low
            } else {
                Level::High
            };
            self.pin_mut(self.pin_map.spi_chip_selects[index])
                .set_level(level);
        }
    }

    fn wait_scl_high(&self, transaction_deadline: u64) -> Result<(), BusFailure> {
        let stretch_deadline = Self::now_us().saturating_add(CLOCK_STRETCH_US);
        while self.pin(self.pin_map.i2c_scl).is_low() {
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

    fn clock_high(&mut self, deadline: u64) -> Result<(), BusFailure> {
        self.release(self.pin_map.i2c_scl);
        self.wait_scl_high(deadline)?;
        Self::delay_us(I2C_HALF_PERIOD_US)
    }

    fn clock_low(&mut self) -> Result<(), BusFailure> {
        self.drive_low(self.pin_map.i2c_scl);
        Self::delay_us(I2C_HALF_PERIOD_US)
    }

    fn recover_i2c(&mut self, deadline: u64) -> Result<(), BusFailure> {
        self.release(self.pin_map.i2c_sda);
        self.release(self.pin_map.i2c_scl);
        self.wait_scl_high(deadline)?;
        if self.pin(self.pin_map.i2c_sda).is_high() {
            return Ok(());
        }
        for _ in 0..9 {
            self.clock_low()?;
            self.clock_high(deadline)?;
            if self.pin(self.pin_map.i2c_sda).is_high() {
                break;
            }
        }
        self.drive_low(self.pin_map.i2c_sda);
        Self::delay_us(I2C_HALF_PERIOD_US)?;
        self.clock_high(deadline)?;
        self.release(self.pin_map.i2c_sda);
        Self::delay_us(I2C_HALF_PERIOD_US)?;
        if self.pin(self.pin_map.i2c_sda).is_low() {
            Err(BusFailure::new(Status::Timeout, 0))
        } else {
            Ok(())
        }
    }

    fn i2c_start(&mut self, deadline: u64) -> Result<(), BusFailure> {
        self.release(self.pin_map.i2c_sda);
        self.clock_high(deadline)?;
        if self.pin(self.pin_map.i2c_sda).is_low() {
            return Err(BusFailure::new(Status::ArbitrationLost, 0));
        }
        self.drive_low(self.pin_map.i2c_sda);
        Self::delay_us(I2C_HALF_PERIOD_US)?;
        self.clock_low()
    }

    fn i2c_stop(&mut self, deadline: u64) -> Result<(), BusFailure> {
        self.drive_low(self.pin_map.i2c_sda);
        Self::delay_us(I2C_HALF_PERIOD_US)?;
        self.clock_high(deadline)?;
        self.release(self.pin_map.i2c_sda);
        Self::delay_us(I2C_HALF_PERIOD_US)
    }

    fn i2c_write_byte(&mut self, byte: u8, deadline: u64) -> Result<bool, BusFailure> {
        for bit in (0..8).rev() {
            let released = byte & (1 << bit) != 0;
            if released {
                self.release(self.pin_map.i2c_sda);
            } else {
                self.drive_low(self.pin_map.i2c_sda);
            }
            self.clock_high(deadline)?;
            if released && self.pin(self.pin_map.i2c_sda).is_low() {
                self.drive_low(self.pin_map.i2c_scl);
                self.release(self.pin_map.i2c_sda);
                return Err(BusFailure::new(Status::ArbitrationLost, 0));
            }
            self.clock_low()?;
        }
        self.release(self.pin_map.i2c_sda);
        self.clock_high(deadline)?;
        let acknowledged = self.pin(self.pin_map.i2c_sda).is_low();
        self.clock_low()?;
        Ok(acknowledged)
    }

    fn i2c_read_byte(&mut self, acknowledge: bool, deadline: u64) -> Result<u8, BusFailure> {
        self.release(self.pin_map.i2c_sda);
        let mut byte = 0;
        for _ in 0..8 {
            self.clock_high(deadline)?;
            byte = (byte << 1) | u8::from(self.pin(self.pin_map.i2c_sda).is_high());
            self.clock_low()?;
        }
        if acknowledge {
            self.drive_low(self.pin_map.i2c_sda);
        } else {
            self.release(self.pin_map.i2c_sda);
        }
        self.clock_high(deadline)?;
        self.clock_low()?;
        self.release(self.pin_map.i2c_sda);
        Ok(byte)
    }

    fn address(
        &mut self,
        address: u16,
        ten_bit: bool,
        read: bool,
        deadline: u64,
    ) -> Result<bool, BusFailure> {
        if !ten_bit {
            return self.i2c_write_byte(((address as u8) << 1) | u8::from(read), deadline);
        }

        let prefix = 0b1111_0000 | (((address >> 7) as u8) & 0b110);
        if !self.i2c_write_byte(prefix, deadline)?
            || !self.i2c_write_byte(address as u8, deadline)?
        {
            return Ok(false);
        }
        if read {
            self.i2c_start(deadline)?;
            self.i2c_write_byte(prefix | 1, deadline)
        } else {
            Ok(true)
        }
    }

    fn set_cs(&mut self, index: u8, active_high: bool, asserted: bool) {
        let high = asserted == active_high;
        let pin = self.pin_map.spi_chip_selects[index as usize];
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
        pac::UART0.uartcr().write(|_| {});
        for (number, pin) in HEADER_PINS.into_iter().zip(&mut self.pins) {
            pin.set_as_input();
            pin.set_pull(Pull::None);
            Self::set_function(number, 5);
        }
    }

    fn configure_uart() {
        let clock = embassy_rp::clocks::clk_peri_freq();
        let divisor = (8 * clock) / UART_BAUD;
        let integer = (divisor >> 7).clamp(1, 65_535);
        let fraction = if integer == 65_535 {
            0
        } else {
            ((divisor & 0x7f) + 1) / 2
        };
        let uart = pac::UART0;
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

    fn configure_spi(speed_hz: u32, phase: bool, polarity: bool) -> u32 {
        let peripheral = embassy_rp::clocks::clk_peri_freq();
        let ratio = peripheral.div_ceil(speed_hz.saturating_mul(2));
        let prescaler = ratio.div_ceil(256).max(1);
        let postdivider = if prescaler == 1 {
            ratio
        } else {
            ratio.div_ceil(prescaler)
        }
        .clamp(1, 256);
        let spi = pac::SPI0;
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
        self.cs_active_high = [false; 2];

        self.release(map.i2c_sda);
        self.release(map.i2c_scl);
        for chip_select in map.spi_chip_selects {
            self.set_output(chip_select, Level::High);
        }
        for pin in [map.spi_miso, map.spi_sck, map.spi_mosi] {
            Self::set_function(pin, 1);
        }
        Self::configure_spi(SPI_MIN_SPEED_HZ, false, false);

        let tx_function = if map.uart_tx % 4 == 0 { 2 } else { 11 };
        let rx_function = if (map.uart_rx - 1) % 4 == 0 { 2 } else { 11 };
        Self::set_function(map.uart_tx, tx_function);
        Self::set_function(map.uart_rx, rx_function);
        Self::configure_uart();
    }

    fn spi_transfer(byte: u8) -> u8 {
        let spi = pac::SPI0;
        while !spi.sr().read().tnf() {}
        spi.dr().write(|register| register.set_data(byte as u16));
        while !spi.sr().read().rne() {}
        spi.dr().read().data() as u8
    }

    pub fn uart_tx_full() -> bool {
        pac::UART0.uartfr().read().txff()
    }

    pub fn uart_write(byte: u8) {
        pac::UART0
            .uartdr()
            .write(|register| register.set_data(byte));
    }

    pub fn uart_read(buffer: &mut [u8]) -> usize {
        let uart = pac::UART0;
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
    fn pin_map(&self) -> &PinMap {
        &self.pin_map
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn configure_pins(&mut self, map: PinMap, generation: u64) -> Result<(), PinValidationError> {
        map.validate()?;
        self.apply_pin_map(map);
        self.generation = generation;
        Ok(())
    }

    fn i2c_transfer(
        &mut self,
        request: &I2cRequest<'_>,
        reads: &mut [u8],
    ) -> Result<usize, BusFailure> {
        let result = (|| {
            let deadline = Self::now_us().saturating_add(TRANSACTION_DEADLINE_US);
            self.recover_i2c(deadline)?;
            let mut read_offset = 0;

            for (completed, message) in request.messages().enumerate() {
                let completed = completed as u32;
                let message = message.map_err(|_| BusFailure::new(Status::BadFrame, completed))?;
                let result = (|| {
                    self.i2c_start(deadline)?;
                    let read = message.flags & I2C_READ != 0;
                    if !self.address(
                        message.address,
                        message.flags & I2C_TEN_BIT != 0,
                        read,
                        deadline,
                    )? {
                        return Err(BusFailure::new(Status::Nack, completed));
                    }
                    if read {
                        for index in 0..message.len as usize {
                            reads[read_offset] =
                                self.i2c_read_byte(index + 1 < message.len as usize, deadline)?;
                            read_offset += 1;
                        }
                    } else {
                        for &byte in message.write_data {
                            if !self.i2c_write_byte(byte, deadline)? {
                                return Err(BusFailure::new(Status::Nack, completed));
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(mut failure) = result {
                    failure.completed = completed;
                    if failure.status != Status::ArbitrationLost {
                        let _ = self.i2c_stop(deadline);
                    }
                    return Err(failure);
                }
            }
            self.i2c_stop(deadline)?;
            Ok(read_offset)
        })();

        // A failed STOP can return while SDA is still asserted, particularly when SCL is held
        // low. Never allow one failed transaction to leave either open-drain output enabled.
        self.release(self.pin_map.i2c_sda);
        self.release(self.pin_map.i2c_scl);
        result
    }

    fn spi_message(
        &mut self,
        request: &SpiRequest<'_>,
        receive: &mut [u8],
    ) -> Result<SpiOutcome, BusFailure> {
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
            self.cs_active_high[request.chip_select as usize] = active_high;
            self.set_cs(request.chip_select, active_high, false);
            self.set_cs(request.chip_select, active_high, true);
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
                    *target = Self::spi_transfer(*target);
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
                    self.set_cs(request.chip_select, active_high, false);
                    Self::delay_ns(segment.cs_change_delay_ns)?;
                    self.set_cs(request.chip_select, active_high, true);
                }
            }
            Ok(outcome)
        })();
        if !no_cs {
            self.set_cs(request.chip_select, active_high, false);
        }
        result
    }

    fn disconnect(&mut self) {
        self.release_buses();
    }

    fn gpio_command(&mut self, command: u8, line: u8, value: u8) -> Result<u8, BusFailure> {
        let index = line as usize;
        let Ok(gpio_pins) = self.pin_map.gpio_pins() else {
            return Err(BusFailure::new(Status::IoError, 0));
        };
        let Some(&number) = gpio_pins.get(index) else {
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
            _ => Err(BusFailure::new(Status::Unsupported, 0)),
        }
    }

    fn gpio_snapshot(&mut self) -> Result<u32, BusFailure> {
        let gpio_pins = self
            .pin_map
            .gpio_pins()
            .map_err(|_| BusFailure::new(Status::IoError, 0))?;
        let mut values = 0;
        for (index, number) in gpio_pins.into_iter().enumerate() {
            if self.pin(number).is_high() {
                values |= 1 << index;
            }
        }
        Ok(values)
    }
}
