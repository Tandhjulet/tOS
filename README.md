# The T Operating System

A very much work-in-progress OS & kernel for learning rust, x86-64 asm and C.

the current road map is:

- shell
- file system
- GUI
- own bootloader (that works with UEFI)

## Network Stack

Currently, the network stack supports TCP/IP and UDP/IP. It has a very basic implementation of TCP that doesn't handle congestion control, packet loss recovery, etc. Furthermore, it doesn't support IP fragmentation, though that would be pretty trivial to add. As of now, while it does support DHCP, DNS is not yet implemented, either.

## Environment

```
[mads@archlinux tOS]$ rustc -vV
rustc 1.96.0-nightly (55e86c996 2026-04-02)
binary: rustc
commit-hash: 55e86c996809902e8bbad512cfb4d2c18be446d9
commit-date: 2026-04-02
host: x86_64-unknown-linux-gnu
release: 1.96.0-nightly
LLVM version: 22.1.2
```
