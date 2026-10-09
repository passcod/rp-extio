#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include "hardware/gpio.h"
#include "hardware/i2c.h"
#include "hardware/spi.h"
#include "hardware/uart.h"
#include "pico/stdlib.h"
#include "tusb.h"
#include "../linux/protocol.h"

static uint8_t request[RPE_MAX_PAYLOAD], response[RPE_MAX_PAYLOAD];
static uint8_t header[RPE_HEADER_SIZE];
static size_t received, expected = RPE_HEADER_SIZE;
static bool reading_header = true;

static uint16_t get16(const uint8_t *p) { return (uint16_t)p[0] | (uint16_t)p[1] << 8; }
static uint32_t get32(const uint8_t *p) { return (uint32_t)get16(p) | (uint32_t)get16(p + 2) << 16; }
static void put16(uint8_t *p, uint16_t v) { p[0] = v; p[1] = v >> 8; }
static void put32(uint8_t *p, uint32_t v) { put16(p, v); put16(p + 2, v >> 16); }

static void send_bytes(const uint8_t *p, size_t count) {
    while (count) {
        tud_task();
        uint32_t room = tud_vendor_write_available();
        if (!tud_mounted()) return;
        if (!room) { tud_vendor_write_flush(); continue; }
        uint32_t n = count < room ? count : room;
        n = tud_vendor_write(p, n);
        p += n; count -= n;
        tud_vendor_write_flush();
    }
}

static uint8_t execute_i2c(size_t input_len, size_t *output_len) {
    if (input_len < 1 || request[0] == 0 || request[0] > RPE_MAX_I2C_MSGS) return RPE_BAD_REQUEST;
    size_t offset = 1, out = 0;
    unsigned count = request[0];
    /* Validate the whole request before touching the bus. */
    for (unsigned i = 0; i < count; i++) {
        if (input_len - offset < 6) return RPE_BAD_REQUEST;
        uint16_t addr = get16(request + offset), flags = get16(request + offset + 2);
        uint16_t len = get16(request + offset + 4);
        offset += 6;
        if (addr > 0x7f || (flags & ~1u) || len == 0) return RPE_UNSUPPORTED;
        if (flags & 1) { if (out + len > RPE_MAX_PAYLOAD) return RPE_BAD_REQUEST; out += len; }
        else { if (input_len - offset < len) return RPE_BAD_REQUEST; offset += len; }
    }
    if (offset != input_len) return RPE_BAD_REQUEST;
    offset = 1; out = 0;
    for (unsigned i = 0; i < count; i++) {
        uint16_t addr = get16(request + offset), flags = get16(request + offset + 2);
        uint16_t len = get16(request + offset + 4);
        offset += 6;
        bool nostop = i + 1 < count;
        int result;
        if (flags & 1) {
            result = i2c_read_timeout_us(i2c0, addr, response + out, len, nostop, 500000);
            out += len;
        } else {
            result = i2c_write_timeout_us(i2c0, addr, request + offset, len, nostop, 500000);
            offset += len;
        }
        if (result != len) return RPE_IO_ERROR;
    }
    *output_len = out;
    return RPE_OK;
}

/* SPI payload: chip-select, mode, segment count, reserved; then per segment
 * speed (u32), length (u16), cs_change (u8), reserved, and transmitted bytes.
 * Every transmitted byte produces one response byte. */
static uint8_t execute_spi(size_t input_len, size_t *output_len) {
    if (input_len < 4) return RPE_BAD_REQUEST;
    uint8_t cs = request[0], mode = request[1], count = request[2];
    if (cs > 1 || mode > 3 || count == 0 || count > RPE_MAX_SPI_SEGMENTS || request[3]) return RPE_UNSUPPORTED;
    size_t offset = 4, out = 0;
    for (unsigned i = 0; i < count; i++) {
        if (input_len - offset < 8) return RPE_BAD_REQUEST;
        uint32_t speed = get32(request + offset);
        uint16_t len = get16(request + offset + 4);
        uint8_t change = request[offset + 6], reserved = request[offset + 7];
        offset += 8;
        if (!speed || speed > 20000000 || !len || change > 1 || reserved) return RPE_UNSUPPORTED;
        if (input_len - offset < len || out + len > RPE_MAX_PAYLOAD) return RPE_BAD_REQUEST;
        offset += len; out += len;
    }
    if (offset != input_len) return RPE_BAD_REQUEST;
    uint8_t pin = cs ? 21 : 17;
    offset = 4; out = 0;
    gpio_put(pin, 0);
    for (unsigned i = 0; i < count; i++) {
        uint32_t speed = get32(request + offset);
        uint16_t len = get16(request + offset + 4);
        uint8_t change = request[offset + 6];
        offset += 8;
        spi_set_baudrate(spi0, speed);
        spi_set_format(spi0, 8, (mode & 2) ? SPI_CPOL_1 : SPI_CPOL_0,
                       (mode & 1) ? SPI_CPHA_1 : SPI_CPHA_0, SPI_MSB_FIRST);
        int result = spi_write_read_blocking(spi0, request + offset, response + out, len);
        offset += len; out += len;
        if (result != len) { gpio_put(pin, 1); return RPE_IO_ERROR; }
        if (change && i + 1 < count) { gpio_put(pin, 1); busy_wait_us_32(1); gpio_put(pin, 0); }
    }
    gpio_put(pin, 1);
    *output_len = out;
    return RPE_OK;
}

static void process_request(void) {
    uint8_t reply[RPE_HEADER_SIZE] = {0};
    put32(reply, RPE_MAGIC);
    reply[RPE_VERSION_OFF] = RPE_VERSION;
    reply[RPE_COMMAND_OFF] = header[RPE_COMMAND_OFF];
    put32(reply + RPE_SEQUENCE_OFF, get32(header + RPE_SEQUENCE_OFF));
    size_t output_len = 0;
    uint8_t status = RPE_BAD_REQUEST;
    if (get32(header) == RPE_MAGIC && header[RPE_VERSION_OFF] == RPE_VERSION &&
        !header[RPE_STATUS_OFF] && !header[RPE_RESERVED_OFF] && !get16(header + RPE_AUX_OFF)) {
        switch (header[RPE_COMMAND_OFF]) {
        case RPE_CMD_I2C: status = execute_i2c(expected, &output_len); break;
        case RPE_CMD_SPI: status = execute_spi(expected, &output_len); break;
        default: status = RPE_UNSUPPORTED; break;
        }
    }
    reply[RPE_STATUS_OFF] = status;
    if (status) output_len = 0;
    put16(reply + RPE_LENGTH_OFF, output_len);
    send_bytes(reply, sizeof(reply));
    if (output_len) send_bytes(response, output_len);
}

static void service_vendor(void) {
    while (tud_vendor_available()) {
        uint8_t *dest = reading_header ? header : request;
        uint32_t n = tud_vendor_read(dest + received, expected - received);
        received += n;
        if (received != expected) continue;
        if (reading_header) {
            expected = get16(header + RPE_LENGTH_OFF);
            received = 0;
            reading_header = false;
            if (expected > RPE_MAX_PAYLOAD) {
                /* A malformed length cannot be framed safely; reset the USB stream. */
                tud_disconnect();
                sleep_ms(10);
                tud_connect();
                expected = RPE_HEADER_SIZE;
                reading_header = true;
            } else if (!expected) {
                process_request();
                expected = RPE_HEADER_SIZE;
                reading_header = true;
            }
        } else {
            process_request();
            expected = RPE_HEADER_SIZE;
            received = 0;
            reading_header = true;
        }
    }
}

void tud_cdc_line_coding_cb(uint8_t itf, cdc_line_coding_t const *coding) {
    (void)itf;
    if (coding->bit_rate < 300 || coding->bit_rate > 3000000) return;
    uart_set_baudrate(uart0, coding->bit_rate);
    uint data_bits = coding->data_bits >= 5 && coding->data_bits <= 8 ? coding->data_bits : 8;
    uart_parity_t parity = coding->parity == 1 ? UART_PARITY_ODD :
                           coding->parity == 2 ? UART_PARITY_EVEN : UART_PARITY_NONE;
    uart_set_format(uart0, data_bits, coding->stop_bits == 2 ? 2 : 1, parity);
}

static void service_uart(void) {
    while (tud_cdc_available() && uart_is_writable(uart0)) uart_putc_raw(uart0, tud_cdc_read_char());
    while (uart_is_readable(uart0) && tud_cdc_write_available()) tud_cdc_write_char(uart_getc(uart0));
    tud_cdc_write_flush();
}

int main(void) {
    i2c_init(i2c0, 400000);
    gpio_set_function(4, GPIO_FUNC_I2C); gpio_set_function(5, GPIO_FUNC_I2C);
    gpio_pull_up(4); gpio_pull_up(5);
    spi_init(spi0, 1000000);
    gpio_set_function(18, GPIO_FUNC_SPI); gpio_set_function(19, GPIO_FUNC_SPI);
    gpio_set_function(16, GPIO_FUNC_SPI);
    for (int pin = 17; pin <= 21; pin += 4) { gpio_init(pin); gpio_put(pin, 1); gpio_set_dir(pin, GPIO_OUT); }
    uart_init(uart0, 115200);
    gpio_set_function(0, GPIO_FUNC_UART); gpio_set_function(1, GPIO_FUNC_UART);
    tusb_rhport_init_t usb_init = { .role = TUSB_ROLE_DEVICE, .speed = TUSB_SPEED_AUTO };
    tusb_init(0, &usb_init);
    while (true) { tud_task(); service_vendor(); service_uart(); }
}
