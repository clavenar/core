use core::ptr::{without_provenance, without_provenance_mut};

pub(crate) const BASE: usize = 0x1000_0000;
const LINE_STATUS: usize = 5;
const RX_READY: u8 = 1 << 0;
const TX_READY: u8 = 1 << 5;

pub(crate) fn write(bytes: &[u8]) {
    for &byte in bytes {
        write_byte(byte);
    }
}

pub(crate) fn write_byte(byte: u8) {
    let line_status = without_provenance::<u8>(BASE + LINE_STATUS);
    let transmit = without_provenance_mut::<u8>(BASE);

    // SAFETY: QEMU virt maps these byte-wide UART registers. OpenSBI has
    // configured the UART for console output before handing off to us.
    unsafe {
        while line_status.read_volatile() & TX_READY == 0 {
            core::hint::spin_loop();
        }
        transmit.write_volatile(byte);
    }
}

pub(crate) fn read_byte() -> Option<u8> {
    let line_status = without_provenance::<u8>(BASE + LINE_STATUS);
    let receive = without_provenance::<u8>(BASE);

    // SAFETY: QEMU virt maps these byte-wide UART registers. Reading the
    // receive register consumes one byte when data is ready.
    unsafe {
        if line_status.read_volatile() & RX_READY == 0 {
            return None;
        }
        Some(receive.read_volatile())
    }
}
