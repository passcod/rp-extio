use core::sync::atomic::{AtomicU8, Ordering};
use embassy_rp::gpio::{Flex, Level, Pull};
use embassy_rp::peripherals::{SPI0, SPI1, UART0, UART1};
use embassy_rp::{Peri, pac};
use rp_extio_firmware::{
    Board, BusFailure, SPI_CPHA, SPI_CPOL, SPI_CS_HIGH, SPI_LSB_FIRST, SPI_MAX_SPEED_HZ,
    SPI_MIN_SPEED_HZ, SPI_NO_CS, SpiOutcome, UART_BAUD, cumulative_spi_delay_ns,
};
use rp_extio_wire::{
    GPIO_DIRECTION_INPUT, GPIO_DIRECTION_OUTPUT, GPIO_GET, GPIO_GET_DIRECTION, GPIO_SET,
    HEADER_PINS, I2C_READ, I2C_TEN_BIT, I2cRequest, Peripheral, PinMap, PinValidationError,
    SpiRequest, Status,
};

const TRANSACTION_DEADLINE_US: u64 = 5_000_000;
const I2C_HALF_PERIOD_US: u64 = 5;
const CLOCK_STRETCH_US: u64 = 5_000_000;
static UART_ENABLED: AtomicU8 = AtomicU8::new(0);

pub struct Pico2Board<'d> {
    pins: [Flex<'d>; HEADER_PINS.len()],
    _spi0: Peri<'d, SPI0>,
    _spi1: Peri<'d, SPI1>,
    _uart0: Peri<'d, UART0>,
    _uart1: Peri<'d, UART1>,
    cs_active_high: [[bool; 2]; 2],
    gpio_outputs: u32,
    pin_map: PinMap,
    generation: u64,
}

impl<'d> Pico2Board<'d> {
    pub fn new(
        mut pins: [Flex<'d>; HEADER_PINS.len()],
        spi0: Peri<'d, SPI0>,
        spi1: Peri<'d, SPI1>,
        uart0: Peri<'d, UART0>,
        uart1: Peri<'d, UART1>,
        pin_map: PinMap,
    ) -> Self {
        for pin in &mut pins {
            pin.set_as_input();
            pin.set_pull(Pull::None);
        }
        let mut board = Self {
            pins,
            _spi0: spi0,
            _spi1: spi1,
            _uart0: uart0,
            _uart1: uart1,
            cs_active_high: [[false; 2]; 2],
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
        for (number, pin) in HEADER_PINS.into_iter().zip(&mut self.pins) {
            pin.set_as_input();
            pin.set_pull(Pull::None);
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
        UART_ENABLED.store(uart_enabled, Ordering::Release);
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
        self.release_buses();
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
}
