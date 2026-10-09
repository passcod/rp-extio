// SPDX-License-Identifier: GPL-2.0-only
#include <linux/i2c.h>
#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/slab.h>
#include <linux/spi/spi.h>
#include <linux/usb.h>
#include "protocol.h"

#define USB_VID 0xCafe
#define USB_PID 0x2350
#define USB_TIMEOUT_MS 12000

struct rpe_bridge {
	struct usb_device *usb;
	struct usb_interface *interface;
	struct mutex lock;
	bool dead;
	u32 sequence;
	u8 ep_in, ep_out;
	u8 tx[RPE_MAX_PAYLOAD], rx[RPE_MAX_PAYLOAD];
	struct i2c_adapter i2c;
	struct spi_controller *spi;
	struct spi_device *spidev[2];
};

static u16 rpe_get16(const u8 *p) { return p[0] | (u16)p[1] << 8; }
static u32 rpe_get32(const u8 *p) { return rpe_get16(p) | (u32)rpe_get16(p + 2) << 16; }
static void rpe_put16(u8 *p, u16 v) { p[0] = v; p[1] = v >> 8; }
static void rpe_put32(u8 *p, u32 v) { rpe_put16(p, v); rpe_put16(p + 2, v >> 16); }

static int rpe_bulk(struct rpe_bridge *bridge, bool in, void *data, int len)
{
	int actual, pipe, ret;

	if (!len)
		return 0;
	pipe = in ? usb_rcvbulkpipe(bridge->usb, bridge->ep_in) :
		usb_sndbulkpipe(bridge->usb, bridge->ep_out);
	ret = usb_bulk_msg(bridge->usb, pipe, data, len, &actual, USB_TIMEOUT_MS);
	if (ret)
		return ret;
	return actual == len ? 0 : -EPROTO;
}

/* The mutex covers the entire exchange and protects the shared buffers.
 * Any transport error poisons the stream: a late reply may otherwise be
 * mistaken for the next operation. Replugging recovers the device. */
static int rpe_exchange(struct rpe_bridge *bridge, u8 command, u16 tx_len,
			 u16 expected_rx)
{
	u8 header[RPE_HEADER_SIZE] = {0};
	u32 seq;
	u16 rx_len;
	int ret;

	if (bridge->dead)
		return -ENODEV;
	seq = ++bridge->sequence;
	rpe_put32(header, RPE_MAGIC);
	header[RPE_VERSION_OFF] = RPE_VERSION;
	header[RPE_COMMAND_OFF] = command;
	rpe_put32(header + RPE_SEQUENCE_OFF, seq);
	rpe_put16(header + RPE_LENGTH_OFF, tx_len);
	ret = rpe_bulk(bridge, false, header, sizeof(header));
	if (!ret)
		ret = rpe_bulk(bridge, false, bridge->tx, tx_len);
	if (!ret)
		ret = rpe_bulk(bridge, true, header, sizeof(header));
	if (ret)
		goto poison;
	if (rpe_get32(header) != RPE_MAGIC || header[RPE_VERSION_OFF] != RPE_VERSION ||
	    header[RPE_COMMAND_OFF] != command || rpe_get32(header + RPE_SEQUENCE_OFF) != seq ||
	    header[RPE_RESERVED_OFF] || rpe_get16(header + RPE_AUX_OFF)) {
		ret = -EPROTO;
		goto poison;
	}
	rx_len = rpe_get16(header + RPE_LENGTH_OFF);
	if (rx_len > RPE_MAX_PAYLOAD || (header[RPE_STATUS_OFF] && rx_len) ||
	    (!header[RPE_STATUS_OFF] && rx_len != expected_rx)) {
		ret = -EPROTO;
		goto poison;
	}
	ret = rpe_bulk(bridge, true, bridge->rx, rx_len);
	if (ret)
		goto poison;
	switch (header[RPE_STATUS_OFF]) {
	case RPE_OK: return 0;
	case RPE_BAD_REQUEST: return -EINVAL;
	case RPE_IO_ERROR: return -EREMOTEIO;
	case RPE_UNSUPPORTED: return -EOPNOTSUPP;
	default: ret = -EPROTO; goto poison;
	}
poison:
	bridge->dead = true;
	return ret;
}

static int rpe_i2c_xfer(struct i2c_adapter *adapter, struct i2c_msg *msgs, int count)
{
	struct rpe_bridge *bridge = i2c_get_adapdata(adapter);
	size_t tx_len = 1, rx_len = 0, out = 0;
	int i, ret;

	if (count < 1 || count > RPE_MAX_I2C_MSGS)
		return -EOPNOTSUPP;
	for (i = 0; i < count; i++) {
		if (msgs[i].flags & ~I2C_M_RD || msgs[i].addr > 0x7f || !msgs[i].len)
			return -EOPNOTSUPP;
		tx_len += 6 + ((msgs[i].flags & I2C_M_RD) ? 0 : msgs[i].len);
		rx_len += (msgs[i].flags & I2C_M_RD) ? msgs[i].len : 0;
		if (tx_len > RPE_MAX_PAYLOAD || rx_len > RPE_MAX_PAYLOAD)
			return -EMSGSIZE;
	}
	mutex_lock(&bridge->lock);
	if (bridge->dead) { ret = -ENODEV; goto unlock; }
	bridge->tx[0] = count;
	tx_len = 1;
	for (i = 0; i < count; i++) {
		rpe_put16(bridge->tx + tx_len, msgs[i].addr);
		rpe_put16(bridge->tx + tx_len + 2, !!(msgs[i].flags & I2C_M_RD));
		rpe_put16(bridge->tx + tx_len + 4, msgs[i].len);
		tx_len += 6;
		if (!(msgs[i].flags & I2C_M_RD)) {
			memcpy(bridge->tx + tx_len, msgs[i].buf, msgs[i].len);
			tx_len += msgs[i].len;
		}
	}
	ret = rpe_exchange(bridge, RPE_CMD_I2C, tx_len, rx_len);
	if (ret)
		goto unlock;
	for (i = 0; i < count; i++) {
		if (msgs[i].flags & I2C_M_RD) {
			memcpy(msgs[i].buf, bridge->rx + out, msgs[i].len);
			out += msgs[i].len;
		}
	}
	ret = count;
unlock:
	mutex_unlock(&bridge->lock);
	return ret;
}

static u32 rpe_i2c_functionality(struct i2c_adapter *adapter)
{
	return I2C_FUNC_I2C;
}

static const struct i2c_algorithm rpe_i2c_algorithm = {
	.xfer = rpe_i2c_xfer,
	.functionality = rpe_i2c_functionality,
};

static int rpe_spi_message(struct spi_controller *controller, struct spi_message *message)
{
	struct rpe_bridge *bridge = *(struct rpe_bridge **)spi_controller_get_devdata(controller);
	struct spi_transfer *transfer;
	size_t tx_len = 4, rx_len = 0, out = 0;
	u8 segments = 0;
	int ret = 0;

	list_for_each_entry(transfer, &message->transfers, transfer_list) {
		if (!transfer->len || transfer->len > RPE_MAX_PAYLOAD ||
		    transfer->bits_per_word != 8 || transfer->delay.value ||
		    transfer->cs_change_delay.value || transfer->word_delay.value) {
			ret = -EOPNOTSUPP; goto done;
		}
		tx_len += 8 + transfer->len;
		rx_len += transfer->len;
		if (++segments > RPE_MAX_SPI_SEGMENTS || tx_len > RPE_MAX_PAYLOAD ||
		    rx_len > RPE_MAX_PAYLOAD) {
			ret = -EMSGSIZE; goto done;
		}
	}
	if (!segments) goto done;
	mutex_lock(&bridge->lock);
	if (bridge->dead) { ret = -ENODEV; goto unlock; }
	bridge->tx[0] = spi_get_chipselect(message->spi, 0);
	bridge->tx[1] = message->spi->mode & (SPI_CPOL | SPI_CPHA);
	bridge->tx[2] = segments;
	bridge->tx[3] = 0;
	tx_len = 4;
	list_for_each_entry(transfer, &message->transfers, transfer_list) {
		u32 speed = transfer->speed_hz ?: message->spi->max_speed_hz;
		rpe_put32(bridge->tx + tx_len, speed);
		rpe_put16(bridge->tx + tx_len + 4, transfer->len);
		bridge->tx[tx_len + 6] = !!transfer->cs_change;
		bridge->tx[tx_len + 7] = 0;
		tx_len += 8;
		if (transfer->tx_buf)
			memcpy(bridge->tx + tx_len, transfer->tx_buf, transfer->len);
		else
			memset(bridge->tx + tx_len, 0, transfer->len);
		tx_len += transfer->len;
	}
	ret = rpe_exchange(bridge, RPE_CMD_SPI, tx_len, rx_len);
	if (ret)
		goto unlock;
	list_for_each_entry(transfer, &message->transfers, transfer_list) {
		if (transfer->rx_buf)
			memcpy(transfer->rx_buf, bridge->rx + out, transfer->len);
		out += transfer->len;
	}
	message->actual_length = out;
unlock:
	mutex_unlock(&bridge->lock);
done:
	message->status = ret;
	spi_finalize_current_message(controller);
	return 0;
}

static int rpe_probe(struct usb_interface *interface, const struct usb_device_id *id)
{
	struct usb_host_interface *alt = interface->cur_altsetting;
	struct rpe_bridge *bridge;
	struct spi_controller *spi;
	int i, ret;

	if (alt->desc.bInterfaceClass != USB_CLASS_VENDOR_SPEC ||
	    alt->desc.bInterfaceNumber != 2)
		return -ENODEV;
	bridge = kzalloc(sizeof(*bridge), GFP_KERNEL);
	if (!bridge)
		return -ENOMEM;
	bridge->usb = usb_get_dev(interface_to_usbdev(interface));
	bridge->interface = interface;
	mutex_init(&bridge->lock);
	for (i = 0; i < alt->desc.bNumEndpoints; i++) {
		struct usb_endpoint_descriptor *ep = &alt->endpoint[i].desc;
		if (usb_endpoint_is_bulk_in(ep)) bridge->ep_in = ep->bEndpointAddress;
		if (usb_endpoint_is_bulk_out(ep)) bridge->ep_out = ep->bEndpointAddress;
	}
	if (!bridge->ep_in || !bridge->ep_out) { ret = -ENODEV; goto free_bridge; }
	strscpy(bridge->i2c.name, "Pico 2 USB I2C", sizeof(bridge->i2c.name));
	bridge->i2c.owner = THIS_MODULE;
	bridge->i2c.algo = &rpe_i2c_algorithm;
	bridge->i2c.dev.parent = &interface->dev;
	i2c_set_adapdata(&bridge->i2c, bridge);
	ret = i2c_add_adapter(&bridge->i2c);
	if (ret) goto free_bridge;
	spi = spi_alloc_host(&interface->dev, sizeof(struct rpe_bridge *));
	if (!spi) { ret = -ENOMEM; goto del_i2c; }
	*(struct rpe_bridge **)spi_controller_get_devdata(spi) = bridge;
	spi->num_chipselect = 2;
	spi->mode_bits = SPI_CPOL | SPI_CPHA;
	spi->bits_per_word_mask = SPI_BPW_MASK(8);
	spi->min_speed_hz = 50000;
	spi->max_speed_hz = 20000000;
	spi->transfer_one_message = rpe_spi_message;
	bridge->spi = spi;
	ret = spi_register_controller(spi);
	if (ret) goto put_spi;
	for (i = 0; i < ARRAY_SIZE(bridge->spidev); i++) {
		struct spi_board_info info = {
			.modalias = "spidev",
			.max_speed_hz = 20000000,
			.chip_select = i,
		};

		bridge->spidev[i] = spi_new_device(spi, &info);
		if (!bridge->spidev[i]) { ret = -ENODEV; goto unregister_spi; }
	}
	usb_set_intfdata(interface, bridge);
	dev_info(&interface->dev, "I2C bus %d, SPI bus %d, UART via cdc_acm\n",
		 bridge->i2c.nr, spi->bus_num);
	return 0;
unregister_spi:
	while (i--)
		spi_unregister_device(bridge->spidev[i]);
	spi_unregister_controller(spi);
	goto del_i2c;
put_spi:
	spi_controller_put(spi);
del_i2c:
	i2c_del_adapter(&bridge->i2c);
free_bridge:
	usb_put_dev(bridge->usb);
	kfree(bridge);
	return ret;
}

static void rpe_disconnect(struct usb_interface *interface)
{
	struct rpe_bridge *bridge = usb_get_intfdata(interface);

	usb_set_intfdata(interface, NULL);
	mutex_lock(&bridge->lock);
	bridge->dead = true;
	mutex_unlock(&bridge->lock);
	spi_unregister_device(bridge->spidev[1]);
	spi_unregister_device(bridge->spidev[0]);
	spi_unregister_controller(bridge->spi);
	i2c_del_adapter(&bridge->i2c);
	usb_put_dev(bridge->usb);
	kfree(bridge);
}

static const struct usb_device_id rpe_ids[] = {
	{ USB_DEVICE_INTERFACE_NUMBER(USB_VID, USB_PID, 2) },
	{ }
};
MODULE_DEVICE_TABLE(usb, rpe_ids);

static struct usb_driver rpe_driver = {
	.name = "rp_extio",
	.probe = rpe_probe,
	.disconnect = rpe_disconnect,
	.id_table = rpe_ids,
};
module_usb_driver(rpe_driver);

MODULE_AUTHOR("RP extio contributors");
MODULE_DESCRIPTION("Pico 2 USB I2C/SPI bridge");
MODULE_LICENSE("GPL");
