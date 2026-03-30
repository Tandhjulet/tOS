# The T Operating System

A very much work-in-progress OS & kernel for learning rust, x86-64 asm and C.

the current road map is:

- network stack
    - missing: TCP (and perhaps SSL/TLS, HTTP(S))
- shell
- file system
- GUI
- own bootloader (that works with UEFI)

## Network Stack

Currently, both RX and TX for the network stack is pretty slow. I've attached some points of improvement below, but it's unlikely they'll ever be implemented.

### Transmission

Speed is mostly slow here due to the fact that we're missing something like `sk_buff` from Linux. For example, say we want to send a DHCP packet: the DHCP layer allocates room for its header and payload and writes it to that buffer. Then, it passes this buffer onto the UDP layer as the payload. The UDP layer now allocates a new buffer with room for its own header and the payload (DHCP header + original payload) and writes to that. This cycle continues until the packet finally reaches the NIC, having been copied unnecessarily for every layer it passed through. This is obviously a major slowdown of transmission speed.

> Solution: pass a linked list (or something alike) along. Every layer that the packet passes through attaches a node pointing to a buffer containing the layer's header to the head of the linked list. Then, when it reaches the NIC driver, it iterates through the linked list, copying every buffer directly into NIC's TX buffer,

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
