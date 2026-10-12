/* SPDX-License-Identifier: GPL-2.0-only WITH Linux-syscall-note */
#ifndef RP_EXTIO_UAPI_H
#define RP_EXTIO_UAPI_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define RPE_ABI_MAJOR 4
#define RPE_ABI_MINOR 0
#define RPE_MAX_PAYLOAD 4096
#define RPE_MAX_DATA 3584
#define RPE_MAX_I2C_MSGS 16
#define RPE_MAX_SPI_SEGMENTS 16

struct rpe_ioc_version {
	__u16 major;
	__u16 minor;
	__u32 max_payload;
	__u64 features;
};

struct rpe_ioc_create_i2c {
	__u32 functionality;
	__u32 speed_hz;
	__u8 bus_id;
	__u8 reserved[3];
	__u32 instance_id;
	__s32 bus_num;
};

struct rpe_ioc_create_spi {
	__u32 mode_bits;
	__u32 min_speed_hz;
	__u32 max_speed_hz;
	__u16 chip_selects;
	__u8 bus_id;
	__u8 reserved;
	__u32 instance_id;
	__s32 bus_num;
};

struct rpe_ioc_destroy {
	__u32 instance_id;
	__u32 reserved;
};

struct rpe_ioc_create_gpio {
	__u16 lines;
	__u16 reserved;
	__u32 instance_id;
	__u32 features;
	__u32 reserved2;
	__u8 pins[26];
	__u8 reserved3[6];
};

struct rpe_ioc_create_adc {
	__u8 channels;
	__u8 resolution_bits;
	__u16 reference_mv;
	__u32 instance_id;
	__s32 device_num;
	__u32 reserved;
};

struct rpe_ioc_create_pwm {
	__u16 channels;
	__u16 enabled;
	__u32 clock_hz;
	__u32 instance_id;
	__s32 chip_num;
};

struct rpe_ioc_create_uart {
	__u32 default_baud;
	__u32 max_baud;
	__u8 bus_id;
	__u8 reserved[3];
	__u32 instance_id;
	__s32 tty_index;
};

#define RPE_IOC_MAGIC 'R'
#define RPE_IOC_GET_VERSION _IOR(RPE_IOC_MAGIC, 0x00, struct rpe_ioc_version)
#define RPE_IOC_CREATE_I2C _IOWR(RPE_IOC_MAGIC, 0x01, struct rpe_ioc_create_i2c)
#define RPE_IOC_CREATE_SPI _IOWR(RPE_IOC_MAGIC, 0x02, struct rpe_ioc_create_spi)
#define RPE_IOC_DESTROY _IOW(RPE_IOC_MAGIC, 0x03, struct rpe_ioc_destroy)
#define RPE_IOC_CREATE_GPIO _IOWR(RPE_IOC_MAGIC, 0x04, struct rpe_ioc_create_gpio)
#define RPE_IOC_CREATE_ADC _IOWR(RPE_IOC_MAGIC, 0x05, struct rpe_ioc_create_adc)
#define RPE_IOC_CREATE_PWM _IOWR(RPE_IOC_MAGIC, 0x06, struct rpe_ioc_create_pwm)
#define RPE_IOC_CREATE_UART _IOWR(RPE_IOC_MAGIC, 0x07, struct rpe_ioc_create_uart)

#endif
