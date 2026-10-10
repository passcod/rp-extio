use embassy_rp::gpio::{Flex, Level, Output, OutputOpenDrain, Pin};
use embassy_rp::peripherals::SPI0;
use embassy_rp::spi::{Blocking, ClkPin, Config, MisoPin, MosiPin, Phase, Polarity, Spi};
use embassy_rp::{Peri, pac};
use rp_extio_firmware::{
    Board, BusFailure, GPIO_LINE_COUNT, PinConfig, SPI_CPHA, SPI_CPOL, SPI_CS_HIGH, SPI_LSB_FIRST,
    SPI_MAX_SPEED_HZ, SPI_MIN_SPEED_HZ, SPI_NO_CS, SpiOutcome, cumulative_spi_delay_ns,
};
use rp_extio_wire::{
    GPIO_DIRECTION_INPUT, GPIO_DIRECTION_OUTPUT, GPIO_GET, GPIO_GET_DIRECTION, GPIO_SET, I2C_READ,
    I2C_TEN_BIT, I2cRequest, SpiRequest, Status,
};

const TRANSACTION_DEADLINE_US: u64 = 5_000_000;
const I2C_HALF_PERIOD_US: u64 = 5;
const CLOCK_STRETCH_US: u64 = 5_000_000;

pub struct Pico2Board<'d> {
    sda: OutputOpenDrain<'d>,
    scl: OutputOpenDrain<'d>,
    spi: Spi<'d, SPI0, Blocking>,
    chip_selects: [Output<'d>; 2],
    cs_active_high: [bool; 2],
    gpio: [Flex<'d>; GPIO_LINE_COUNT],
    gpio_outputs: u32,
    pin_config: &'static PinConfig,
}

impl<'d> Pico2Board<'d> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sda: Peri<'d, impl Pin>,
        scl: Peri<'d, impl Pin>,
        miso: Peri<'d, impl MisoPin<SPI0>>,
        cs0: Peri<'d, impl Pin>,
        sck: Peri<'d, impl ClkPin<SPI0>>,
        mosi: Peri<'d, impl MosiPin<SPI0>>,
        cs1: Peri<'d, impl Pin>,
        spi0: Peri<'d, SPI0>,
        mut gpio: [Flex<'d>; GPIO_LINE_COUNT],
        pin_config: &'static PinConfig,
    ) -> Self {
        let mut sda = OutputOpenDrain::new(sda, Level::High);
        let mut scl = OutputOpenDrain::new(scl, Level::High);
        sda.set_pullup(false);
        scl.set_pullup(false);

        let spi = Spi::new_blocking(spi0, sck, mosi, miso, Config::default());
        let chip_selects = [Output::new(cs0, Level::High), Output::new(cs1, Level::High)];
        for pin in &mut gpio {
            pin.set_as_input();
        }
        Self {
            sda,
            scl,
            spi,
            chip_selects,
            cs_active_high: [false; 2],
            gpio,
            gpio_outputs: 0,
            pin_config,
        }
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
        self.sda.set_high();
        self.scl.set_high();
        for (cs, active_high) in self.chip_selects.iter_mut().zip(self.cs_active_high) {
            if active_high {
                cs.set_low();
            } else {
                cs.set_high();
            }
        }
    }

    fn wait_scl_high(&self, transaction_deadline: u64) -> Result<(), BusFailure> {
        let stretch_deadline = Self::now_us().saturating_add(CLOCK_STRETCH_US);
        while self.scl.is_low() {
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
        self.scl.set_high();
        self.wait_scl_high(deadline)?;
        Self::delay_us(I2C_HALF_PERIOD_US)
    }

    fn clock_low(&mut self) -> Result<(), BusFailure> {
        self.scl.set_low();
        Self::delay_us(I2C_HALF_PERIOD_US)
    }

    fn recover_i2c(&mut self, deadline: u64) -> Result<(), BusFailure> {
        self.sda.set_high();
        self.scl.set_high();
        self.wait_scl_high(deadline)?;
        if self.sda.is_high() {
            return Ok(());
        }
        for _ in 0..9 {
            self.clock_low()?;
            self.clock_high(deadline)?;
            if self.sda.is_high() {
                break;
            }
        }
        self.sda.set_low();
        Self::delay_us(I2C_HALF_PERIOD_US)?;
        self.clock_high(deadline)?;
        self.sda.set_high();
        Self::delay_us(I2C_HALF_PERIOD_US)?;
        if self.sda.is_low() {
            Err(BusFailure::new(Status::Timeout, 0))
        } else {
            Ok(())
        }
    }

    fn i2c_start(&mut self, deadline: u64) -> Result<(), BusFailure> {
        self.sda.set_high();
        self.clock_high(deadline)?;
        if self.sda.is_low() {
            return Err(BusFailure::new(Status::ArbitrationLost, 0));
        }
        self.sda.set_low();
        Self::delay_us(I2C_HALF_PERIOD_US)?;
        self.clock_low()
    }

    fn i2c_stop(&mut self, deadline: u64) -> Result<(), BusFailure> {
        self.sda.set_low();
        Self::delay_us(I2C_HALF_PERIOD_US)?;
        self.clock_high(deadline)?;
        self.sda.set_high();
        Self::delay_us(I2C_HALF_PERIOD_US)
    }

    fn i2c_write_byte(&mut self, byte: u8, deadline: u64) -> Result<bool, BusFailure> {
        for bit in (0..8).rev() {
            let released = byte & (1 << bit) != 0;
            if released {
                self.sda.set_high();
            } else {
                self.sda.set_low();
            }
            self.clock_high(deadline)?;
            if released && self.sda.is_low() {
                self.scl.set_low();
                self.sda.set_high();
                return Err(BusFailure::new(Status::ArbitrationLost, 0));
            }
            self.clock_low()?;
        }
        self.sda.set_high();
        self.clock_high(deadline)?;
        let acknowledged = self.sda.is_low();
        self.clock_low()?;
        Ok(acknowledged)
    }

    fn i2c_read_byte(&mut self, acknowledge: bool, deadline: u64) -> Result<u8, BusFailure> {
        self.sda.set_high();
        let mut byte = 0;
        for _ in 0..8 {
            self.clock_high(deadline)?;
            byte = (byte << 1) | u8::from(self.sda.is_high());
            self.clock_low()?;
        }
        if acknowledge {
            self.sda.set_low();
        } else {
            self.sda.set_high();
        }
        self.clock_high(deadline)?;
        self.clock_low()?;
        self.sda.set_high();
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
        if high {
            self.chip_selects[index as usize].set_high();
        } else {
            self.chip_selects[index as usize].set_low();
        }
    }

    fn effective_spi_speed(requested: u32) -> u32 {
        let peripheral = embassy_rp::clocks::clk_peri_freq();
        let ratio = peripheral.div_ceil(requested.saturating_mul(2));
        let prescaler = ratio.div_ceil(256).max(1);
        let postdivider = if prescaler == 1 {
            ratio
        } else {
            ratio.div_ceil(prescaler)
        }
        .clamp(1, 256);
        peripheral / (prescaler * 2) / postdivider
    }
}

impl Board for Pico2Board<'_> {
    fn pin_config(&self) -> &PinConfig {
        self.pin_config
    }

    fn i2c_transfer(
        &mut self,
        request: &I2cRequest<'_>,
        reads: &mut [u8],
    ) -> Result<usize, BusFailure> {
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
                } else {
                    self.sda.set_high();
                    self.scl.set_high();
                }
                return Err(failure);
            }
        }
        self.i2c_stop(deadline)?;
        Ok(read_offset)
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
                let mut config = Config::default();
                config.frequency = segment.speed_hz;
                config.phase = if request.mode & SPI_CPHA != 0 {
                    Phase::CaptureOnSecondTransition
                } else {
                    Phase::CaptureOnFirstTransition
                };
                config.polarity = if request.mode & SPI_CPOL != 0 {
                    Polarity::IdleHigh
                } else {
                    Polarity::IdleLow
                };
                self.spi.set_config(&config);
                outcome.effective_speeds[index] = Self::effective_spi_speed(segment.speed_hz);
                let target = &mut receive[offset..offset + segment.len as usize];
                for (byte_index, (target, &source)) in target.iter_mut().zip(segment.tx).enumerate()
                {
                    *target = if lsb_first {
                        source.reverse_bits()
                    } else {
                        source
                    };
                    self.spi
                        .blocking_transfer_in_place(core::slice::from_mut(target))
                        .map_err(|_| BusFailure::new(Status::IoError, outcome.completed_bytes))?;
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
        let Some(pin) = self.gpio.get_mut(index) else {
            return Err(BusFailure::new(Status::Unsupported, 0));
        };
        let mask = 1_u32 << index;
        match command {
            GPIO_GET => Ok(u8::from(pin.is_high())),
            GPIO_SET => {
                if self.gpio_outputs & mask == 0 {
                    return Err(BusFailure::new(Status::Unsupported, 0));
                }
                pin.set_level(if value == 0 { Level::Low } else { Level::High });
                Ok(value)
            }
            GPIO_DIRECTION_INPUT => {
                pin.set_as_input();
                self.gpio_outputs &= !mask;
                Ok(1)
            }
            GPIO_DIRECTION_OUTPUT => {
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
        let mut values = 0;
        for (index, pin) in self.gpio.iter().enumerate() {
            if pin.is_high() {
                values |= 1 << index;
            }
        }
        Ok(values)
    }
}
