use core::ptr::{without_provenance, without_provenance_mut};

use super::order_device_accesses;
use crate::arch::monotonic_nanos;

// QEMU RV64 virt provides eight 4 KiB transport windows after the UART.
const FIRST_WINDOW: usize = 0x1000_1000;
const WINDOW_SIZE: usize = 0x1000;
const WINDOWS: usize = 8;

const MAGIC_VALUE: u32 = 0x7472_6976;
const MODERN_VERSION: u32 = 2;
const VERSION_1_HIGH_BIT: u32 = 1; // VIRTIO_F_VERSION_1 is feature bit 32.

const MAGIC: usize = 0x000;
const VERSION: usize = 0x004;
const DEVICE_ID: usize = 0x008;
const DEVICE_FEATURES: usize = 0x010;
const DEVICE_FEATURES_SEL: usize = 0x014;
const DRIVER_FEATURES: usize = 0x020;
const DRIVER_FEATURES_SEL: usize = 0x024;
const QUEUE_SEL: usize = 0x030;
const QUEUE_NUM_MAX: usize = 0x034;
const QUEUE_NUM: usize = 0x038;
const QUEUE_READY: usize = 0x044;
const QUEUE_NOTIFY: usize = 0x050;
const STATUS: usize = 0x070;
const QUEUE_DESC: usize = 0x080;
const QUEUE_DRIVER: usize = 0x090;
const QUEUE_DEVICE: usize = 0x0a0;
const CONFIG_GENERATION: usize = 0x0fc;
const CONFIG: usize = 0x100;

const ACKNOWLEDGE: u32 = 1;
const DRIVER: u32 = 2;
const DRIVER_OK: u32 = 4;
const FEATURES_OK: u32 = 8;
const DEVICE_NEEDS_RESET: u32 = 64;
const FAILED: u32 = 128;

pub(super) struct Device {
    base: usize,
}

impl Device {
    /// # Safety
    /// All QEMU virt transport windows must be accessible to the supervisor
    /// during discovery. Keep the selected window mapped while using the device.
    pub(super) unsafe fn discover(device_id: u32) -> Option<Self> {
        (0..WINDOWS)
            .map(|slot| Self {
                base: FIRST_WINDOW + slot * WINDOW_SIZE,
            })
            .find(|device| {
                device.read(MAGIC) == MAGIC_VALUE
                    && device.read(VERSION) == MODERN_VERSION
                    && device.read(DEVICE_ID) == device_id
            })
    }

    pub(super) fn base(&self) -> usize {
        self.base
    }

    pub(super) fn reset(&self, deadline: u64) -> bool {
        self.write(STATUS, 0);
        loop {
            if self.read(STATUS) == 0 {
                return true;
            }
            if monotonic_nanos() >= deadline {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    pub(super) fn negotiate(&self, supported_features: u32) -> Option<u32> {
        self.write(STATUS, ACKNOWLEDGE);
        self.write(STATUS, ACKNOWLEDGE | DRIVER);

        self.write(DEVICE_FEATURES_SEL, 0);
        let features = self.read(DEVICE_FEATURES) & supported_features;
        self.write(DEVICE_FEATURES_SEL, 1);
        if self.read(DEVICE_FEATURES) & VERSION_1_HIGH_BIT == 0 {
            return None;
        }
        // Accept VERSION_1 and the device-specific features supported by the driver.
        self.write(DRIVER_FEATURES_SEL, 0);
        self.write(DRIVER_FEATURES, features);
        self.write(DRIVER_FEATURES_SEL, 1);
        self.write(DRIVER_FEATURES, VERSION_1_HIGH_BIT);
        self.write(STATUS, ACKNOWLEDGE | DRIVER | FEATURES_OK);
        self.has_status(FEATURES_OK).then_some(features)
    }

    /// # Safety
    /// The device must expose a 64-bit configuration field at this aligned offset.
    pub(super) unsafe fn config_u64(&self, offset: usize, deadline: u64) -> Option<u64> {
        while monotonic_nanos() < deadline {
            let generation = self.read(CONFIG_GENERATION);
            let low = self.read(CONFIG + offset);
            let high = self.read(CONFIG + offset + 4);
            if self.read(CONFIG_GENERATION) == generation {
                return Some((u64::from(high) << 32) | u64::from(low));
            }
        }
        None
    }

    pub(super) fn set_queue(
        &self,
        index: u32,
        size: u32,
        descriptor: u64,
        available: u64,
        used: u64,
    ) -> bool {
        self.write(QUEUE_SEL, index);
        if self.read(QUEUE_READY) != 0 || self.read(QUEUE_NUM_MAX) < size {
            return false;
        }
        self.write(QUEUE_NUM, size);
        self.write_address(QUEUE_DESC, descriptor);
        self.write_address(QUEUE_DRIVER, available);
        self.write_address(QUEUE_DEVICE, used);
        self.write(QUEUE_READY, 1);
        self.read(QUEUE_READY) == 1
    }

    pub(super) fn start(&self) -> bool {
        self.write(STATUS, ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK);
        self.operational()
    }

    pub(super) fn operational(&self) -> bool {
        self.has_status(FEATURES_OK | DRIVER_OK)
    }

    pub(super) fn notify(&self, index: u32) {
        self.write(QUEUE_NOTIFY, index);
    }

    fn has_status(&self, required: u32) -> bool {
        self.read(STATUS) & (required | DEVICE_NEEDS_RESET | FAILED) == required
    }

    fn write_address(&self, register: usize, address: u64) {
        self.write(register, address as u32);
        self.write(register + 4, (address >> 32) as u32);
    }

    fn read(&self, register: usize) -> u32 {
        order_device_accesses();
        let address = without_provenance::<u32>(self.base + register);
        // SAFETY: The selected transport exposes aligned 32-bit registers.
        let value = u32::from_le(unsafe { address.read_volatile() });
        order_device_accesses();
        value
    }

    fn write(&self, register: usize, value: u32) {
        order_device_accesses();
        let address = without_provenance_mut::<u32>(self.base + register);
        // SAFETY: The selected transport exposes aligned 32-bit registers.
        unsafe { address.write_volatile(value.to_le()) };
    }
}
