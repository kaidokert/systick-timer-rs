MEMORY
{
  /* NOTE 1 K = 1 KiBi = 1024 bytes */
  FLASH : ORIGIN = 0x08000000, LENGTH = 512K  /* 1 MB Flash, adjust to 512K for smaller models */
  RAM : ORIGIN = 0x20000000, LENGTH = 256K    /* 256 KB SRAM */
}
