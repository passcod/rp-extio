#include "tusb.h"
#include "pico/unique_id.h"

#define USB_VID 0xCafe
#define USB_PID 0x2350

enum { ITF_CDC, ITF_CDC_DATA, ITF_VENDOR, ITF_COUNT };
#define CONFIG_TOTAL_LEN (TUD_CONFIG_DESC_LEN + TUD_CDC_DESC_LEN + TUD_VENDOR_DESC_LEN)

static const tusb_desc_device_t device_desc = {
    .bLength = sizeof(tusb_desc_device_t), .bDescriptorType = TUSB_DESC_DEVICE,
    .bcdUSB = 0x0200, .bDeviceClass = TUSB_CLASS_MISC,
    .bDeviceSubClass = MISC_SUBCLASS_COMMON, .bDeviceProtocol = MISC_PROTOCOL_IAD,
    .bMaxPacketSize0 = CFG_TUD_ENDPOINT0_SIZE, .idVendor = USB_VID,
    .idProduct = USB_PID, .bcdDevice = 0x0100,
    .iManufacturer = 1, .iProduct = 2, .iSerialNumber = 3, .bNumConfigurations = 1,
};

const uint8_t *tud_descriptor_device_cb(void) { return (const uint8_t *)&device_desc; }

static const uint8_t config_desc[] = {
    TUD_CONFIG_DESCRIPTOR(1, ITF_COUNT, 0, CONFIG_TOTAL_LEN, 0, 100),
    TUD_CDC_DESCRIPTOR(ITF_CDC, 4, 0x81, 8, 0x02, 0x82, 64),
    TUD_VENDOR_DESCRIPTOR(ITF_VENDOR, 5, 0x03, 0x83, 64),
};
const uint8_t *tud_descriptor_configuration_cb(uint8_t index) {
    (void)index;
    return config_desc;
}

static const char *strings[] = { "", "RP extio", "Pico 2 USB buses", NULL, "UART", "I2C SPI" };
static uint16_t string_desc[32];
const uint16_t *tud_descriptor_string_cb(uint8_t index, uint16_t langid) {
    (void)langid;
    if (index == 0) { string_desc[1] = 0x0409; string_desc[0] = (TUSB_DESC_STRING << 8) | 4; return string_desc; }
    if (index >= sizeof(strings) / sizeof(strings[0])) return NULL;
    char serial[PICO_UNIQUE_BOARD_ID_SIZE_BYTES * 2 + 1];
    if (index == 3) pico_get_unique_board_id_string(serial, sizeof(serial));
    const char *value = index == 3 ? serial : strings[index];
    unsigned n = 0;
    while (value[n] && n < 31) { string_desc[n + 1] = value[n]; n++; }
    string_desc[0] = (TUSB_DESC_STRING << 8) | (2 * n + 2);
    return string_desc;
}
