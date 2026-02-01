# The T Operating System

A very much work-in-progress OS & kernel for learning rust, x86-64 asm and C.

i want: (totally realistic)

- TCP/IP stack
- own bootloader (that works with UEFI)
- some sort of shell (?)
- filesystem maybe (?)
- actually make the os
- permission rings (?)

more realistic:

- come up with something myself to write text to the screen (that's not the vga buffer).
- make own exception handler + idt impl. (https://os.phil-opp.com/catching-exceptions/)

## Environment

```
[mads@archlinux tOS]$ rustc -vV
rustc 1.87.0-nightly (3ea711f17 2025-03-09)
binary: rustc
commit-hash: 3ea711f17e3946ac3f4df11691584e2c56b4b0cf
commit-date: 2025-03-09
host: x86_64-unknown-linux-gnu
release: 1.87.0-nightly
LLVM version: 20.1.0
```
