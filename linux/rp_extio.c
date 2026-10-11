// SPDX-License-Identifier: GPL-2.0-only
#include <linux/completion.h>
#include <linux/bitmap.h>
#include <linux/build_bug.h>
#include <linux/device.h>
#include <linux/fs.h>
#include <linux/gpio/driver.h>
#include <linux/i2c.h>
#include <linux/iio/iio.h>
#include <linux/idr.h>
#include <linux/jiffies.h>
#include <linux/kmod.h>
#include <linux/list.h>
#include <linux/miscdevice.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/irq.h>
#include <linux/irqdomain.h>
#include <linux/poll.h>
#include <linux/pwm.h>
#include <linux/pinctrl/pinconf-generic.h>
#include <linux/slab.h>
#include <linux/spi/spi.h>
#include <linux/string.h>
#include <linux/uaccess.h>
#include <linux/unaligned.h>
#include <linux/version.h>

#include "rp_extio_uapi.h"

#define RPE_MAGIC 0x444b5052U /* "RPKD" in little endian */
#define RPE_HEADER_SIZE 32
#define RPE_TIMEOUT (8 * HZ)

#define RPE_OP_I2C 2
#define RPE_OP_SPI 3
#define RPE_OP_GPIO 4
#define RPE_OP_GPIO_EVENT 5
#define RPE_OP_ADC 6
#define RPE_OP_PWM 7

#define RPE_GPIO_GET 1
#define RPE_GPIO_SET 2
#define RPE_GPIO_DIRECTION_INPUT 3
#define RPE_GPIO_DIRECTION_OUTPUT 4
#define RPE_GPIO_GET_DIRECTION 5
#define RPE_GPIO_SNAPSHOT 6
#define RPE_GPIO_SET_BIAS 7
#define RPE_GPIO_BIAS_NONE 0
#define RPE_GPIO_BIAS_PULL_UP 1
#define RPE_GPIO_BIAS_PULL_DOWN 2
#define RPE_MAX_GPIO_LINES 26

#define RPE_PWM_GET 1
#define RPE_PWM_APPLY 2
#define RPE_PWM_ENABLED BIT(0)
#define RPE_PWM_INVERTED BIT(1)
#define RPE_PWM_STATE_SIZE 20

#define RPE_STATUS_OK 0
#define RPE_STATUS_NACK 1
#define RPE_STATUS_ARBITRATION_LOST 2
#define RPE_STATUS_TIMEOUT 3
#define RPE_STATUS_DISCONNECTED 4
#define RPE_STATUS_UNSUPPORTED 5
#define RPE_STATUS_BAD_FRAME 6
#define RPE_STATUS_IO_ERROR 7
#define RPE_STATUS_INVALID_ROUTE 8
#define RPE_STATUS_PIN_CONFLICT 9
#define RPE_STATUS_BUSY 10

#define RPE_I2C_READ 1
#define RPE_I2C_TEN_BIT 2
#define RPE_SPI_CS_CHANGE 1
#define RPE_SPI_MODES (SPI_CPHA | SPI_CPOL | SPI_CS_HIGH | SPI_LSB_FIRST | SPI_NO_CS)
#define RPE_I2C_FUNCTIONS (I2C_FUNC_I2C | I2C_FUNC_10BIT_ADDR | I2C_FUNC_SMBUS_EMUL)

enum rpe_instance_type {
	RPE_INSTANCE_I2C,
	RPE_INSTANCE_SPI,
	RPE_INSTANCE_GPIO,
	RPE_INSTANCE_ADC,
	RPE_INSTANCE_PWM,
};

struct rpe_session;

struct rpe_instance {
	struct list_head node;
	struct rpe_session *session;
	enum rpe_instance_type type;
	u32 id;
	struct mutex transaction_lock;
	union {
		struct {
			struct i2c_adapter adapter;
			u32 functionality;
			u32 speed_hz;
			u8 bus_id;
		} i2c;
		struct {
			struct spi_controller *controller;
			struct spi_device **children;
			u16 chip_selects;
			u8 bus_id;
		} spi;
		struct {
			struct gpio_chip chip;
			spinlock_t irq_lock;
			unsigned long irq_enabled;
			u32 irq_type[RPE_MAX_GPIO_LINES];
			bool registering;
			char names[RPE_MAX_GPIO_LINES][6];
			const char *name_ptrs[RPE_MAX_GPIO_LINES];
		} gpio;
		struct {
			struct iio_dev *device;
			u8 channels;
			u8 resolution_bits;
			u16 reference_mv;
		} adc;
		struct {
			struct pwm_chip *chip;
			u16 enabled;
			u32 clock_hz;
			bool registering;
		} pwm;
	};
};

struct rpe_request {
	struct list_head node;
	struct completion done;
	struct rpe_instance *instance;
	u64 id;
	u16 operation;
	size_t request_len;
	size_t reply_len;
	bool delivered;
	int result;
	u8 request[RPE_HEADER_SIZE + RPE_MAX_PAYLOAD];
	u8 reply[RPE_MAX_PAYLOAD];
};

struct rpe_session {
	struct mutex lock;
	struct list_head requests;
	struct list_head instances;
	wait_queue_head_t read_wait;
	struct ida instance_ids;
	atomic_t readable_requests;
	u64 next_request_id;
	bool closing;
};

static DEFINE_MUTEX(rpe_open_lock);
static bool rpe_opened;
static struct miscdevice rpe_misc;

static_assert(sizeof(struct rpe_ioc_version) == 16);
static_assert(sizeof(struct rpe_ioc_create_i2c) == 20);
static_assert(sizeof(struct rpe_ioc_create_spi) == 24);
static_assert(sizeof(struct rpe_ioc_destroy) == 8);
static_assert(sizeof(struct rpe_ioc_create_gpio) == 48);
static_assert(sizeof(struct rpe_ioc_create_adc) == 16);
static_assert(sizeof(struct rpe_ioc_create_pwm) == 16);

static void rpe_put16(u8 *p, u16 value)
{
	put_unaligned_le16(value, p);
}

static void rpe_put32(u8 *p, u32 value)
{
	put_unaligned_le32(value, p);
}

static void rpe_put64(u8 *p, u64 value)
{
	put_unaligned_le64(value, p);
}

static u16 rpe_get16(const u8 *p)
{
	return get_unaligned_le16(p);
}

static u32 rpe_get32(const u8 *p)
{
	return get_unaligned_le32(p);
}

static u64 rpe_get64(const u8 *p)
{
	return get_unaligned_le64(p);
}

static void rpe_make_header(struct rpe_request *request, u16 operation,
			    u32 payload_len)
{
	u8 *header = request->request;

	rpe_put32(header, RPE_MAGIC);
	rpe_put16(header + 4, RPE_ABI_MAJOR);
	rpe_put16(header + 6, operation);
	rpe_put32(header + 8, request->instance->id);
	rpe_put32(header + 12, payload_len);
	rpe_put64(header + 16, request->id);
	rpe_put32(header + 24, 0);
	rpe_put32(header + 28, 0);
	request->operation = operation;
	request->request_len = RPE_HEADER_SIZE + payload_len;
}

static int rpe_status_errno(s32 status)
{
	switch (status) {
	case RPE_STATUS_OK:
		return 0;
	case RPE_STATUS_NACK:
		return -EREMOTEIO;
	case RPE_STATUS_ARBITRATION_LOST:
		return -EAGAIN;
	case RPE_STATUS_TIMEOUT:
		return -ETIMEDOUT;
	case RPE_STATUS_DISCONNECTED:
		return -ENODEV;
	case RPE_STATUS_UNSUPPORTED:
		return -EOPNOTSUPP;
	case RPE_STATUS_BAD_FRAME:
		return -EPROTO;
	case RPE_STATUS_IO_ERROR:
		return -EIO;
	case RPE_STATUS_INVALID_ROUTE:
	case RPE_STATUS_PIN_CONFLICT:
		return -EINVAL;
	case RPE_STATUS_BUSY:
		return -EBUSY;
	default:
		return -EPROTO;
	}
}

static int rpe_submit(struct rpe_instance *instance, struct rpe_request *request)
{
	struct rpe_session *session = instance->session;
	long waited;

	init_completion(&request->done);
	request->instance = instance;
	request->result = -ENODEV;

	mutex_lock(&session->lock);
	if (session->closing) {
		mutex_unlock(&session->lock);
		return -ENODEV;
	}
	request->id = ++session->next_request_id;
	rpe_put64(request->request + 16, request->id);
	list_add_tail(&request->node, &session->requests);
	atomic_inc(&session->readable_requests);
	mutex_unlock(&session->lock);
	wake_up_interruptible(&session->read_wait);

	waited = wait_for_completion_interruptible_timeout(&request->done, RPE_TIMEOUT);
	mutex_lock(&session->lock);
	if (!list_empty(&request->node)) {
		if (!request->delivered)
			atomic_dec(&session->readable_requests);
		list_del_init(&request->node);
	}
	if (waited == 0)
		request->result = -ETIMEDOUT;
	else if (waited < 0)
		request->result = waited;
	mutex_unlock(&session->lock);
	return request->result;
}

static int rpe_i2c_xfer(struct i2c_adapter *adapter, struct i2c_msg *messages,
			int count)
{
	struct rpe_instance *instance = i2c_get_adapdata(adapter);
	struct rpe_request *request;
	size_t offset = RPE_HEADER_SIZE + 4;
	size_t read_len = 0;
	size_t write_len = 0;
	int i, ret;

	if (count < 1 || count > RPE_MAX_I2C_MSGS)
		return -EOPNOTSUPP;
	request = kzalloc(sizeof(*request), GFP_KERNEL);
	if (!request)
		return -ENOMEM;
	INIT_LIST_HEAD(&request->node);
	request->request[RPE_HEADER_SIZE] = instance->i2c.bus_id;
	request->request[RPE_HEADER_SIZE + 1] = count;

	for (i = 0; i < count; i++) {
		u16 wire_flags = 0;

		if (messages[i].flags & ~(I2C_M_RD | I2C_M_TEN | I2C_M_DMA_SAFE)) {
			ret = -EOPNOTSUPP;
			goto free_request;
		}
		if (messages[i].addr > ((messages[i].flags & I2C_M_TEN) ? 0x3ff : 0x7f)) {
			ret = -EINVAL;
			goto free_request;
		}
		if (messages[i].flags & I2C_M_RD) {
			wire_flags |= RPE_I2C_READ;
			read_len += messages[i].len;
		}
		if (messages[i].flags & I2C_M_TEN)
			wire_flags |= RPE_I2C_TEN_BIT;
		if (!(wire_flags & RPE_I2C_READ))
			write_len += messages[i].len;
		if (offset + 8 + ((wire_flags & RPE_I2C_READ) ? 0 : messages[i].len) >
		    sizeof(request->request) || read_len > RPE_MAX_DATA ||
		    write_len > RPE_MAX_DATA) {
			ret = -EMSGSIZE;
			goto free_request;
		}
		rpe_put16(request->request + offset, messages[i].addr);
		rpe_put16(request->request + offset + 2, wire_flags);
		rpe_put16(request->request + offset + 4, messages[i].len);
		rpe_put16(request->request + offset + 6, 0);
		offset += 8;
		if (!(wire_flags & RPE_I2C_READ)) {
			memcpy(request->request + offset, messages[i].buf, messages[i].len);
			offset += messages[i].len;
		}
	}

	mutex_lock(&instance->transaction_lock);
	request->instance = instance;
	rpe_make_header(request, RPE_OP_I2C, offset - RPE_HEADER_SIZE);
	ret = rpe_submit(instance, request);
	if (!ret) {
		size_t input = 4;

		if (request->reply_len != 4 + read_len || rpe_get32(request->reply) != count) {
			ret = -EPROTO;
			goto unlock;
		}
		for (i = 0; i < count; i++) {
			if (messages[i].flags & I2C_M_RD) {
				memcpy(messages[i].buf, request->reply + input, messages[i].len);
				input += messages[i].len;
			}
		}
		ret = count;
	}
unlock:
	mutex_unlock(&instance->transaction_lock);
free_request:
	kfree(request);
	return ret;
}

static u32 rpe_i2c_functionality(struct i2c_adapter *adapter)
{
	struct rpe_instance *instance = i2c_get_adapdata(adapter);

	return instance->i2c.functionality;
}

static const struct i2c_algorithm rpe_i2c_algorithm = {
	.xfer = rpe_i2c_xfer,
	.functionality = rpe_i2c_functionality,
};

static int rpe_delay_ns(struct spi_delay delay, struct spi_transfer *transfer,
			u32 *nanoseconds)
{
	int value = spi_delay_to_ns(&delay, transfer);

	if (value < 0)
		return value;
	*nanoseconds = value;
	return 0;
}

static int rpe_spi_message(struct spi_controller *controller,
			   struct spi_message *message)
{
	struct rpe_instance *instance =
		*(struct rpe_instance **)spi_controller_get_devdata(controller);
	struct rpe_request *request;
	struct spi_transfer *transfer;
	size_t offset = RPE_HEADER_SIZE + 8;
	size_t data_len = 0;
	u8 segments = 0;
	int ret = 0;

	request = kzalloc(sizeof(*request), GFP_KERNEL);
	if (!request) {
		ret = -ENOMEM;
		goto finish;
	}
	INIT_LIST_HEAD(&request->node);
	request->request[RPE_HEADER_SIZE] = instance->spi.bus_id;
	request->request[RPE_HEADER_SIZE + 1] = spi_get_chipselect(message->spi, 0);
	rpe_put16(request->request + RPE_HEADER_SIZE + 2, message->spi->mode);

	list_for_each_entry(transfer, &message->transfers, transfer_list) {
		u32 speed = transfer->speed_hz ?: message->spi->max_speed_hz;
		u16 flags = transfer->cs_change ? RPE_SPI_CS_CHANGE : 0;
		u32 word_delay, delay, cs_change_delay;

		if ((transfer->bits_per_word && transfer->bits_per_word != 8) ||
		    ++segments > RPE_MAX_SPI_SEGMENTS ||
		    data_len + transfer->len > RPE_MAX_DATA || offset + 20 + transfer->len >
		    sizeof(request->request)) {
			ret = -EOPNOTSUPP;
			goto free_request;
		}
		ret = rpe_delay_ns(transfer->word_delay, transfer, &word_delay);
		if (!ret)
			ret = rpe_delay_ns(transfer->delay, transfer, &delay);
		if (!ret)
			ret = rpe_delay_ns(transfer->cs_change_delay, transfer,
					   &cs_change_delay);
		if (ret)
			goto free_request;
		rpe_put32(request->request + offset, speed);
		rpe_put16(request->request + offset + 4, transfer->len);
		rpe_put16(request->request + offset + 6, flags);
		rpe_put32(request->request + offset + 8, word_delay);
		rpe_put32(request->request + offset + 12, delay);
		rpe_put32(request->request + offset + 16, cs_change_delay);
		offset += 20;
		if (transfer->tx_buf)
			memcpy(request->request + offset, transfer->tx_buf, transfer->len);
		else
			memset(request->request + offset, 0, transfer->len);
		offset += transfer->len;
		data_len += transfer->len;
	}
	if (!segments) {
		ret = -EINVAL;
		goto free_request;
	}
	request->request[RPE_HEADER_SIZE + 4] = segments;

	mutex_lock(&instance->transaction_lock);
	request->instance = instance;
	rpe_make_header(request, RPE_OP_SPI, offset - RPE_HEADER_SIZE);
	ret = rpe_submit(instance, request);
	if (!ret) {
		size_t input = 8 + segments * 4;
		size_t output = 0;
		int index = 0;

		if (request->reply_len != input + data_len || request->reply[4] != segments ||
		    request->reply[5] || request->reply[6] || request->reply[7]) {
			ret = -EPROTO;
			goto unlock_spi;
		}
		list_for_each_entry(transfer, &message->transfers, transfer_list) {
			transfer->effective_speed_hz = rpe_get32(request->reply + 8 + index * 4);
			if (transfer->rx_buf)
				memcpy(transfer->rx_buf, request->reply + input + output,
				       transfer->len);
			output += transfer->len;
			index++;
		}
		message->actual_length = rpe_get32(request->reply);
	}
unlock_spi:
	mutex_unlock(&instance->transaction_lock);
free_request:
	kfree(request);
finish:
	message->status = ret;
	spi_finalize_current_message(controller);
	return 0;
}

static int rpe_gpio_call(struct rpe_instance *instance, u8 command,
			 unsigned int offset, int value)
{
	struct rpe_request *request;
	int ret;

	if (offset >= instance->gpio.chip.ngpio)
		return -EINVAL;
	/* gpiochip_add_data() queries initial directions before its ioctl can
	 * return the instance ID to the daemon. Firmware starts every exposed
	 * line as a low input, so answer registration-time probes locally.
	 */
	if (READ_ONCE(instance->gpio.registering))
		return command == RPE_GPIO_GET_DIRECTION ||
		       command == RPE_GPIO_DIRECTION_INPUT;
	request = kzalloc(sizeof(*request), GFP_KERNEL);
	if (!request)
		return -ENOMEM;
	INIT_LIST_HEAD(&request->node);
	request->request[RPE_HEADER_SIZE] = command;
	request->request[RPE_HEADER_SIZE + 1] = offset;
	request->request[RPE_HEADER_SIZE + 2] = value;
	request->instance = instance;

	mutex_lock(&instance->transaction_lock);
	rpe_make_header(request, RPE_OP_GPIO, 4);
	ret = rpe_submit(instance, request);
	if (!ret) {
		if (request->reply_len != 8 || rpe_get32(request->reply) != 1 ||
		    request->reply[4] > (command == RPE_GPIO_SET_BIAS ? 2 : 1) ||
		    request->reply[5] || request->reply[6] ||
		    request->reply[7])
			ret = -EPROTO;
		else
			ret = request->reply[4];
	}
	mutex_unlock(&instance->transaction_lock);
	kfree(request);
	return ret;
}

static int rpe_gpio_get_direction(struct gpio_chip *chip, unsigned int offset)
{
	return rpe_gpio_call(gpiochip_get_data(chip), RPE_GPIO_GET_DIRECTION,
			     offset, 0);
}

static int rpe_gpio_direction_input(struct gpio_chip *chip, unsigned int offset)
{
	int ret = rpe_gpio_call(gpiochip_get_data(chip), RPE_GPIO_DIRECTION_INPUT,
				 offset, 0);

	return ret < 0 ? ret : 0;
}

static int rpe_gpio_direction_output(struct gpio_chip *chip, unsigned int offset,
				     int value)
{
	int ret = rpe_gpio_call(gpiochip_get_data(chip), RPE_GPIO_DIRECTION_OUTPUT,
				 offset, value);

	return ret < 0 ? ret : 0;
}

static int rpe_gpio_get(struct gpio_chip *chip, unsigned int offset)
{
	return rpe_gpio_call(gpiochip_get_data(chip), RPE_GPIO_GET, offset, 0);
}

static int rpe_gpio_get_multiple(struct gpio_chip *chip, unsigned long *mask,
				 unsigned long *bits)
{
	struct rpe_instance *instance = gpiochip_get_data(chip);
	struct rpe_request *request;
	u32 values;
	int ret;

	request = kzalloc(sizeof(*request), GFP_KERNEL);
	if (!request)
		return -ENOMEM;
	INIT_LIST_HEAD(&request->node);
	request->request[RPE_HEADER_SIZE] = RPE_GPIO_SNAPSHOT;
	request->instance = instance;
	mutex_lock(&instance->transaction_lock);
	rpe_make_header(request, RPE_OP_GPIO, 4);
	ret = rpe_submit(instance, request);
	if (!ret) {
		if (request->reply_len != 20 || rpe_get32(request->reply) != chip->ngpio)
			ret = -EPROTO;
		else {
			values = rpe_get32(request->reply + 4);
			bitmap_from_arr32(bits, &values, min_t(unsigned int, chip->ngpio, 32));
			bitmap_and(bits, bits, mask, chip->ngpio);
		}
	}
	mutex_unlock(&instance->transaction_lock);
	kfree(request);
	return ret;
}

static int rpe_gpio_set_config(struct gpio_chip *chip, unsigned int offset,
			       unsigned long config)
{
	u8 bias;
	int ret;

	switch (pinconf_to_config_param(config)) {
	case PIN_CONFIG_BIAS_DISABLE:
		bias = RPE_GPIO_BIAS_NONE;
		break;
	case PIN_CONFIG_BIAS_PULL_UP:
		bias = RPE_GPIO_BIAS_PULL_UP;
		break;
	case PIN_CONFIG_BIAS_PULL_DOWN:
		bias = RPE_GPIO_BIAS_PULL_DOWN;
		break;
	default:
		return -ENOTSUPP;
	}
	ret = rpe_gpio_call(gpiochip_get_data(chip), RPE_GPIO_SET_BIAS, offset, bias);
	return ret < 0 ? ret : 0;
}

static int rpe_gpio_set_result(struct gpio_chip *chip, unsigned int offset, int value)
{
	int ret = rpe_gpio_call(gpiochip_get_data(chip), RPE_GPIO_SET, offset, value);

	return ret < 0 ? ret : 0;
}

#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 17, 0)
static int rpe_gpio_set(struct gpio_chip *chip, unsigned int offset, int value)
{
	return rpe_gpio_set_result(chip, offset, value);
}
#else
static void rpe_gpio_set(struct gpio_chip *chip, unsigned int offset, int value)
{
	rpe_gpio_set_result(chip, offset, value);
}
#endif

static int rpe_adc_read_raw(struct iio_dev *device,
			    const struct iio_chan_spec *channel, int *value,
			    int *value2, long mask)
{
	struct rpe_instance *instance = *(struct rpe_instance **)iio_priv(device);
	struct rpe_request *request;
	int ret;

	if (mask == IIO_CHAN_INFO_SCALE) {
		*value = instance->adc.reference_mv;
		*value2 = instance->adc.resolution_bits;
		return IIO_VAL_FRACTIONAL_LOG2;
	}
	if (mask != IIO_CHAN_INFO_RAW || channel->channel >= instance->adc.channels)
		return -EINVAL;
	request = kzalloc(sizeof(*request), GFP_KERNEL);
	if (!request)
		return -ENOMEM;
	INIT_LIST_HEAD(&request->node);
	request->request[RPE_HEADER_SIZE] = channel->channel;
	request->instance = instance;
	mutex_lock(&instance->transaction_lock);
	rpe_make_header(request, RPE_OP_ADC, 4);
	ret = rpe_submit(instance, request);
	if (!ret) {
		if (request->reply_len != 2)
			ret = -EPROTO;
		else {
			*value = rpe_get16(request->reply);
			ret = IIO_VAL_INT;
		}
	}
	mutex_unlock(&instance->transaction_lock);
	kfree(request);
	return ret;
}

static const struct iio_info rpe_adc_info = {
	.read_raw = rpe_adc_read_raw,
};

#define RPE_ADC_CHANNEL(_index) { \
	.type = IIO_VOLTAGE, \
	.indexed = 1, \
	.channel = (_index), \
	.info_mask_separate = BIT(IIO_CHAN_INFO_RAW), \
	.info_mask_shared_by_type = BIT(IIO_CHAN_INFO_SCALE), \
}

static const struct iio_chan_spec rpe_adc_channels[] = {
	RPE_ADC_CHANNEL(0),
	RPE_ADC_CHANNEL(1),
	RPE_ADC_CHANNEL(2),
};

static int rpe_pwm_call(struct rpe_instance *instance, unsigned int channel,
			u8 command, struct pwm_state *state)
{
	struct rpe_request *request;
	u16 flags = 0;
	int ret;

	if (channel >= instance->pwm.chip->npwm ||
	    !(instance->pwm.enabled & BIT(channel)))
		return -EOPNOTSUPP;
	if (READ_ONCE(instance->pwm.registering)) {
		if (command != RPE_PWM_GET)
			return -EBUSY;
		memset(state, 0, sizeof(*state));
		state->polarity = PWM_POLARITY_NORMAL;
		return 0;
	}
	if (state->enabled)
		flags |= RPE_PWM_ENABLED;
	if (state->polarity == PWM_POLARITY_INVERSED)
		flags |= RPE_PWM_INVERTED;
	request = kzalloc(sizeof(*request), GFP_KERNEL);
	if (!request)
		return -ENOMEM;
	INIT_LIST_HEAD(&request->node);
	request->request[RPE_HEADER_SIZE] = command;
	request->request[RPE_HEADER_SIZE + 1] = channel;
	rpe_put16(request->request + RPE_HEADER_SIZE + 2, flags);
	rpe_put64(request->request + RPE_HEADER_SIZE + 4, state->period);
	rpe_put64(request->request + RPE_HEADER_SIZE + 12, state->duty_cycle);
	request->instance = instance;
	mutex_lock(&instance->transaction_lock);
	rpe_make_header(request, RPE_OP_PWM, RPE_PWM_STATE_SIZE);
	ret = rpe_submit(instance, request);
	if (!ret) {
		if (request->reply_len != RPE_PWM_STATE_SIZE ||
		    request->reply[0] != command || request->reply[1] != channel ||
		    (rpe_get16(request->reply + 2) & ~(RPE_PWM_ENABLED | RPE_PWM_INVERTED))) {
			ret = -EPROTO;
		} else {
			flags = rpe_get16(request->reply + 2);
			state->enabled = !!(flags & RPE_PWM_ENABLED);
			state->polarity = flags & RPE_PWM_INVERTED ?
				PWM_POLARITY_INVERSED : PWM_POLARITY_NORMAL;
			state->period = rpe_get64(request->reply + 4);
			state->duty_cycle = rpe_get64(request->reply + 12);
			if (state->duty_cycle > state->period ||
			    (state->enabled && !state->period))
				ret = -EPROTO;
		}
	}
	mutex_unlock(&instance->transaction_lock);
	kfree(request);
	return ret;
}

static int rpe_pwm_apply(struct pwm_chip *chip, struct pwm_device *pwm,
			 const struct pwm_state *state)
{
	struct pwm_state applied = *state;

	return rpe_pwm_call(pwmchip_get_drvdata(chip), pwm->hwpwm,
			    RPE_PWM_APPLY, &applied);
}

static int rpe_pwm_get_state(struct pwm_chip *chip, struct pwm_device *pwm,
			     struct pwm_state *state)
{
	memset(state, 0, sizeof(*state));
	state->polarity = PWM_POLARITY_NORMAL;
	return rpe_pwm_call(pwmchip_get_drvdata(chip), pwm->hwpwm,
			    RPE_PWM_GET, state);
}

static const struct pwm_ops rpe_pwm_ops = {
	.apply = rpe_pwm_apply,
	.get_state = rpe_pwm_get_state,
};

static void rpe_gpio_irq_mask(struct irq_data *data)
{
	struct gpio_chip *chip = irq_data_get_irq_chip_data(data);
	struct rpe_instance *instance = gpiochip_get_data(chip);
	unsigned long flags;

	spin_lock_irqsave(&instance->gpio.irq_lock, flags);
	clear_bit(irqd_to_hwirq(data), &instance->gpio.irq_enabled);
	spin_unlock_irqrestore(&instance->gpio.irq_lock, flags);
	gpiochip_disable_irq(chip, irqd_to_hwirq(data));
}

static void rpe_gpio_irq_unmask(struct irq_data *data)
{
	struct gpio_chip *chip = irq_data_get_irq_chip_data(data);
	struct rpe_instance *instance = gpiochip_get_data(chip);
	unsigned long flags;

	gpiochip_enable_irq(chip, irqd_to_hwirq(data));
	spin_lock_irqsave(&instance->gpio.irq_lock, flags);
	set_bit(irqd_to_hwirq(data), &instance->gpio.irq_enabled);
	spin_unlock_irqrestore(&instance->gpio.irq_lock, flags);
}

static int rpe_gpio_irq_set_type(struct irq_data *data, unsigned int type)
{
	struct gpio_chip *chip = irq_data_get_irq_chip_data(data);
	struct rpe_instance *instance = gpiochip_get_data(chip);
	unsigned int offset = irqd_to_hwirq(data);
	unsigned long flags;

	if (type != IRQ_TYPE_EDGE_RISING && type != IRQ_TYPE_EDGE_FALLING &&
	    type != IRQ_TYPE_EDGE_BOTH)
		return -EINVAL;
	spin_lock_irqsave(&instance->gpio.irq_lock, flags);
	instance->gpio.irq_type[offset] = type;
	spin_unlock_irqrestore(&instance->gpio.irq_lock, flags);
	return 0;
}

static const struct irq_chip rpe_gpio_irq_chip = {
	.name = "rp-extio-gpio",
	.irq_mask = rpe_gpio_irq_mask,
	.irq_unmask = rpe_gpio_irq_unmask,
	.irq_set_type = rpe_gpio_irq_set_type,
	.flags = IRQCHIP_IMMUTABLE,
	GPIOCHIP_IRQ_RESOURCE_HELPERS,
};

static struct rpe_request *rpe_next_request(struct rpe_session *session)
{
	struct rpe_request *request;

	list_for_each_entry(request, &session->requests, node)
		if (!request->delivered)
			return request;
	return NULL;
}

static ssize_t rpe_read(struct file *file, char __user *buffer, size_t count,
			loff_t *position)
{
	struct rpe_session *session = file->private_data;
	struct rpe_request *request;
	int ret;

	if (count < RPE_HEADER_SIZE)
		return -EMSGSIZE;
	if (file->f_flags & O_NONBLOCK) {
		if (!READ_ONCE(session->closing) &&
		    !atomic_read(&session->readable_requests))
			return -EAGAIN;
	} else {
		ret = wait_event_interruptible(session->read_wait,
					       READ_ONCE(session->closing) ||
					       atomic_read(&session->readable_requests));
		if (ret)
			return ret;
	}
	mutex_lock(&session->lock);
	request = rpe_next_request(session);
	if (!request) {
		ret = session->closing ? -ENODEV : -EAGAIN;
		goto unlock;
	}
	if (count < request->request_len) {
		ret = -EMSGSIZE;
		goto unlock;
	}
	if (copy_to_user(buffer, request->request, request->request_len)) {
		ret = -EFAULT;
		goto unlock;
	}
	request->delivered = true;
	atomic_dec(&session->readable_requests);
	ret = request->request_len;
unlock:
	mutex_unlock(&session->lock);
	return ret;
}

static ssize_t rpe_write(struct file *file, const char __user *buffer,
			 size_t count, loff_t *position)
{
	struct rpe_session *session = file->private_data;
	struct rpe_request *request;
	u8 *frame;
	u32 payload_len, instance_id;
	u16 operation;
	u64 request_id;
	s32 status;
	int ret = -ENOENT;

	if (count < RPE_HEADER_SIZE || count > RPE_HEADER_SIZE + RPE_MAX_PAYLOAD)
		return -EMSGSIZE;
	frame = memdup_user(buffer, count);
	if (IS_ERR(frame))
		return PTR_ERR(frame);
	if (rpe_get32(frame) != RPE_MAGIC || rpe_get16(frame + 4) != RPE_ABI_MAJOR ||
	    rpe_get32(frame + 28)) {
		ret = -EPROTO;
		goto free_frame;
	}
	operation = rpe_get16(frame + 6);
	instance_id = rpe_get32(frame + 8);
	payload_len = rpe_get32(frame + 12);
	request_id = rpe_get64(frame + 16);
	status = (s32)rpe_get32(frame + 24);
	if (payload_len > RPE_MAX_PAYLOAD || count != RPE_HEADER_SIZE + payload_len ||
	    (rpe_status_errno(status) == -EPROTO && status != RPE_STATUS_BAD_FRAME)) {
		ret = -EPROTO;
		goto free_frame;
	}

	mutex_lock(&session->lock);
	if (operation == RPE_OP_GPIO_EVENT) {
		struct rpe_instance *instance;
		u8 line, old_value, new_value;
		unsigned long flags;
		u32 type;
		bool enabled, trigger;

		if (request_id || status || payload_len != 4 || frame[RPE_HEADER_SIZE + 3]) {
			ret = -EPROTO;
			goto unlock_write;
		}
		line = frame[RPE_HEADER_SIZE];
		old_value = frame[RPE_HEADER_SIZE + 1];
		new_value = frame[RPE_HEADER_SIZE + 2];
		if (old_value > 1 || new_value > 1 || old_value == new_value) {
			ret = -EPROTO;
			goto unlock_write;
		}
		ret = -ENOENT;
		list_for_each_entry(instance, &session->instances, node) {
			if (instance->id != instance_id || instance->type != RPE_INSTANCE_GPIO)
				continue;
			if (line >= instance->gpio.chip.ngpio) {
				ret = -EINVAL;
				break;
			}
			spin_lock_irqsave(&instance->gpio.irq_lock, flags);
			enabled = test_bit(line, &instance->gpio.irq_enabled);
			type = instance->gpio.irq_type[line];
			spin_unlock_irqrestore(&instance->gpio.irq_lock, flags);
			trigger = (new_value && (type & IRQ_TYPE_EDGE_RISING)) ||
				  (!new_value && (type & IRQ_TYPE_EDGE_FALLING));
			if (enabled && trigger)
				generic_handle_domain_irq(instance->gpio.chip.irq.domain, line);
			ret = count;
			break;
		}
		goto unlock_write;
	}
	list_for_each_entry(request, &session->requests, node) {
		if (!request->delivered || request->id != request_id)
			continue;
		if (request->instance->id != instance_id || request->operation != operation) {
			ret = -EPROTO;
			break;
		}
		memcpy(request->reply, frame + RPE_HEADER_SIZE, payload_len);
		request->reply_len = payload_len;
		request->result = rpe_status_errno(status);
		list_del_init(&request->node);
		complete(&request->done);
		ret = count;
		break;
	}
unlock_write:
	mutex_unlock(&session->lock);
free_frame:
	kfree(frame);
	return ret;
}

static __poll_t rpe_poll(struct file *file, poll_table *wait)
{
	struct rpe_session *session = file->private_data;
	__poll_t mask = 0;

	poll_wait(file, &session->read_wait, wait);
	mutex_lock(&session->lock);
	if (rpe_next_request(session))
		mask |= EPOLLIN | EPOLLRDNORM;
	if (session->closing)
		mask |= EPOLLHUP | EPOLLERR;
	mutex_unlock(&session->lock);
	return mask;
}

static void rpe_destroy_instance(struct rpe_instance *instance)
{
	int i;

	if (!list_empty(&instance->node))
		list_del_init(&instance->node);
	if (instance->type == RPE_INSTANCE_I2C) {
		i2c_del_adapter(&instance->i2c.adapter);
	} else if (instance->type == RPE_INSTANCE_SPI) {
		for (i = 0; i < instance->spi.chip_selects; i++)
			if (instance->spi.children[i])
				spi_unregister_device(instance->spi.children[i]);
		spi_unregister_controller(instance->spi.controller);
		kfree(instance->spi.children);
	} else if (instance->type == RPE_INSTANCE_GPIO) {
		gpiochip_remove(&instance->gpio.chip);
	} else if (instance->type == RPE_INSTANCE_ADC) {
		iio_device_unregister(instance->adc.device);
		iio_device_free(instance->adc.device);
	} else {
		pwmchip_remove(instance->pwm.chip);
		pwmchip_put(instance->pwm.chip);
	}
	ida_free(&instance->session->instance_ids, instance->id);
	kfree(instance);
}

static int rpe_create_gpio(struct rpe_session *session, void __user *argument)
{
	struct rpe_ioc_create_gpio create;
	struct rpe_instance *instance;
	struct gpio_irq_chip *girq;
	int i, j, ret;

	if (copy_from_user(&create, argument, sizeof(create)))
		return -EFAULT;
	if (!create.lines || create.lines > RPE_MAX_GPIO_LINES || create.reserved || create.features ||
	    create.reserved2 || memchr_inv(create.reserved3, 0, sizeof(create.reserved3)))
		return -EINVAL;
	for (i = 0; i < create.lines; i++) {
		if (create.pins[i] > 28 || (create.pins[i] > 22 && create.pins[i] < 26))
			return -EINVAL;
		for (j = 0; j < i; j++)
			if (create.pins[i] == create.pins[j])
				return -EINVAL;
	}
	instance = kzalloc(sizeof(*instance), GFP_KERNEL);
	if (!instance)
		return -ENOMEM;
	instance->session = session;
	instance->type = RPE_INSTANCE_GPIO;
	instance->gpio.registering = true;
	mutex_init(&instance->transaction_lock);
	spin_lock_init(&instance->gpio.irq_lock);
	instance->id = ida_alloc_min(&session->instance_ids, 1, GFP_KERNEL);
	if ((int)instance->id < 0) {
		ret = (int)instance->id;
		goto free_instance;
	}
	instance->gpio.chip.label = "rp-extio virtual GPIO";
	instance->gpio.chip.parent = rpe_misc.this_device;
	instance->gpio.chip.owner = THIS_MODULE;
	instance->gpio.chip.get_direction = rpe_gpio_get_direction;
	instance->gpio.chip.direction_input = rpe_gpio_direction_input;
	instance->gpio.chip.direction_output = rpe_gpio_direction_output;
	instance->gpio.chip.get = rpe_gpio_get;
	instance->gpio.chip.get_multiple = rpe_gpio_get_multiple;
	instance->gpio.chip.set = rpe_gpio_set;
	instance->gpio.chip.set_config = rpe_gpio_set_config;
	instance->gpio.chip.base = -1;
	instance->gpio.chip.ngpio = create.lines;
	for (i = 0; i < create.lines; i++) {
		snprintf(instance->gpio.names[i], sizeof(instance->gpio.names[i]), "GP%u",
			 create.pins[i]);
		instance->gpio.name_ptrs[i] = instance->gpio.names[i];
	}
	instance->gpio.chip.names = instance->gpio.name_ptrs;
	instance->gpio.chip.can_sleep = true;
	girq = &instance->gpio.chip.irq;
	gpio_irq_chip_set_chip(girq, &rpe_gpio_irq_chip);
	girq->handler = handle_edge_irq;
	girq->default_type = IRQ_TYPE_NONE;
	girq->parent_handler = NULL;
	girq->num_parents = 0;
	ret = gpiochip_add_data(&instance->gpio.chip, instance);
	if (ret)
		goto free_id;
	WRITE_ONCE(instance->gpio.registering, false);
	list_add_tail(&instance->node, &session->instances);
	create.instance_id = instance->id;
	if (copy_to_user(argument, &create, sizeof(create))) {
		ret = -EFAULT;
		rpe_destroy_instance(instance);
		return ret;
	}
	return 0;
free_id:
	ida_free(&session->instance_ids, instance->id);
free_instance:
	kfree(instance);
	return ret;
}

static int rpe_create_adc(struct rpe_session *session, void __user *argument)
{
	struct rpe_ioc_create_adc create;
	struct rpe_instance *instance;
	struct iio_dev *device;
	int ret;

	if (copy_from_user(&create, argument, sizeof(create)))
		return -EFAULT;
	if (!create.channels || create.channels > ARRAY_SIZE(rpe_adc_channels) ||
	    !create.resolution_bits || create.resolution_bits > 16 ||
	    !create.reference_mv || create.reserved)
		return -EINVAL;
	instance = kzalloc(sizeof(*instance), GFP_KERNEL);
	if (!instance)
		return -ENOMEM;
	device = iio_device_alloc(rpe_misc.this_device, sizeof(struct rpe_instance *));
	if (!device) {
		ret = -ENOMEM;
		goto free_instance;
	}
	instance->session = session;
	instance->type = RPE_INSTANCE_ADC;
	instance->adc.device = device;
	instance->adc.channels = create.channels;
	instance->adc.resolution_bits = create.resolution_bits;
	instance->adc.reference_mv = create.reference_mv;
	mutex_init(&instance->transaction_lock);
	*(struct rpe_instance **)iio_priv(device) = instance;
	instance->id = ida_alloc_min(&session->instance_ids, 1, GFP_KERNEL);
	if ((int)instance->id < 0) {
		ret = (int)instance->id;
		goto free_device;
	}
	device->name = "rp-extio-adc";
	device->info = &rpe_adc_info;
	device->modes = INDIO_DIRECT_MODE;
	device->channels = rpe_adc_channels;
	device->num_channels = create.channels;
	ret = iio_device_register(device);
	if (ret)
		goto free_id;
	list_add_tail(&instance->node, &session->instances);
	create.instance_id = instance->id;
	create.device_num = iio_device_id(device);
	if (copy_to_user(argument, &create, sizeof(create))) {
		rpe_destroy_instance(instance);
		return -EFAULT;
	}
	return 0;
free_id:
	ida_free(&session->instance_ids, instance->id);
free_device:
	iio_device_free(device);
free_instance:
	kfree(instance);
	return ret;
}

static int rpe_create_pwm(struct rpe_session *session, void __user *argument)
{
	struct rpe_ioc_create_pwm create;
	struct rpe_instance *instance;
	struct pwm_chip *chip;
	int ret;

	if (copy_from_user(&create, argument, sizeof(create)))
		return -EFAULT;
	if (!create.channels || create.channels > 16 ||
	    (create.enabled & ~GENMASK(create.channels - 1, 0)) || !create.clock_hz)
		return -EINVAL;
	instance = kzalloc(sizeof(*instance), GFP_KERNEL);
	if (!instance)
		return -ENOMEM;
	chip = pwmchip_alloc(rpe_misc.this_device, create.channels, 0);
	if (IS_ERR(chip)) {
		ret = PTR_ERR(chip);
		goto free_instance;
	}
	instance->session = session;
	instance->type = RPE_INSTANCE_PWM;
	instance->pwm.chip = chip;
	instance->pwm.enabled = create.enabled;
	instance->pwm.clock_hz = create.clock_hz;
	instance->pwm.registering = true;
	mutex_init(&instance->transaction_lock);
	pwmchip_set_drvdata(chip, instance);
	instance->id = ida_alloc_min(&session->instance_ids, 1, GFP_KERNEL);
	if ((int)instance->id < 0) {
		ret = (int)instance->id;
		goto put_chip;
	}
	chip->ops = &rpe_pwm_ops;
	chip->atomic = false;
	ret = pwmchip_add(chip);
	if (ret)
		goto free_id;
	WRITE_ONCE(instance->pwm.registering, false);
	list_add_tail(&instance->node, &session->instances);
	create.instance_id = instance->id;
	create.chip_num = chip->id;
	if (copy_to_user(argument, &create, sizeof(create))) {
		rpe_destroy_instance(instance);
		return -EFAULT;
	}
	return 0;
free_id:
	ida_free(&session->instance_ids, instance->id);
put_chip:
	pwmchip_put(chip);
free_instance:
	kfree(instance);
	return ret;
}

static int rpe_create_i2c(struct rpe_session *session, void __user *argument)
{
	struct rpe_ioc_create_i2c create;
	struct rpe_instance *instance;
	int ret;

	if (copy_from_user(&create, argument, sizeof(create)))
		return -EFAULT;
	if (!create.speed_hz || !(create.functionality & I2C_FUNC_I2C) ||
	    (create.functionality & ~RPE_I2C_FUNCTIONS) || create.bus_id > 1 ||
	    memchr_inv(create.reserved, 0, sizeof(create.reserved)))
		return -EINVAL;
	instance = kzalloc(sizeof(*instance), GFP_KERNEL);
	if (!instance)
		return -ENOMEM;
	instance->session = session;
	instance->type = RPE_INSTANCE_I2C;
	mutex_init(&instance->transaction_lock);
	instance->i2c.functionality = create.functionality;
	instance->i2c.speed_hz = create.speed_hz;
	instance->i2c.bus_id = create.bus_id;
	instance->id = ida_alloc_min(&session->instance_ids, 1, GFP_KERNEL);
	if ((int)instance->id < 0) {
		ret = (int)instance->id;
		goto free_instance;
	}
	strscpy(instance->i2c.adapter.name, "rp-extio virtual I2C",
		sizeof(instance->i2c.adapter.name));
	instance->i2c.adapter.owner = THIS_MODULE;
	instance->i2c.adapter.algo = &rpe_i2c_algorithm;
	i2c_set_adapdata(&instance->i2c.adapter, instance);
	ret = i2c_add_adapter(&instance->i2c.adapter);
	if (ret)
		goto free_id;
	list_add_tail(&instance->node, &session->instances);
	create.instance_id = instance->id;
	create.bus_num = instance->i2c.adapter.nr;
	if (copy_to_user(argument, &create, sizeof(create))) {
		ret = -EFAULT;
		rpe_destroy_instance(instance);
		return ret;
	}
	return 0;
free_id:
	ida_free(&session->instance_ids, instance->id);
free_instance:
	kfree(instance);
	return ret;
}

static int rpe_create_spi(struct rpe_session *session, void __user *argument)
{
	struct rpe_ioc_create_spi create;
	struct rpe_instance *instance;
	struct spi_controller *controller;
	int i, ret;

	if (copy_from_user(&create, argument, sizeof(create)))
		return -EFAULT;
	if (!create.chip_selects || create.chip_selects > 32 || create.bus_id > 1 ||
	    create.reserved ||
	    !create.min_speed_hz || create.min_speed_hz > create.max_speed_hz ||
	    (create.mode_bits & ~RPE_SPI_MODES))
		return -EINVAL;
	if (request_module("spidev"))
		return -ENODEV;
	instance = kzalloc(sizeof(*instance), GFP_KERNEL);
	if (!instance)
		return -ENOMEM;
	instance->session = session;
	instance->type = RPE_INSTANCE_SPI;
	mutex_init(&instance->transaction_lock);
	instance->spi.chip_selects = create.chip_selects;
	instance->spi.bus_id = create.bus_id;
	instance->spi.children = kcalloc(create.chip_selects,
					 sizeof(*instance->spi.children), GFP_KERNEL);
	if (!instance->spi.children) {
		ret = -ENOMEM;
		goto free_instance;
	}
	instance->id = ida_alloc_min(&session->instance_ids, 1, GFP_KERNEL);
	if ((int)instance->id < 0) {
		ret = (int)instance->id;
		goto free_children;
	}
	controller = spi_alloc_host(rpe_misc.this_device, sizeof(struct rpe_instance *));
	if (!controller) {
		ret = -ENOMEM;
		goto free_id;
	}
	*(struct rpe_instance **)spi_controller_get_devdata(controller) = instance;
	instance->spi.controller = controller;
	controller->num_chipselect = create.chip_selects;
	controller->mode_bits = create.mode_bits;
	controller->bits_per_word_mask = SPI_BPW_MASK(8);
	controller->min_speed_hz = create.min_speed_hz;
	controller->max_speed_hz = create.max_speed_hz;
	controller->transfer_one_message = rpe_spi_message;
	ret = spi_register_controller(controller);
	if (ret)
		goto put_controller;
	for (i = 0; i < create.chip_selects; i++) {
		struct spi_board_info info = {
			.modalias = "spidev",
			.max_speed_hz = create.max_speed_hz,
			.chip_select = i,
		};

		instance->spi.children[i] = spi_new_device(controller, &info);
		if (!instance->spi.children[i]) {
			ret = -ENODEV;
			goto unregister_children;
		}
		ret = device_set_driver_override(&instance->spi.children[i]->dev,
						 "spidev");
		if (ret)
			goto unregister_current_child;
		ret = device_attach(&instance->spi.children[i]->dev);
		if (ret < 0)
			goto unregister_current_child;
		if (!ret) {
			ret = -ENODEV;
			goto unregister_current_child;
		}
	}
	list_add_tail(&instance->node, &session->instances);
	create.instance_id = instance->id;
	create.bus_num = controller->bus_num;
	if (copy_to_user(argument, &create, sizeof(create))) {
		ret = -EFAULT;
		rpe_destroy_instance(instance);
		return ret;
	}
	return 0;
unregister_current_child:
	spi_unregister_device(instance->spi.children[i]);
	instance->spi.children[i] = NULL;
unregister_children:
	while (i--)
		spi_unregister_device(instance->spi.children[i]);
	spi_unregister_controller(controller);
	goto free_id;
put_controller:
	spi_controller_put(controller);
free_id:
	ida_free(&session->instance_ids, instance->id);
free_children:
	kfree(instance->spi.children);
free_instance:
	kfree(instance);
	return ret;
}

static long rpe_ioctl(struct file *file, unsigned int command, unsigned long arg)
{
	struct rpe_session *session = file->private_data;
	void __user *argument = (void __user *)arg;
	struct rpe_instance *instance;
	struct rpe_ioc_destroy destroy;
	struct rpe_ioc_version version = {
		.major = RPE_ABI_MAJOR,
		.minor = RPE_ABI_MINOR,
		.max_payload = RPE_MAX_PAYLOAD,
	};
	int ret = 0;

	mutex_lock(&session->lock);
	if (session->closing) {
		ret = -ENODEV;
		goto unlock;
	}
	switch (command) {
	case RPE_IOC_GET_VERSION:
		ret = copy_to_user(argument, &version, sizeof(version)) ? -EFAULT : 0;
		break;
	case RPE_IOC_CREATE_I2C:
		ret = rpe_create_i2c(session, argument);
		break;
	case RPE_IOC_CREATE_SPI:
		ret = rpe_create_spi(session, argument);
		break;
	case RPE_IOC_CREATE_GPIO:
		ret = rpe_create_gpio(session, argument);
		break;
	case RPE_IOC_CREATE_ADC:
		ret = rpe_create_adc(session, argument);
		break;
	case RPE_IOC_CREATE_PWM:
		ret = rpe_create_pwm(session, argument);
		break;
	case RPE_IOC_DESTROY:
		if (copy_from_user(&destroy, argument, sizeof(destroy))) {
			ret = -EFAULT;
			break;
		}
		if (destroy.reserved) {
			ret = -EINVAL;
			break;
		}
		ret = -ENOENT;
		list_for_each_entry(instance, &session->instances, node) {
			if (instance->id == destroy.instance_id) {
				list_del_init(&instance->node);
				mutex_unlock(&session->lock);
				rpe_destroy_instance(instance);
				return 0;
			}
		}
		break;
	default:
		ret = -ENOTTY;
	}
unlock:
	mutex_unlock(&session->lock);
	return ret;
}

static int rpe_open(struct inode *inode, struct file *file)
{
	struct rpe_session *session;

	mutex_lock(&rpe_open_lock);
	if (rpe_opened) {
		mutex_unlock(&rpe_open_lock);
		return -EBUSY;
	}
	session = kzalloc(sizeof(*session), GFP_KERNEL);
	if (!session) {
		mutex_unlock(&rpe_open_lock);
		return -ENOMEM;
	}
	mutex_init(&session->lock);
	INIT_LIST_HEAD(&session->requests);
	INIT_LIST_HEAD(&session->instances);
	init_waitqueue_head(&session->read_wait);
	ida_init(&session->instance_ids);
	atomic_set(&session->readable_requests, 0);
	rpe_opened = true;
	file->private_data = session;
	mutex_unlock(&rpe_open_lock);
	return 0;
}

static int rpe_release(struct inode *inode, struct file *file)
{
	struct rpe_session *session = file->private_data;
	struct rpe_request *request, *request_tmp;
	struct rpe_instance *instance, *instance_tmp;
	LIST_HEAD(instances);

	mutex_lock(&session->lock);
	session->closing = true;
	list_for_each_entry_safe(request, request_tmp, &session->requests, node) {
		list_del_init(&request->node);
		request->result = -ENODEV;
		complete(&request->done);
	}
	atomic_set(&session->readable_requests, 0);
	wake_up_interruptible(&session->read_wait);
	list_splice_init(&session->instances, &instances);
	mutex_unlock(&session->lock);
	list_for_each_entry_safe(instance, instance_tmp, &instances, node)
		rpe_destroy_instance(instance);
	ida_destroy(&session->instance_ids);
	kfree(session);

	mutex_lock(&rpe_open_lock);
	rpe_opened = false;
	mutex_unlock(&rpe_open_lock);
	return 0;
}

static const struct file_operations rpe_fops = {
	.owner = THIS_MODULE,
	.open = rpe_open,
	.release = rpe_release,
	.read = rpe_read,
	.write = rpe_write,
	.poll = rpe_poll,
	.unlocked_ioctl = rpe_ioctl,
	.compat_ioctl = compat_ptr_ioctl,
};

static struct miscdevice rpe_misc = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = "rp-extio-control",
	.fops = &rpe_fops,
	.mode = 0600,
};

static int __init rpe_init(void)
{
	return misc_register(&rpe_misc);
}

static void __exit rpe_exit(void)
{
	misc_deregister(&rpe_misc);
}

module_init(rpe_init);
module_exit(rpe_exit);

MODULE_AUTHOR("rp-extio contributors");
MODULE_DESCRIPTION("Userspace-backed I2C, SPI, and GPIO controllers");
MODULE_LICENSE("GPL");
