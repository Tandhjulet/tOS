struct UDP {}

struct UdpMessage<'a> {
    src_port: u16,
    dst_port: u16,

    length: u16,
    checksum: u16,

    data: &'a [u8],
}
