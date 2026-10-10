/* SPDX-License-Identifier: GPL-2.0-only WITH Linux-syscall-note */
#ifndef RP_EXTIO_UAPI_H
#define RP_EXTIO_UAPI_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define RPE_ABI_MAJOR 1
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
	__u32 instance_id;
	__s32 bus_num;
};

struct rpe_ioc_create_spi {
	__u32 mode_bits;
	__u32 min_speed_hz;
	__u32 max_speed_hz;
	__u16 chip_selects;
	__u16 reserved;
	__u32 instance_id;
	__s32 bus_num;
};

struct rpe_ioc_destroy {
	__u32 instance_id;
	__u32 reserved;
};

#define RPE_IOC_MAGIC 'R'
#define RPE_IOC_GET_VERSION _IOR(RPE_IOC_MAGIC, 0x00, struct rpe_ioc_version)
#define RPE_IOC_CREATE_I2C _IOWR(RPE_IOC_MAGIC, 0x01, struct rpe_ioc_create_i2c)
#define RPE_IOC_CREATE_SPI _IOWR(RPE_IOC_MAGIC, 0x02, struct rpe_ioc_create_spi)
#define RPE_IOC_DESTROY _IOW(RPE_IOC_MAGIC, 0x03, struct rpe_ioc_destroy)

#endif
