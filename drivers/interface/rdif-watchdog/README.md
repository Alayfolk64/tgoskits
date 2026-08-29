# rdif-watchdog

`no_std` driver interface for reset-mode hardware watchdogs. The capability can
arm a watchdog, report the hardware-rounded timeout, ping it, and query its
programmed/remaining duration. It deliberately exposes no userspace ABI or
generic policy loop.
