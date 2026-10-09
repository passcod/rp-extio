#ifndef RP_EXTIO_PROTOCOL_H
#define RP_EXTIO_PROTOCOL_H

/* All multibyte fields are little endian. The header is exactly 16 bytes. */
#define RPE_MAGIC 0x32455052u /* "RPE2" */
#define RPE_VERSION 1
#define RPE_MAX_PAYLOAD 2048
#define RPE_MAX_I2C_MSGS 16
#define RPE_MAX_SPI_SEGMENTS 32

#define RPE_CMD_I2C 1
#define RPE_CMD_SPI 2

#define RPE_OK 0
#define RPE_BAD_REQUEST 1
#define RPE_IO_ERROR 2
#define RPE_UNSUPPORTED 3

/* Wire header offsets. No packed structs or host-endian assumptions. */
#define RPE_MAGIC_OFF 0
#define RPE_VERSION_OFF 4
#define RPE_COMMAND_OFF 5
#define RPE_STATUS_OFF 6
#define RPE_RESERVED_OFF 7
#define RPE_SEQUENCE_OFF 8
#define RPE_LENGTH_OFF 12
#define RPE_AUX_OFF 14
#define RPE_HEADER_SIZE 16

#endif
