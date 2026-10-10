MEMORY
{
  FLASH : ORIGIN = 0x10000000, LENGTH = 2048K
  RAM   : ORIGIN = 0x20000000, LENGTH = 512K
}

SECTIONS {
  .boot_info : ALIGN(4)
  {
    KEEP(*(.boot_info));
  } > FLASH
} INSERT AFTER .vector_table;

_stext = ADDR(.boot_info) + SIZEOF(.boot_info);

SECTIONS {
  .bi_entries : ALIGN(4)
  {
    __bi_entries_start = .;
    KEEP(*(.bi_entries));
    . = ALIGN(4);
    __bi_entries_end = .;
  } > FLASH
} INSERT AFTER .text;

SECTIONS {
  .flash_end :
  {
    __flash_binary_end = .;
  } > FLASH
} INSERT AFTER .uninit;
