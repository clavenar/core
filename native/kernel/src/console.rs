use crate::drivers::uart;

pub(crate) fn write_task(id: usize) {
    uart::write(b"task ");
    uart::write_byte(b'0' + id as u8);
}

pub(crate) fn write_hex(value: usize) {
    uart::write(b"0x");
    for shift in (0..usize::BITS).step_by(4).rev() {
        let digit = (value >> shift) & 0xf;
        uart::write_byte(b"0123456789abcdef"[digit]);
    }
}
