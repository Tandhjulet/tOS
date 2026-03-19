# The T Operating System

A very much work-in-progress OS & kernel for learning rust, x86-64 asm and C.

the current road map is:

- implement a network stack (WIP)
    - impl. ARP, IP, ICMP/UDP/TCP (and perhaps SSL/TLS, HTTP(S))
- shell
- file system
- GUI
- own bootloader (that works with UEFI)

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
