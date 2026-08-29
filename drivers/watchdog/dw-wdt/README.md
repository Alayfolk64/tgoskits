# dw-wdt

Portable `no_std` register driver for the Synopsys DesignWare watchdog timer.
It uses `mmio-api`, supports fixed or device-tree supplied TOP counts, rounds a
requested timeout up to a representable value, and intentionally provides no
stop operation because the hardware generally cannot stop without reset.

FDT clock/reset preparation and RDIF registration belong to `ax-driver`; the
sleepable feed policy belongs to the consuming kernel.
