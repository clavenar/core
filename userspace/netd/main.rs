#![no_std]
#![no_main]

use clv::net;
use core::panic::PanicInfo;

const FRAME_BYTES: usize = 60;
const PROBE_ETHERTYPE: [u8; 2] = 0x88b5_u16.to_be_bytes();
const PROBE_PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const PROBE_REQUEST: &[u8; 8] = b"CLVNET01";
const PROBE_REPLY: &[u8; 8] = b"CLVNET02";

// SAFETY: The kernel enters this unique symbol with an aligned user stack.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start(_task: usize, packed_mac: usize) -> ! {
    let [a, b, c, d, e, f, _, _] = (packed_mac as u64).to_le_bytes();
    let mac = [a, b, c, d, e, f];
    let request = probe_frame(mac, PROBE_PEER, PROBE_REQUEST);
    let reply = probe_frame(PROBE_PEER, mac, PROBE_REPLY);
    let mut buffer = [0; net::MAX_FRAME_BYTES];
    clv::print(b"netd ready\r\n");
    loop {
        let Some(frame) = net::receive(&mut buffer) else {
            clv::exit(1);
        };
        if frame == request {
            send_reply(&reply);
        }
    }
}

fn send_reply(frame: &[u8]) {
    loop {
        match net::transmit(frame) {
            Some(0) => core::hint::spin_loop(),
            Some(_) => return,
            None => clv::exit(1),
        }
    }
}

fn probe_frame(destination: [u8; 6], source: [u8; 6], marker: &[u8; 8]) -> [u8; FRAME_BYTES] {
    let mut frame = [0; FRAME_BYTES];
    frame[..6].copy_from_slice(&destination);
    frame[6..12].copy_from_slice(&source);
    frame[12..14].copy_from_slice(&PROBE_ETHERTYPE);
    frame[14..22].copy_from_slice(marker);
    frame
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    clv::exit(1)
}
